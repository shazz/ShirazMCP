//! `send_key` — inject IKBD key/mouse events over the existing control fifo
//! (P27 §3, ADR-44), optionally composed with the ADR-15 managed frame-
//! runner for deterministic headless capture.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::frames;
use super::input_wire::{self, KeyEvent, KEYS_MAX};
use super::managed::{self, Fire};
use super::watch_wire::cap_output_tail;
use super::{to_mcp, wrong_state, HatariServer};
use crate::error::Error;
use crate::session::AnySession;
use crate::types::Addr;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendKeyArgs {
    /// Key sequence, 1..=14 entries. Each entry: a single ASCII
    /// alphanumeric character ("a", "5" — uppercase auto-shifts), an ST
    /// scancode ("57", "0x39"), a named alias (space, enter, esc, f1..f10,
    /// up/down/left/right, help, undo, ...), or a mouse event
    /// (doubleclick, rightdown, rightup).
    pub keys: Vec<String>,
    /// "press" (default; press+release), "down", or "up". Programs that
    /// POLL key state each frame can miss a same-slice press+release: use
    /// down + frames + a second call with up.
    pub action: Option<String>,
    /// Run exactly N VBL frames after injecting (1..=5000) and end PAUSED —
    /// the headless capture flow: send_key(keys, frames) → screenshot.
    /// Omitted: running sessions keep running (keys land within one main-
    /// loop iteration); paused sessions queue the keys (queued=true) until
    /// resume/run_frames.
    pub frames: Option<u32>,
    /// Wait budget for the frames run, ms; default as run_frames.
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SendKeyOutput {
    /// Canonical events as written, e.g. ["hatari-event keypress 57"].
    pub sent: Vec<String>,
    /// true iff the session is paused and no frames were requested: events
    /// sit in the fifo and take effect on the next resume/run_frames.
    pub queued: bool,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames_requested: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames_completed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[tool_router(router = input_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "send_key",
        description = "Inject IKBD key/mouse events into the running or \
                       paused emulator over the control fifo. keys: 1..=14 \
                       entries — single ASCII alphanumeric characters, ST \
                       scancodes (decimal or 0x-hex, 0..=255), named \
                       aliases (space, enter, esc, f1..f10, up/down/left/ \
                       right, help, undo, ...), or mouse events \
                       (doubleclick, rightdown, rightup). action: press \
                       (default, press+release in one main-loop slice — \
                       state-polling programs may miss it, use down then \
                       up across two calls instead), down, or up. Optional \
                       frames: N composes with the managed frame-runner \
                       for deterministic headless capture — freeze, queue \
                       the keys, run exactly N VBL frames, freeze again \
                       (send_key(keys, frames) then screenshot). Without \
                       frames: a Running session gets the keys within one \
                       main-loop iteration; a Paused session queues them \
                       (queued=true) until resume/run_frames."
    )]
    pub async fn send_key(
        &self,
        Parameters(a): Parameters<SendKeyArgs>,
    ) -> Result<Json<SendKeyOutput>, McpError> {
        let sent = build_sent(&a)?;
        let mut guard = self.session.lock().await;
        require_live(&guard)?;
        if let Some(n) = a.frames {
            return with_frames(&mut guard, sent, n, a.timeout_ms)
                .await
                .map(Json);
        }
        if matches!(*guard, AnySession::Paused(_)) {
            queued_output(&mut guard, sent).await.map(Json)
        } else {
            running_output(&mut guard, sent).await.map(Json)
        }
    }
}

fn invalid(msg: String) -> McpError {
    McpError::invalid_params(msg, None)
}

/// §3.5 step 1: everything client-side, nothing written unless all of it
/// parses (`Control_ProcessBuffer`'s batch-abort, P27 §1.2).
fn build_sent(a: &SendKeyArgs) -> Result<Vec<String>, McpError> {
    if !(1..=KEYS_MAX).contains(&a.keys.len()) {
        return Err(invalid(format!(
            "keys must have 1..={KEYS_MAX} entries (got {})",
            a.keys.len()
        )));
    }
    let action = input_wire::parse_action(a.action.as_deref()).map_err(invalid)?;
    let events: Vec<KeyEvent> = a
        .keys
        .iter()
        .map(|k| input_wire::parse_key_entry(k).map_err(|m| invalid(format!("key '{k}': {m}"))))
        .collect::<Result<_, _>>()?;
    input_wire::wire_lines(&events, action).map_err(invalid)
}

fn require_live(guard: &AnySession) -> Result<(), McpError> {
    match guard {
        AnySession::Stopped(_) => Err(wrong_state(guard.state_name(), "running or paused")),
        _ => Ok(()),
    }
}

/// Write every canonical wire line via `write_wrapper` (queues while
/// frozen, serviced within one main-loop iteration while running — P27
/// §1.2). Fails fast on the first write error (a dead child, most likely).
async fn write_all(guard: &mut AnySession, lines: &[String]) -> Result<(), McpError> {
    let proc = guard
        .process_mut()
        .ok_or_else(|| to_mcp(Error::Session("no live session".to_string())))?;
    for line in lines {
        proc.write_wrapper(line).await.map_err(to_mcp)?;
    }
    Ok(())
}

/// §3.5 step 5: Paused + no `frames` — no confirm is possible (a frozen
/// Hatari reads nothing, P27 §1.2), so this is honest, not lazy.
async fn queued_output(
    guard: &mut AnySession,
    sent: Vec<String>,
) -> Result<SendKeyOutput, McpError> {
    write_all(guard, &sent).await?;
    Ok(SendKeyOutput {
        sent,
        queued: true,
        state: "paused".to_string(),
        frames_requested: None,
        frames_completed: None,
        pc: None,
        output: None,
    })
}

/// §3.5 step 4: Running + no `frames` — one confirm round-trip proves the
/// whole batch (events + sentinel) was consumed by `Control_ProcessBuffer`;
/// scan its output for a Hatari-side rejection.
async fn running_output(
    guard: &mut AnySession,
    sent: Vec<String>,
) -> Result<SendKeyOutput, McpError> {
    write_all(guard, &sent).await?;
    let proc = guard
        .process_mut()
        .ok_or_else(|| to_mcp(Error::Session("no live session".to_string())))?;
    let confirm = proc.debug("evaluate pc").await.map_err(to_mcp)?;
    check_no_error_lines(&confirm)?;
    Ok(SendKeyOutput {
        sent,
        queued: false,
        state: "running".to_string(),
        frames_requested: None,
        frames_completed: None,
        pc: None,
        output: None,
    })
}

/// §3.5 step 3: the headless-capture composition — ensure_paused, queue the
/// keys, run exactly `n` VBL frames on the ADR-15 managed lifecycle.
async fn with_frames(
    guard: &mut AnySession,
    sent: Vec<String>,
    n: u32,
    timeout_ms: Option<u64>,
) -> Result<SendKeyOutput, McpError> {
    frames::validate_frames(n)?;
    let timeout = timeout_ms.unwrap_or_else(|| frames::default_timeout(n));
    managed::ensure_paused(guard).await.map_err(to_mcp)?;
    let vbl_before = frames::current_vbl(guard).await?;
    frames::check_vbl_headroom(vbl_before, n)?;
    write_all(guard, &sent).await?;
    let condition = frames::vbl_condition(vbl_before, n);
    let fire = managed::run_to_breakpoint(guard, &condition, timeout)
        .await
        .map_err(to_mcp)?;
    build_frames_output(sent, n, fire)
}

fn build_frames_output(sent: Vec<String>, n: u32, fire: Fire) -> Result<SendKeyOutput, McpError> {
    match fire {
        Fire::TimedOut => Ok(SendKeyOutput {
            sent,
            queued: false,
            state: "running".to_string(),
            frames_requested: Some(n),
            frames_completed: Some(false),
            pc: None,
            output: None,
        }),
        Fire::Fired { pc, output, ours } => {
            check_no_error_lines(&output)?;
            Ok(SendKeyOutput {
                sent,
                queued: false,
                state: "paused".to_string(),
                frames_requested: Some(n),
                frames_completed: Some(ours),
                pc: Some(Addr::new(pc).to_string()),
                output: Some(cap_output_tail(output)),
            })
        }
    }
}

/// Belt-and-braces (P27 §3.5 step 3): should be unreachable after
/// client-side validation, but a Hatari-side rejection must never be
/// silently swallowed.
fn check_no_error_lines(output: &str) -> Result<(), McpError> {
    match output.lines().find(|l| l.starts_with("ERROR: ")) {
        Some(line) => Err(invalid(format!("hatari rejected a queued event: {line}"))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(keys: &[&str], frames: Option<u32>) -> SendKeyArgs {
        SendKeyArgs {
            keys: keys.iter().map(|s| s.to_string()).collect(),
            action: None,
            frames,
            timeout_ms: None,
        }
    }

    #[test]
    fn build_sent_rejects_empty_keys() {
        assert!(build_sent(&args(&[], None)).is_err());
    }

    #[test]
    fn build_sent_rejects_over_max_keys() {
        let keys = vec!["a"; KEYS_MAX + 1];
        assert!(build_sent(&args(&keys, None)).is_err());
    }

    #[test]
    fn build_sent_accepts_max_keys() {
        let keys = vec!["a"; KEYS_MAX];
        assert!(build_sent(&args(&keys, None)).is_ok());
    }

    #[test]
    fn build_sent_names_the_offending_entry() {
        let err = build_sent(&args(&["a", "!"], None)).unwrap_err();
        assert!(err.message.contains('!'));
    }

    #[test]
    fn build_sent_canonicalizes_wire_lines() {
        let sent = build_sent(&args(&["space"], None)).unwrap();
        assert_eq!(sent, vec!["hatari-event keypress 57".to_string()]);
    }

    #[test]
    fn check_no_error_lines_catches_hatari_rejection() {
        let out = "some output\nERROR: unrecognized event: 'x'\nmore";
        assert!(check_no_error_lines(out).is_err());
    }

    #[test]
    fn check_no_error_lines_passes_clean_output() {
        assert!(check_no_error_lines("nothing interesting here").is_ok());
    }
}
