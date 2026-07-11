use super::{HatariProcess, Paused, Running, SessionConfig, Stopped};
use crate::session::Session;

/// Runtime-dispatch wrapper for the tool layer (ADR-3): MCP tools cannot
/// know the state at compile time, so they hold this enum behind a
/// `tokio::sync::Mutex` and match. Each arm only exposes what that state's
/// `Session<S>` legally offers — the enum gives dispatch, the type-state
/// gives safety.
///
/// **Transition pattern** (documented now, used from P1): mutating tools
/// will `let mut guard = mutex.lock().await;` then move the session out with
/// `std::mem::replace(&mut *guard, AnySession::new(config_clone))`, run the
/// consuming transition, and write the result back. P0 implements no
/// mutating dispatch — `status` only reads.
#[derive(Debug)]
pub enum AnySession {
    Stopped(Session<Stopped>),
    Running(Session<Running>),
    Paused(Session<Paused>),
}

impl AnySession {
    /// Fresh session in `Stopped`.
    pub fn new(config: SessionConfig) -> Self {
        AnySession::Stopped(Session::new(config))
    }

    /// "stopped" | "running" | "paused" — delegates to the inner
    /// `state_name()`, never re-derives the string.
    pub fn state_name(&self) -> &'static str {
        match self {
            AnySession::Stopped(s) => s.state_name(),
            AnySession::Running(s) => s.state_name(),
            AnySession::Paused(s) => s.state_name(),
        }
    }

    pub fn config(&self) -> &SessionConfig {
        match self {
            AnySession::Stopped(s) => s.config(),
            AnySession::Running(s) => s.config(),
            AnySession::Paused(s) => s.config(),
        }
    }

    /// The live process, if Running or Paused. Tools that need a live
    /// session call this and map `None` → wrong-state `McpError` (§8.1).
    pub fn process_mut(&mut self) -> Option<&mut HatariProcess> {
        match self {
            AnySession::Stopped(_) => None,
            AnySession::Running(s) => Some(s.process_mut()),
            AnySession::Paused(s) => Some(s.process_mut()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn any_session_new_is_stopped() {
        let s = AnySession::new(SessionConfig::default());
        assert_eq!(s.state_name(), "stopped");
        assert_eq!(s.config().hatari_path, PathBuf::from("/usr/bin/hatari"));
    }
}
