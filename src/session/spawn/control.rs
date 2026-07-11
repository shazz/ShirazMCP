//! ADR-11: freeze/thaw over Hatari's stdin, the frozen-state control plane.
//! Split out of `spawn/mod.rs` purely to keep that file under the line
//! ceiling (precedent: `boot.rs`). `shutdown` joined it in P11 for the same
//! reason (§6.1 fallback) — otherwise unrelated to freeze/thaw.

use std::time::Duration;

use super::HatariProcess;
use crate::error::{Error, Result};
use crate::session::fifo::{self, HATARI_DEBUG_PREFIX};

/// Which control plane services debugger commands. Runtime mirror of the
/// type-state (Running=Fifo, Paused=DebugStdin); private, mutated ONLY by
/// `freeze()`/`thaw()` so it cannot drift from the `Session<S>` transitions.
#[derive(Debug)]
pub(super) enum ControlChannel {
    Fifo,
    DebugStdin,
}

/// ADR-11 §2.1 verbatim wire strings — never build these ad hoc.
const ARM_FREEZE: &str = "hatari-debug b pc > 0 :once";
const THAW_CMD: &str = "c";
/// Any cheap, side-effect-free command works to sentinel-confirm a control
/// plane is live; `evaluate pc` matches the existing `pause`/`resume`
/// convention (P1.md §5.3) so log traces read consistently.
const LIVENESS_PROBE: &str = "evaluate pc";

impl HatariProcess {
    /// Freeze: arm `b pc > 0 :once` on the fifo (raw, no sentinel — a fifo
    /// sentinel would queue behind the freeze and replay on resume), then
    /// sentinel-confirm over stdin (echo ⇒ DebugUI entered ⇒ CPU parked),
    /// then drain any stray debugger-entry text so the control plane starts
    /// clean (`drain_stdin_entry_text`). On error from the arm/confirm step
    /// `self.control` stays `Fifo` — the session should be treated as
    /// still-Running (ADR-11's documented known limitation covers the case
    /// where the CPU froze anyway: a retried `pause` recovers). Once confirm
    /// succeeds the CPU genuinely IS parked, so `control` flips to
    /// `DebugStdin` before the drain runs and stays there even if the drain
    /// itself errors (a drain `Timeout` is swallowed internally; only a
    /// real I/O error — a dead stdin, i.e. a dead session — propagates).
    pub(crate) async fn freeze(&mut self) -> Result<()> {
        fifo::write_wrapper(&mut self.fifo, ARM_FREEZE).await?;
        let what = "freeze (debugger entry after b pc > 0 :once)";
        fifo::debug(
            &mut self.stdin,
            &mut self.log,
            "",
            LIVENESS_PROBE,
            self.config.sentinel_timeout_ms,
            what,
        )
        .await?;
        self.control = ControlChannel::DebugStdin;
        self.drain_stdin_entry_text().await
    }

