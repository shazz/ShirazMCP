//! Managed run-to-breakpoint choreography (ADR-11 §known-limitation,
//! resolved by ADR-15). Owns: freeze if needed → arm :once over stdin →
//! continue + wait for fire → recover to a KNOWN state on every path.
//! Invariant: this module never leaves a session in a state where a
//! breakpoint can fire behind the FSM's back — every exit is either
//! Paused-at-breakpoint, or Running with the managed breakpoint disarmed,
//! or an error that names the one narrow race and its remedy (pause).

use crate::debug::breakpoint as bp;
use crate::error::{Error, Result};
use crate::session::{AnySession, HatariProcess, Paused, RunToBreak, Session};

/// Outcome the tools serialize.
#[derive(Debug)]
pub(crate) enum Fire {
    /// Frozen at the breakpoint; `pc` read post-freeze; `output` = §3.2
    /// entry block (protocol-stripped); `ours` = the fired expression
    /// matched the condition this call armed (false ⇒ a foreign
    /// breakpoint fired first; ours was disarmed).
    Fired { pc: u32, output: String, ours: bool },
    /// Budget elapsed; machine left Running, managed breakpoint disarmed.
    TimedOut,
}

/// Arm `condition` as :once and run until it fires (or `timeout_ms`).
pub(crate) async fn run_to_breakpoint(
    guard: &mut AnySession,
    condition: &str,
    timeout_ms: u64,
) -> Result<Fire> {
    let (paused, was_running) = freeze_if_needed(guard).await?;
    let (paused, armed) = arm_or_restore(guard, paused, condition, was_running).await?;

    match paused.run_until_break(timeout_ms).await {
        RunToBreak::Fired(session, output) => handle_fired(guard, session, output, &armed).await,
        RunToBreak::TimedOut(mut running) => {
            // Known state first (§8.1): even if the disarm below hits the
            // fire/timeout race, the session is already Running.
            let disarm_result = disarm_ours(running.process_mut(), &armed.expression).await;
            *guard = AnySession::Running(running);
            disarm_result?;
            Ok(Fire::TimedOut)
        }
        RunToBreak::Broken(running, e) => {
            *guard = AnySession::Running(running);
            Err(e)
        }
    }
}

/// A breakpoint fired (ours or a foreign one): read PC, disarm ours if it
/// wasn't the one that fired (Hatari already auto-removed it if it was),
/// and adopt the frozen session.
async fn handle_fired(
    guard: &mut AnySession,
    mut session: Session<Paused>,
    output: String,
    armed: &bp::Armed,
) -> Result<Fire> {
    let pc = crate::debug::evaluate(session.process_mut(), "pc").await?;
    let ours_str = format!("{} :once", armed.expression);
    // "Among", not "first": `:trace` hits (count_hits, ADR-47) print the
    // same match header without stopping, ahead of the one that froze us.
    let ours = bp::fired_expressions(&output).contains(&ours_str);
    if !ours {
        disarm_ours(session.process_mut(), &armed.expression).await?;
    }
    *guard = AnySession::Paused(session);
    Ok(Fire::Fired { pc, output, ours })
}

/// Freeze a Running session (remembering that it was Running, so a later
/// failure can resume it back), or adopt an already-Paused one as-is.
/// Stopped is rejected; on any error the guard is restored to a known
/// state before returning.
async fn freeze_if_needed(guard: &mut AnySession) -> Result<(Session<Paused>, bool)> {
    let config = guard.config().clone();
    match std::mem::replace(guard, AnySession::new(config)) {
        AnySession::Stopped(s) => {
            *guard = AnySession::Stopped(s);
            Err(Error::Session("no live session".to_string()))
        }
        AnySession::Paused(s) => Ok((s, false)),
        AnySession::Running(s) => match s.pause().await {
            Ok(p) => Ok((p, true)),
            Err((s, e)) => {
                *guard = AnySession::Running(s);
                Err(e)
            }
        },
    }
}

/// Freeze a Running session and re-seat the guard as Paused (no-op when
/// already Paused; Stopped errors) so a caller can do preparatory frozen
/// reads before a `run_to_breakpoint` — which will then find the session
/// already Paused. Deliberate semantic drift, documented here: because the
/// original Running-ness is forgotten, a subsequent arm failure leaves the
/// session Paused instead of restored-to-Running. Every caller reports the
/// true session state in its output, and `resume` recovers.
pub(super) async fn ensure_paused(guard: &mut AnySession) -> Result<()> {
    let (paused, _was_running) = freeze_if_needed(guard).await?;
    *guard = AnySession::Paused(paused);
    Ok(())
}

/// `bp::arm_once`, restoring the pre-call state on failure (arming never
/// half-changes the world — §8.1 step 3).
async fn arm_or_restore(
    guard: &mut AnySession,
    mut paused: Session<Paused>,
    condition: &str,
    was_running: bool,
) -> Result<(Session<Paused>, bp::Armed)> {
    match bp::arm_once(paused.process_mut(), condition).await {
        Ok(armed) => Ok((paused, armed)),
        Err(e) => {
            restore_after_arm_failure(guard, paused, was_running).await;
            Err(e)
        }
    }
}

/// Arming failed (bad condition / Hatari `ERROR:`): restore the initial
/// state exactly — resume if we froze it ourselves, else stay Paused.
async fn restore_after_arm_failure(
    guard: &mut AnySession,
    paused: Session<Paused>,
    was_running: bool,
) {
    if !was_running {
        *guard = AnySession::Paused(paused);
        return;
    }
    match paused.resume().await {
        Ok(r) => *guard = AnySession::Running(r),
        Err((p, e)) => {
            tracing::warn!(error = %e, "failed to resume after breakpoint-arm failure");
            *guard = AnySession::Paused(p);
        }
    }
}

/// Best-effort: if our `:once` condition is still armed (something else
/// fired first, or the disarm-on-timeout race), remove it. Absence is
/// fine — Hatari auto-removes a fired `:once` breakpoint itself.
async fn disarm_ours(proc: &mut HatariProcess, condition: &str) -> Result<()> {
    let wanted = format!("{condition} :once");
    let entries = bp::list(proc).await.map_err(map_race)?;
    if let Some(entry) = entries.iter().find(|e| e.expression == wanted) {
        bp::remove(proc, entry.index).await.map_err(map_race)?;
    }
    Ok(())
}

/// A timeout while disarming means the breakpoint fired in the window
/// between the run-to-break timeout and this cleanup — the one
/// unavoidable race (§8.1). Its remedy is `pause`, which recovers per
/// ADR-11 (a stdin sentinel succeeds against an already-frozen debugger).
fn map_race(e: Error) -> Error {
    match e {
        Error::Timeout { .. } => Error::Session(
            "breakpoint fired during timeout recovery — call pause to adopt the frozen session"
                .to_string(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_race_renames_timeout() {
        let e = Error::Timeout {
            what: "x".to_string(),
            waited_ms: 1,
        };
        assert!(matches!(map_race(e), Error::Session(_)));
    }

    #[test]
    fn map_race_passes_through_other_errors() {
        let e = Error::Parse {
            context: "x".to_string(),
            input: "y".to_string(),
        };
        assert!(matches!(map_race(e), Error::Parse { .. }));
    }
}
