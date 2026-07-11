//! P27/ADR-43 §4: broken-pipe classification and bounded recovery. `EPIPE`
//! on either control plane means Hatari exited or is in its exit path
//! (`Control_RemoveFifo` — v2.4.1 `control.c:489` — runs only from
//! `Control_SetFifo`'s own re-open, a startup option-parse failure, or
//! `main.c:971`, the line immediately after `M68000_Start()` returns; a
//! live Hatari, even one frozen in DebugUI, holds both read ends open).
//! Precedent for splitting an `impl HatariProcess` block out of
//! `spawn/mod.rs`: `control.rs` (the line-ceiling fallback named there).

use std::time::Duration;

use super::boot;
use super::HatariProcess;
use crate::error::{Error, Result};

/// Post-EPIPE fifo reopen bound (P27 §4.3): a live Hatari holds the read
/// end open, so the blocking `O_WRONLY` open returns instantly; a mid-exit
/// Hatari has already removed the fifo file (`main.c:971`) and the open
/// fails fast. Deliberately NOT `boot_timeout_ms` — this open either
/// succeeds immediately or never.
const REOPEN_TIMEOUT_MS: u64 = 1_000;

/// Brief grace before the final liveness re-probe on the STDIN plane
/// (P27 §4.3): covers the narrow mid-exit window where `try_wait()` has not
/// yet observed the exit. Not a poll loop — one sleep, one re-probe, no
/// retry beyond it — so this does not reintroduce the blind-sleep pattern
/// ADR-7 exists to avoid (there is no completion signal to poll for here;
/// the OS either has reaped the child by the time we look again, or not).
const STDIN_EPIPE_GRACE: Duration = Duration::from_millis(200);

/// True iff `e` is a control-plane write that hit EPIPE. `Error::Fifo` is
/// the variant `write_line` uses for both writers (`fifo/mod.rs`), so this
/// covers stdin writes too; the uncontextualized `Error::Io` variant never
/// qualifies — only a control-plane-attributed EPIPE means "Hatari died".
pub(crate) fn is_broken_pipe(e: &Error) -> bool {
    matches!(e, Error::Fifo { source, .. } if source.kind() == std::io::ErrorKind::BrokenPipe)
}

impl HatariProcess {
    /// Non-blocking liveness probe. `Some(status)` ⇒ the child has exited
    /// (and been reaped by this call — `try_wait` consumes the zombie).
    /// `None` covers both "still running" and "wait failed" — either way,
    /// this is not evidence of death, so callers must not retry-forever on
    /// a `None` that never resolves; every call site here bounds itself.
    pub fn exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// After an EPIPE on the FIFO plane: confirm the child is alive first
    /// (dead ⇒ `Error::ProcessExited`, never retried into a corpse), then
    /// reopen the fifo write end once, bounded. A reopen failure gets one
    /// final liveness check before propagating — a fast-failing open is
    /// almost certainly the mid-exit fifo-removal window, not a transient
    /// fault.
    pub(super) async fn recover_fifo_epipe(&mut self) -> Result<()> {
        if let Some(status) = self.exit_status() {
            return Err(Error::ProcessExited {
                status: status.to_string(),
            });
        }
        match boot::open_fifo_write(&self.config, REOPEN_TIMEOUT_MS).await {
            Ok(fifo) => {
                self.fifo = fifo;
                Ok(())
            }
            Err(open_err) => {
                Err(self
                    .exit_status()
                    .map_or(open_err, |status| Error::ProcessExited {
                        status: status.to_string(),
                    }))
            }
        }
    }

    /// After an EPIPE on the STDIN plane: no reopen is possible (a child's
    /// stdin is fixed at spawn — the kernel closes the read end only when
    /// the process exits). Classify only, after a brief grace + re-probe
    /// for the mid-exit window. The `Error::Session` arm should be
    /// unreachable per §1.4 (a live Hatari never closes its stdin read
    /// end), but must not panic or loop if it somehow is.
    pub(super) async fn classify_stdin_epipe(&mut self) -> Error {
        if let Some(status) = self.exit_status() {
            return Error::ProcessExited {
                status: status.to_string(),
            };
        }
        tokio::time::sleep(STDIN_EPIPE_GRACE).await;
        match self.exit_status() {
            Some(status) => Error::ProcessExited {
                status: status.to_string(),
            },
            None => Error::Session(
                "hatari stdin broken but process alive — unrecoverable; call stop".to_string(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn fifo_err(kind: io::ErrorKind) -> Error {
        Error::Fifo {
            context: "write cmd".to_string(),
            source: io::Error::new(kind, "boom"),
        }
    }

    #[test]
    fn is_broken_pipe_true_for_fifo_epipe() {
        assert!(is_broken_pipe(&fifo_err(io::ErrorKind::BrokenPipe)));
    }

    #[test]
    fn is_broken_pipe_false_for_other_fifo_error_kinds() {
        assert!(!is_broken_pipe(&fifo_err(io::ErrorKind::NotFound)));
    }

    #[test]
    fn is_broken_pipe_false_for_timeout() {
        let e = Error::Timeout {
            what: "x".to_string(),
            waited_ms: 1,
        };
        assert!(!is_broken_pipe(&e));
    }

    /// Only a contextualized `Error::Fifo` qualifies — the uncontextualized
    /// `Error::Io` fringe variant never triggers control-plane recovery,
    /// even carrying the same OS error kind.
    #[test]
    fn is_broken_pipe_false_for_plain_io_epipe() {
        let e: Error = io::Error::new(io::ErrorKind::BrokenPipe, "boom").into();
        assert!(matches!(e, Error::Io(_)));
        assert!(!is_broken_pipe(&e));
    }
}
