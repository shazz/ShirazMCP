//! ADR-11 §run-to-break: `Session<Paused>` can run the machine until an
//! armed breakpoint fires and adopt the resulting freeze — the managed
//! alternative to hand-arming breakpoints via the raw `debug` tool.

use crate::error::Error;
use crate::session::{Paused, Running, Session};

/// Every arm returns the session in a compile-time-known state — a fired
/// breakpoint can never leave the FSM guessing (the "known state"
/// requirement from ADR-11's limitation note).
#[derive(Debug)]
pub enum RunToBreak {
    /// A breakpoint fired within the budget: frozen at it. `output` holds
    /// the debugger-entry log lines (§3.2 — match report, stats, …).
    Fired(Session<Paused>, String),
    /// Nothing fired: the machine was left RUNNING. Whatever was armed is
    /// still armed — callers own disarming (§8.1 recovery).
    TimedOut(Session<Running>),
    /// Control-plane I/O failed (Hatari almost certainly died). Treated as
    /// Running so `stop` — the only sensible next call — is reachable.
    Broken(Session<Running>, Error),
}

impl Session<Paused> {
    /// `c` + queued sentinel probe; see `HatariProcess::continue_until_break`.
    pub async fn run_until_break(mut self, timeout_ms: u64) -> RunToBreak {
        match self.data.continue_until_break(timeout_ms).await {
            Ok(output) => RunToBreak::Fired(self, output),
            Err(e @ Error::Timeout { .. }) => {
                let _ = e; // timeout detail already logged by read_until
                RunToBreak::TimedOut(Session {
                    config: self.config,
                    data: self.data,
                })
            }
            Err(e) => RunToBreak::Broken(
                Session {
                    config: self.config,
                    data: self.data,
                },
                e,
            ),
        }
    }
}
