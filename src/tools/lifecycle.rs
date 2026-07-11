use std::path::PathBuf;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{to_mcp, wrong_state, HatariServer};
use crate::session::{AnySession, HatariConfig, HatariProcess, Session, Stopped};

/// `start` — input. Every field optional; `None` → `HatariConfig::default()`
/// value.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartArgs {
    /// Path to the hatari binary.
    pub hatari_path: Option<String>,
    /// TOS ROM image path.
    pub tos_path: Option<String>,
    /// Directory to mount as GEMDOS drive C: (TOS auto-runs its AUTO/*.PRG).
    pub harddrive: Option<String>,
    /// "st" | "ste" | "megaste" | "tt" | "falcon".
    pub machine: Option<String>,
    /// RAM size in MiB (1-14).
    pub memsize_mb: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StartOutput {
    pub state: String,
    pub pid: Option<u32>,
    pub fifo: String,
    pub log: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StateOutput {
    pub state: String,
}

fn overlay_config(base: HatariConfig, args: StartArgs) -> HatariConfig {
    HatariConfig {
        hatari_path: args
            .hatari_path
            .map(PathBuf::from)
            .unwrap_or(base.hatari_path),
        tos_path: args.tos_path.map(PathBuf::from).unwrap_or(base.tos_path),
        harddrive: args.harddrive.map(PathBuf::from).or(base.harddrive),
        machine: args.machine.unwrap_or(base.machine),
        memsize_mb: args.memsize_mb.unwrap_or(base.memsize_mb),
        ..base
    }
}

fn start_output(proc: &HatariProcess) -> StartOutput {
    StartOutput {
        state: "running".to_string(),
        pid: proc.pid(),
        fifo: proc.config().fifo_path().display().to_string(),
        log: proc.config().log_path().display().to_string(),
    }
}

// `vis = "pub(crate)"`: the generated router fn defaults to private, but
// `#[tool_handler]` in mod.rs (a sibling module) needs to call it.
#[tool_router(router = lifecycle_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "start",
        description = "Boot Hatari (Atari ST emulator). Returns once the \
                       debugger control plane answers (sentinel-gated; no \
                       fixed sleeps). Fails if a session is already running \
                       or paused."
    )]
    pub async fn start(
        &self,
        Parameters(args): Parameters<StartArgs>,
    ) -> Result<Json<StartOutput>, McpError> {
        let mut guard = self.session.lock().await;
        let config = overlay_config(guard.config().clone(), args);
        let placeholder = AnySession::new(config.clone());
        match std::mem::replace(&mut *guard, placeholder) {
            // WHY: `s` (the session `mem::replace` pulled out) still carries
            // whatever config it was constructed with — starting *it*
            // silently drops every per-call override (`overlay_config`'s
            // `config` above never reaches `HatariProcess::spawn`). Stopped
            // sessions own no live data (`Data = ()`), so discarding `s`
            // and starting a fresh `Session<Stopped>` built from the
            // overlaid `config` is free and correct.
            AnySession::Stopped(_) => match Session::<Stopped>::new(config).start().await {
                Ok(running) => {
                    let out = start_output(running.process());
                    *guard = AnySession::Running(running);
                    Ok(Json(out))
                }
                Err((s, e)) => {
                    *guard = AnySession::Stopped(s);
                    Err(to_mcp(e))
                }
            },
            other => {
                let name = other.state_name();
                *guard = other;
                Err(wrong_state(name, "stopped"))
            }
        }
    }

    #[tool(
        name = "stop",
        description = "Stop the emulator: graceful quit, then kill. Always \
                       reaches the stopped state; no orphan hatari process \
                       remains."
    )]
    pub async fn stop(&self) -> Result<Json<StateOutput>, McpError> {
        let mut guard = self.session.lock().await;
        let config = guard.config().clone();
        let taken = std::mem::replace(&mut *guard, AnySession::new(config));
        *guard = match taken {
            AnySession::Stopped(s) => AnySession::Stopped(s),
            AnySession::Running(s) => AnySession::Stopped(s.stop().await),
            AnySession::Paused(s) => AnySession::Stopped(s.stop().await),
        };
        // P26 §7.5: Hatari's symbol table dies with the process — a stale
        // listing mirror must not answer `symbols(action="list")`.
        self.clear_symbol_store();
        Ok(Json(StateOutput {
            state: "stopped".to_string(),
        }))
    }

    #[tool(
        name = "pause",
        description = "Freeze emulation via a debugger break (freezer-\
                       cartridge 'freeze then rip'). Registers, memory, \
                       screenshot and rip all work while frozen; the \
                       machine state does not advance."
    )]
    pub async fn pause(&self) -> Result<Json<StateOutput>, McpError> {
        let mut guard = self.session.lock().await;
        let config = guard.config().clone();
        match std::mem::replace(&mut *guard, AnySession::new(config)) {
            AnySession::Running(s) => match s.pause().await {
                Ok(p) => {
                    *guard = AnySession::Paused(p);
                    Ok(Json(StateOutput {
                        state: "paused".to_string(),
                    }))
                }
                Err((s, e)) => {
                    *guard = AnySession::Running(s);
                    Err(to_mcp(e))
                }
            },
            other => {
                let name = other.state_name();
                *guard = other;
                Err(wrong_state(name, "running"))
            }
        }
    }

    #[tool(name = "resume", description = "Unfreeze a paused emulator.")]
    pub async fn resume(&self) -> Result<Json<StateOutput>, McpError> {
        let mut guard = self.session.lock().await;
        let config = guard.config().clone();
        match std::mem::replace(&mut *guard, AnySession::new(config)) {
            AnySession::Paused(s) => match s.resume().await {
                Ok(r) => {
                    *guard = AnySession::Running(r);
                    Ok(Json(StateOutput {
                        state: "running".to_string(),
                    }))
                }
                Err((s, e)) => {
                    *guard = AnySession::Paused(s);
                    Err(to_mcp(e))
                }
            },
            other => {
                let name = other.state_name();
                *guard = other;
                Err(wrong_state(name, "paused"))
            }
        }
    }

    #[tool(
        name = "status",
        description = "Report emulator session status: current state \
                       (stopped|running|paused), configured Hatari binary \
                       path, and server version. Also probes a live child: \
                       if Hatari exited without the session noticing (the \
                       fifo/stdin FSM has no other way to detect this), \
                       resets to stopped and reports the exit in `note` — \
                       the only tool that self-heals a defunct session."
    )]
    pub async fn status(&self) -> Result<Json<StatusOutput>, McpError> {
        let mut guard = self.session.lock().await;
        let note = reap_if_dead(&mut guard);
        Ok(Json(StatusOutput {
            state: guard.state_name().to_string(),
            hatari_path: guard.config().hatari_path.display().to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            note,
        }))
    }
}

