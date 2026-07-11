//! `breakpoint` (managed one-shot run-to-break: set/list/remove/clear) and
//! `state` (save/load emulator snapshots) — grouped tools (§8.3). Wire
//! schema + small helpers live in `breakpoint_wire.rs` (line-ceiling split).

use std::sync::atomic::Ordering;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::breakpoint_wire::{bare_output, cap_output, load_snapshot, save_snapshot};
use super::managed::{self, Fire};
use super::{to_mcp, wrong_state, HatariServer};
use crate::debug::breakpoint as bp;
use crate::error::Error;
use crate::types::Addr;

/// Re-exported for `tests/analyze_live.rs` (see tools/analyze.rs's note).
pub use super::breakpoint_wire::{
    BreakpointAction, BreakpointArgs, BreakpointEntryOut, BreakpointOutput, SnapshotOutput,
    StateAction, StateArgs,
};
// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = breakpoint_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "breakpoint",
        description = "Managed breakpoints. action=set arms a ONE-SHOT \
                       Hatari condition (b syntax) and runs the machine \
                       until it fires or timeout_ms elapses: fired ⇒ \
                       session ends PAUSED at the breakpoint (inspect, \
                       then resume); timeout ⇒ the breakpoint is disarmed \
                       and the session stays RUNNING — nothing is ever \
                       left armed behind the session's back. \
                       list/remove/clear manage Hatari's breakpoint table \
                       (clear also cleans up breakpoints armed via the \
                       raw debug tool). Never arm breakpoints via \
                       debug(cmd) — a fire outside this tool freezes the \
                       machine invisibly (recover with pause). Conditions \
                       accept symbol names once a map is loaded: pc = \
                       BlitSphere (symbols tool)."
    )]
    pub async fn breakpoint(
        &self,
        Parameters(a): Parameters<BreakpointArgs>,
    ) -> Result<Json<BreakpointOutput>, McpError> {
        match a.action {
            BreakpointAction::Set => self.breakpoint_set(a).await,
            BreakpointAction::List => self.breakpoint_list().await,
            BreakpointAction::Remove => self.breakpoint_remove(a).await,
            BreakpointAction::Clear => self.breakpoint_clear().await,
        }
        .map(Json)
    }

    #[tool(
        name = "state",
        description = "Emulator state snapshots (Hatari memory-state \
                       format, version-locked to this Hatari build). \
                       action=save writes work_dir/state-NNNN.snap (works \
                       running or paused) and returns the path. \
                       action=load restores a snapshot path from inside \
                       the work dir — session must be RUNNING; restoring \
                       clears all breakpoints and profiler data."
    )]
    pub async fn state(
        &self,
        Parameters(a): Parameters<StateArgs>,
    ) -> Result<Json<SnapshotOutput>, McpError> {
        match a.action {
            StateAction::Save => self.state_save().await,
            StateAction::Load => self.state_load(a).await,
        }
        .map(Json)
    }
}

/// P26 §8.3: arming fails with `Error::Parse` whose line contains Hatari's
/// `breakcond.c` "invalid variable/register/symbol name" text when the
/// condition names an unresolved symbol — point at the symbols tool
/// instead of a generic internal error. Everything else passes through
/// `to_mcp` unchanged (session errors, the timeout/disarm race, …).
fn arm_hint(e: Error) -> McpError {
    if let Error::Parse { input, .. } = &e {
        if input.contains("invalid variable/register/symbol name") {
            return McpError::invalid_params(
                format!(
                    "{input} — if this is a label, load the symbol map first: \
                     symbols(action=\"load\")"
                ),
                None,
            );
        }
    }
    to_mcp(e)
}

fn set_output(fire: Fire) -> BreakpointOutput {
    let (fired, pc, output) = match fire {
        Fire::Fired {
            pc,
            output,
            ours: _,
        } => (
            true,
            Some(Addr::new(pc).to_string()),
            Some(cap_output(output)),
        ),
        Fire::TimedOut => (false, None, None),
    };
    BreakpointOutput {
        action: "set".to_string(),
        fired: Some(fired),
        pc,
        output,
        breakpoints: None,
        state: if fired { "paused" } else { "running" }.to_string(),
    }
}

impl HatariServer {
    async fn breakpoint_set(&self, a: BreakpointArgs) -> Result<BreakpointOutput, McpError> {
        let condition = a
            .condition
            .ok_or_else(|| McpError::invalid_params("set requires condition", None))?;
        let timeout_ms = a.timeout_ms.unwrap_or(10_000);
        let mut guard = self.session.lock().await;
        let fire = managed::run_to_breakpoint(&mut guard, &condition, timeout_ms)
            .await
            .map_err(arm_hint)?;
        Ok(set_output(fire))
    }

    async fn breakpoint_list(&self) -> Result<BreakpointOutput, McpError> {
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        let entries = bp::list(proc).await.map_err(to_mcp)?;
        Ok(BreakpointOutput {
            action: "list".to_string(),
            fired: None,
            pc: None,
            output: None,
            breakpoints: Some(
                entries
                    .into_iter()
                    .map(|e| BreakpointEntryOut {
                        index: e.index,
                        expression: e.expression,
                    })
                    .collect(),
            ),
            state: name.to_string(),
        })
    }

    async fn breakpoint_remove(&self, a: BreakpointArgs) -> Result<BreakpointOutput, McpError> {
        let index = a
            .index
            .ok_or_else(|| McpError::invalid_params("remove requires index", None))?;
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        bp::remove(proc, index).await.map_err(to_mcp)?;
        Ok(bare_output("remove", name))
    }

    async fn breakpoint_clear(&self) -> Result<BreakpointOutput, McpError> {
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        bp::clear(proc).await.map_err(to_mcp)?;
        Ok(bare_output("clear", name))
    }

    async fn state_save(&self) -> Result<SnapshotOutput, McpError> {
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        let n = self.state_seq.fetch_add(1, Ordering::Relaxed) + 1;
        save_snapshot(proc, n, name).await
    }

    async fn state_load(&self, a: StateArgs) -> Result<SnapshotOutput, McpError> {
        let raw = a
            .path
            .ok_or_else(|| McpError::invalid_params("load requires path", None))?;
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        if name != "running" {
            return Err(wrong_state(name, "running"));
        }
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running"))?;
        load_snapshot(proc, &raw).await
    }
}

#[cfg(test)]
mod arm_hint_tests {
    use super::*;

    #[test]
    fn unresolved_symbol_gets_the_load_hint() {
        let e = Error::Parse {
            context: "breakpoint add".to_string(),
            input: "ERROR: invalid variable/register/symbol name 'BlitSphere'".to_string(),
        };
        let mcp = arm_hint(e);
        assert!(mcp.message.contains("symbols(action=\"load\")"));
        assert!(mcp.message.contains("BlitSphere"));
    }

    #[test]
    fn other_parse_errors_pass_through_unchanged() {
        let e = Error::Parse {
            context: "breakpoint add".to_string(),
            input: "ERROR: syntax error".to_string(),
        };
        let mcp = arm_hint(e);
        assert!(!mcp.message.contains("symbols(action"));
    }

    #[test]
    fn non_parse_errors_pass_through() {
        let e = Error::Session("no live session".to_string());
        let mcp = arm_hint(e);
        assert!(mcp.message.contains("no live session"));
    }
}
