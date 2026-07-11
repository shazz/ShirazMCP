//! Wire schema + small helpers for the grouped `breakpoint`/`state` tools
//! (§8.3) — split out of `breakpoint.rs` purely to keep that file under
//! the line ceiling, same pattern as `rip_wire.rs`/`analyze_wire.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::session::HatariProcess;
use rmcp::ErrorData as McpError;

/// `breakpoint`'s own capped-output budget (smaller than `debug`'s 8 KiB —
/// this is a match-report block, not arbitrary debugger output).
const OUTPUT_CAP: usize = 2 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BreakpointArgs {
    pub action: BreakpointAction,
    /// set: Hatari condition, e.g. "pc = $12345", "VBL > 1000" (`b` syntax;
    /// the tool appends :once).
    pub condition: Option<String>,
    /// set: wait budget for the fire, ms (default 10000).
    pub timeout_ms: Option<u64>,
    /// remove: 1-based index from action=list.
    pub index: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BreakpointAction {
    Set,
    List,
    Remove,
    Clear,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BreakpointOutput {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fired: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakpoints: Option<Vec<BreakpointEntryOut>>,
    pub state: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BreakpointEntryOut {
    pub index: u32,
    pub expression: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StateArgs {
    pub action: StateAction,
    /// load: snapshot path (must be inside the session work_dir).
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StateAction {
    Save,
    Load,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SnapshotOutput {
    pub action: String,
    pub path: String,
    pub state: String,
}

pub(super) fn bare_output(action: &str, state: &str) -> BreakpointOutput {
    BreakpointOutput {
        action: action.to_string(),
        fired: None,
        pc: None,
        output: None,
        breakpoints: None,
        state: state.to_string(),
    }
}

pub(super) fn cap_output(output: String) -> String {
    if output.len() <= OUTPUT_CAP {
        return output;
    }
    let cut = output[..OUTPUT_CAP].rfind('\n').unwrap_or(OUTPUT_CAP);
    output[..cut].to_string()
}

async fn canonicalize_in_work_dir(
    proc: &HatariProcess,
    raw: &str,
) -> Result<std::path::PathBuf, McpError> {
    let work_dir = tokio::fs::canonicalize(&proc.config().work_dir)
        .await
        .map_err(|e| super::to_mcp(Error::Io(e)))?;
    let path = tokio::fs::canonicalize(raw)
        .await
        .map_err(|_| McpError::invalid_params(format!("snapshot not found: {raw}"), None))?;
    if !path.starts_with(&work_dir) {
        return Err(McpError::invalid_params(
            "snapshot path must be inside the session work dir",
            None,
        ));
    }
    Ok(path)
}

/// `state(action=save)`'s full body once `proc`/`name` are in hand: fresh
/// name + delete first (`File_QueryOverwrite` confirm can never trigger),
/// `statesave`, confirm the "saved" line.
pub(super) async fn save_snapshot(
    proc: &mut HatariProcess,
    seq: u64,
    state: &str,
) -> Result<SnapshotOutput, McpError> {
    let path = proc.config().work_dir.join(format!("state-{seq:04}.snap"));
    let _ = tokio::fs::remove_file(&path).await;
    let out = proc
        .debug(&format!("statesave {}", path.display()))
        .await
        .map_err(super::to_mcp)?;
    if !out.contains("Memory state file saved:") {
        return Err(super::to_mcp(Error::Parse {
            context: "statesave".to_string(),
            input: out,
        }));
    }
    Ok(SnapshotOutput {
        action: "save".to_string(),
        path: path.display().to_string(),
        state: state.to_string(),
    })
}

/// `state(action=load)`'s full body once `proc` is in hand: canonicalize +
/// contain to `work_dir`, `stateload`, wait for the restore confirmation.
pub(super) async fn load_snapshot(
    proc: &mut HatariProcess,
    raw: &str,
) -> Result<SnapshotOutput, McpError> {
    let path = canonicalize_in_work_dir(proc, raw).await?;
    let out = proc
        .debug(&format!("stateload {}", path.display()))
        .await
        .map_err(super::to_mcp)?;
    ensure_restored(proc, out).await?;
    Ok(SnapshotOutput {
        action: "load".to_string(),
        path: path.display().to_string(),
        state: "running".to_string(),
    })
}

/// The restore may complete AFTER the sentinel round-trip (§3.6); if its
/// confirmation isn't already in `out`, wait for it. On timeout, a single
/// 0-budget check for the pinned `"Unable to restore"` failure line (the
/// stateload-specific error text) upgrades the message when present.
async fn ensure_restored(proc: &mut HatariProcess, out: String) -> Result<(), McpError> {
    if out.contains("Memory state file restored:") {
        return Ok(());
    }
    if proc
        .wait_for_log("Memory state file restored:", 5_000)
        .await
        .is_ok()
    {
        return Ok(());
    }
    if proc.wait_for_log("Unable to restore", 0).await.is_ok() {
        return Err(McpError::internal_error(
            "Unable to restore Hatari memory state",
            None,
        ));
    }
    Err(super::to_mcp(Error::Timeout {
        what: "stateload restore confirmation".to_string(),
        waited_ms: 5_000,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_output_short_passthrough() {
        assert_eq!(cap_output("hi".to_string()), "hi");
    }

    #[test]
    fn cap_output_cuts_at_line_boundary() {
        let long = "a".repeat(OUTPUT_CAP) + "\nrest";
        let capped = cap_output(long.clone());
        assert!(capped.len() <= OUTPUT_CAP);
        assert!(!capped.contains("rest"));
    }
}