/// If the FSM says Running/Paused but the child has already exited
/// (P27/ADR-43 §4.5 — EPIPE-detecting tools only surface this on their next
/// call; `status` is the one place that probes proactively), reset the
/// session to Stopped and say why. `None` for a Stopped session (nothing to
/// probe) or a live, still-running child — the common, silent case.
///
/// `Implementation note` (P27 §4.6): the dead-child arm needs a real exited
/// `Child`, which only `HatariProcess::spawn` produces — not unit-testable
/// without a process; covered by the `#[ignore]`d live test instead.
fn reap_if_dead(guard: &mut AnySession) -> Option<String> {
    let old_state = guard.state_name();
    let config = guard.config().clone();
    let status = guard.process_mut()?.exit_status()?;
    *guard = AnySession::new(config);
    Some(format!(
        "hatari exited ({status}) while session was {old_state}; reset to stopped"
    ))
}

/// Wire shape of `status`. Field names are the JSON contract.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StatusOutput {
    pub state: String,
    pub hatari_path: String,
    pub version: String,
    /// Set only when `status` had to self-heal a defunct session (P27/ADR-43).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use rmcp::handler::server::wrapper::Parameters;

    use super::*;
    use crate::session::HatariConfig;

    fn no_override_args() -> StartArgs {
        StartArgs {
            hatari_path: None,
            tos_path: None,
            harddrive: None,
            machine: None,
            memsize_mb: None,
        }
    }

    /// Regression for the bug this fix addresses: `start` computed an
    /// overlaid config but then booted the session it pulled out of the
    /// `Mutex` via `mem::replace` — which still carried whatever config it
    /// was constructed with — silently discarding every per-call override
    /// (`harddrive`, `machine`, `memsize_mb`, ...). `hatari_path` is the one
    /// override `status` exposes, so drive the bug through it: give the
    /// server a base config with one (nonexistent) binary path, then
    /// override it with a *different* nonexistent path on `start`. The
    /// spawn fails either way (ENOENT, no real Hatari/timeout needed), but
    /// `status` after the failure must report the overlaid path, not the
    /// base one.
    #[tokio::test]
    async fn start_overlay_reaches_spawn_even_on_failure() {
        let base = HatariConfig {
            hatari_path: "/nonexistent/base-hatari".into(),
            work_dir: "/tmp/shiraz-mcp-lifecycle-test-overlay".into(),
            ..HatariConfig::default()
        };
        let server = HatariServer::new(base);

        let args = StartArgs {
            hatari_path: Some("/nonexistent/overlaid-hatari".to_string()),
            ..no_override_args()
        };
        let err = server
            .start(Parameters(args))
            .await
            .err()
            .expect("spawning a nonexistent binary must fail");
        assert!(
            err.message.contains("overlaid-hatari")
                || err.message.to_lowercase().contains("no such file"),
            "expected spawn error to reference the overlaid binary path, got: {}",
            err.message
        );

        let status = server.status().await.expect("status after failed start");
        assert_eq!(
            status.0.hatari_path, "/nonexistent/overlaid-hatari",
            "status must report the overlaid config, not the stale one the \
             Stopped session happened to be constructed with"
        );
    }

    /// Happy-ish path complement: a `start` with no overrides at all keeps
    /// reporting the base config (guards against the fix over-correcting
    /// into always discarding the base).
    #[tokio::test]
    async fn start_without_overrides_keeps_base_config() {
        let base = HatariConfig {
            hatari_path: "/nonexistent/only-hatari".into(),
            work_dir: "/tmp/shiraz-mcp-lifecycle-test-base".into(),
            ..HatariConfig::default()
        };
        let server = HatariServer::new(base);

        let _ = server.start(Parameters(no_override_args())).await;
        let status = server.status().await.expect("status after failed start");
        assert_eq!(status.0.hatari_path, "/nonexistent/only-hatari");
    }

    /// P27/ADR-43 §4.6: `status` on a genuinely `Stopped` session has
    /// nothing to probe — `note` stays `None`, and existing callers that
    /// never look at the new field keep passing unmodified.
    #[tokio::test]
    async fn status_on_stopped_session_has_no_note() {
        let server = HatariServer::new(HatariConfig::default());
        let status = server.status().await.expect("status on a stopped session");
        assert_eq!(status.0.state, "stopped");
        assert!(status.0.note.is_none());
    }

    /// `reap_if_dead` itself, in isolation: a `Stopped` `AnySession` has no
    /// live process to probe (`process_mut()` returns `None`), so the `?`
    /// short-circuits to `None` before ever touching `exit_status`. The
    /// dead-child arm needs a real exited `Child` (only `HatariProcess::spawn`
    /// produces one) and is covered by the `#[ignore]`d live test instead
    /// (P27 §4.6 `Implementation note`).
    #[test]
    fn reap_if_dead_is_noop_for_stopped_session() {
        let mut guard = AnySession::new(HatariConfig::default());
        assert_eq!(reap_if_dead(&mut guard), None);
        assert_eq!(guard.state_name(), "stopped");
    }
}