    /// Drain stray debugger-entry text left queued on the stdin control
    /// plane: breakpoint arm/fire banners, the "entered debug mode" banner
    /// and, for programs that carry one, the DRI/GST symbol-table load
    /// message. Called once after `freeze`'s own confirm round-trip and
    /// once after a managed breakpoint fires (`continue_until_break`, ADR-15)
    /// — both are a fresh entry into DebugUI and both can leave this text
    /// behind. Per `read_until`'s doc, a line arriving in the same poll as —
    /// but after — the CALLER's confirm sentinel is left queued in
    /// `LogTail::pending` for the next call rather than consumed. Undrained,
    /// it corrupts the first real post-entry command: a stale queued
    /// sentinel echo can satisfy that command's OWN sentinel search early,
    /// returning before Hatari actually finished (observed live as
    /// truncated `savebin`/`memory::acquire` reads). One extra, fully
    /// discarded sentinel round-trip drains it — `pending` carries into
    /// this call's `read_until`, so leftover text becomes this probe's
    /// (discarded) output, terminated by this probe's own echo. Confirmed
    /// live: MJJHACK (no symbol table, ADR-8) never exercises this path
    /// (nothing to drain); a symbol-bearing program reproduces the
    /// corruption every time without this call.
    ///
    /// A drain `Timeout` is swallowed (warned, not propagated) — the caller
    /// already sentinel-confirmed the CPU is parked, so a slow/absent drain
    /// echo is hygiene, not a broken freeze/break. A non-timeout error (dead
    /// stdin) IS propagated.
    ///
    /// Known limitation (tracked for a follow-up ADR, not closed here): the
    /// caller's own confirm sentinel is retried on a fixed timer
    /// (`SENTINEL_RETRY_BUDGET`) when Hatari is slow to answer — exactly
    /// what heavy symbol-table loading causes — which can leave MULTIPLE
    /// stale sentinel echoes queued, not just banner text. This drain can
    /// terminate on one of those instead of absorbing everything past it;
    /// closing that fully needs per-call-unique sentinels rather than the
    /// fixed `$DEADBEEF` family, which is future work, not this fix.
    async fn drain_stdin_entry_text(&mut self) -> Result<()> {
        let what = "stdin drain (stray debugger-entry banner/symbol text)";
        match fifo::debug(
            &mut self.stdin,
            &mut self.log,
            "",
            LIVENESS_PROBE,
            self.config.sentinel_timeout_ms,
            what,
        )
        .await
        {
            Ok(drained) => {
                if !drained.is_empty() {
                    tracing::debug!(bytes = drained.len(), "drained stray debugger-entry text");
                }
                Ok(())
            }
            Err(Error::Timeout { .. }) => {
                tracing::warn!("stdin drain timed out; proceeding (CPU already parked)");
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Thaw: `c` on stdin — MUST be the last stdin write of the pause
    /// episode (ADR-11) — flip to `Fifo` immediately (writes after `c` are
    /// illegal, so from here on every command belongs on the fifo even if
    /// the confirm below fails), then a fifo sentinel round-trip confirms
    /// the main loop is servicing it again.
    pub(crate) async fn thaw(&mut self) -> Result<()> {
        fifo::write_wrapper(&mut self.stdin, THAW_CMD).await?;
        self.control = ControlChannel::Fifo;
        let what = "resume (fifo liveness after c)";
        fifo::debug(
            &mut self.fifo,
            &mut self.log,
            HATARI_DEBUG_PREFIX,
            LIVENESS_PROBE,
            self.config.sentinel_timeout_ms,
            what,
        )
        .await?;
        Ok(())
    }

    /// Run until the next debugger entry (i.e. until an armed breakpoint
    /// fires). Precondition: frozen (`ControlChannel::DebugStdin`) — callers
    /// arm their breakpoint over stdin first. On Ok the machine is frozen
    /// again and `control` stays `DebugStdin` — a fired breakpoint is a
    /// fresh DebugUI entry exactly like `freeze`'s, so it gets the same
    /// stray-text drain before the match report is handed back (the
    /// breakpoint's own output is preserved; only the drain's is
    /// discarded). On `Err(Timeout)` (or any I/O error after `c` was
    /// written) the machine is running, so `control` flips to `Fifo` before
    /// returning.
    pub(crate) async fn continue_until_break(&mut self, timeout_ms: u64) -> Result<String> {
        if !matches!(self.control, ControlChannel::DebugStdin) {
            return Err(Error::Session(
                "run_until_break requires a frozen session".to_string(),
            ));
        }
        let what = "breakpoint fire (sentinel probe queued behind c)";
        match fifo::continue_and_wait(&mut self.stdin, &mut self.log, timeout_ms, what).await {
            Ok(output) => {
                self.drain_stdin_entry_text().await?;
                Ok(output)
            }
            Err(e) => {
                self.control = ControlChannel::Fifo;
                Err(e)
            }
        }
    }

    /// Wait for a log line containing `needle` (bounded). Used by state
    /// load, whose completion outlives the sentinel round-trip (§3.6).
    pub async fn wait_for_log(&mut self, needle: &str, timeout_ms: u64) -> Result<()> {
        self.log
            .read_until(|l| l.contains(needle), needle, timeout_ms, &mut Vec::new())
            .await
    }

    /// Graceful kill: if frozen, thaw first (a frozen Hatari never reads
    /// the fifo quit — ADR-11), then write "hatari-shortcut quit", wait up
    /// to 2s, else SIGKILL. Never returns `Err` — a session must always be
    /// stoppable (P0 §7.2 contract).
    pub async fn shutdown(mut self) {
        if matches!(self.control, ControlChannel::DebugStdin) {
            if let Err(e) = fifo::write_wrapper(&mut self.stdin, "c").await {
                tracing::warn!(error = %e, "failed to thaw before shutdown");
            }
            self.control = ControlChannel::Fifo;
        }
        if let Err(e) = self.write_wrapper("hatari-shortcut quit").await {
            tracing::warn!(error = %e, "failed to write graceful quit");
        }
        match tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "error waiting for hatari exit"),
            Err(_) => {
                if let Err(e) = self.child.kill().await {
                    tracing::warn!(error = %e, "failed to SIGKILL hatari");
                }
            }
        }
    }
}
