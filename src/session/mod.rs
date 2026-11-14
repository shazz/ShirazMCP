pub mod fifo;
pub mod memory;
pub mod record;
pub mod spawn;
pub mod state;

mod any;
mod run;
pub use any::AnySession;
pub use run::RunToBreak;
pub use spawn::HatariConfig as SessionConfig;
pub use spawn::{HatariConfig, HatariProcess, MachineOptions};
pub use state::{LiveState, Paused, Running, SessionState, Stopped};

use crate::error::Error;

/// A Hatari session in compile-time-known state `S` (ADR-3). Transitions
/// consume `self`; a method that does not exist on a state is a compile
/// error, not a runtime check.
#[derive(Debug)]
pub struct Session<S: SessionState> {
    pub(in crate::session) config: HatariConfig,
    pub(in crate::session) data: S::Data,
}

// ---- available in every state ----
impl<S: SessionState> Session<S> {
    /// The state name as reported by the `status` tool.
    pub fn state_name(&self) -> &'static str {
        S::NAME
    }

    pub fn config(&self) -> &HatariConfig {
        &self.config
    }
}

// ---- Stopped ----
impl Session<Stopped> {
    /// The only public entry point into the FSM.
    pub fn new(config: HatariConfig) -> Self {
        Self { config, data: () }
    }

    /// Spawn + sentinel-gated boot (§5.1). On failure the session is
    /// returned so it is never lost (ADR-3 error shape).
    pub async fn start(self) -> Result<Session<Running>, (Session<Stopped>, Error)> {
        match HatariProcess::spawn(self.config.clone()).await {
            Ok(proc) => Ok(Session {
                config: self.config,
                data: proc,
            }),
            Err(e) => Err((self, e)),
        }
    }
}

// ---- shared by Running + Paused ----
impl<S: LiveState> Session<S> {
    /// The debugger/memory channel. Tools and the debug layer go through
    /// this; only `LiveState` sessions have it (compile-time guarantee).
    pub fn process_mut(&mut self) -> &mut HatariProcess {
        &mut self.data
    }

    pub fn process(&self) -> &HatariProcess {
        &self.data
    }

    /// Kill + reap (graceful path §5.1 shutdown). Infallible by contract.
    pub async fn stop(self) -> Session<Stopped> {
        self.data.shutdown().await;
        Session {
            config: self.config,
            data: (),
        }
    }
}

// ---- Running ----
impl Session<Running> {
    /// Freeze emulation (ADR-11): arm a one-shot breakpoint over the fifo,
    /// then sentinel-confirm the freeze over Hatari's stdin (the frozen-
    /// state control plane — DebugUI reads stdin once the CPU is parked).
    /// The fifo "stop" wrapper command banned by ADR-11 is never written
    /// here (or anywhere in this crate) — it deadlocks the session
    /// permanently. On error the session is returned still-Running (the
    /// freeze may or may not have landed — see the known limitation in
    /// ADR-11).
    pub async fn pause(mut self) -> Result<Session<Paused>, (Session<Running>, Error)> {
        if let Err(e) = self.data.freeze().await {
            return Err((self, e));
        }
        Ok(Session {
            config: self.config,
            data: self.data,
        })
    }
}

// ---- Paused ----
impl Session<Paused> {
    /// Unfreeze (ADR-11): write `c` to stdin, then confirm the fifo is
    /// serviced again with a sentinel round-trip.
    pub async fn resume(mut self) -> Result<Session<Running>, (Session<Paused>, Error)> {
        if let Err(e) = self.data.thaw().await {
            return Err((self, e));
        }
        Ok(Session {
            config: self.config,
            data: self.data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_is_stopped() {
        let s = Session::new(HatariConfig::default());
        assert_eq!(s.state_name(), "stopped");
    }

    #[tokio::test]
    async fn start_bad_binary_returns_session() {
        let config = HatariConfig {
            hatari_path: "/nonexistent/hatari".into(),
            boot_timeout_ms: 500,
            ..HatariConfig::default()
        };
        let Err((s, _e)) = Session::new(config).start().await else {
            panic!("start() must fail against a nonexistent binary")
        };
        assert_eq!(s.state_name(), "stopped");
    }
}
