//! `run_frames` — deterministic VBL frame stepping (P14 §7.3), riding the
//! ADR-15 managed run-to-breakpoint lifecycle exactly like `watch`.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::managed::{self, Fire};
use super::watch_wire::cap_output_tail;
use super::{to_mcp, HatariServer};
use crate::debug;
use crate::error::Error;
use crate::session::AnySession;
use crate::types::Addr;

pub const FRAMES_MAX: u32 = 5_000;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunFramesArgs {
    /// VBL frames to run (1..=5000).
    pub frames: u32,
    /// Wait budget, ms. Default 10000 + 40·frames (a 50 Hz frame is 20 ms
    /// at real speed; the default assumes ≤ half speed under an armed
    /// per-instruction check), capped at 600000.
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunFramesOutput {
    pub requested: u32,
    /// true iff our VBL breakpoint fired and vbl_after == vbl_before + requested.
    pub completed: bool,
    pub vbl_before: u32,
    /// Absent on timeout (machine left running; the counter keeps moving).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vbl_after: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames_run: Option<u32>,
    /// PC at the freeze (fired only) — frame-deterministic, not
    /// PC-deterministic (wherever the CPU was at the n-th VBL boundary).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    pub state: String,
}

/// "VBL > {cur + n - 1}" — decimal (debugger default base), §3.3.
pub(crate) fn vbl_condition(cur: u32, n: u32) -> String {
    format!("VBL > {}", u64::from(cur) + u64::from(n) - 1)
}

pub(crate) fn default_timeout(frames: u32) -> u64 {
    (10_000 + 40 * u64::from(frames)).min(600_000)
}

#[tool_router(router = frames_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "run_frames",
        description = "Run the machine for exactly N VBL frames (50 Hz), \
                       then freeze — deterministic frame stepping for \
                       animated/transient states (no more poll-and-race to \
                       catch a scroller). Arms a managed one-shot \
                       breakpoint on Hatari's VBL frame counter: session \
                       ends PAUSED exactly N frames later (completed=true; \
                       pc is wherever the CPU was at that frame boundary), \
                       or RUNNING on timeout with the breakpoint disarmed. \
                       Combine with watch/machine_state/screenshot for \
                       reproducible before-after comparisons."
    )]
    pub async fn run_frames(
        &self,
        Parameters(a): Parameters<RunFramesArgs>,
    ) -> Result<Json<RunFramesOutput>, McpError> {
        validate_frames(a.frames)?;
        let timeout = a.timeout_ms.unwrap_or_else(|| default_timeout(a.frames));
        let mut guard = self.session.lock().await;
        managed::ensure_paused(&mut guard).await.map_err(to_mcp)?;
        let vbl_before = current_vbl(&mut guard).await?;
        check_vbl_headroom(vbl_before, a.frames)?;
        let condition = vbl_condition(vbl_before, a.frames);
        let fire = managed::run_to_breakpoint(&mut guard, &condition, timeout)
            .await
            .map_err(to_mcp)?;
        Ok(Json(
            build_output(&mut guard, a.frames, vbl_before, fire).await?,
        ))
    }
}

// `pub(crate)`: reused by `send_key`'s `frames:` composition (P27 §3.5) —
// same 1..=FRAMES_MAX shape, no duplicated bound.
pub(crate) fn validate_frames(frames: u32) -> Result<(), McpError> {
    if (1..=FRAMES_MAX).contains(&frames) {
        Ok(())
    } else {
        Err(McpError::invalid_params(
            format!("frames must be 1..={FRAMES_MAX}"),
            None,
        ))
    }
}

/// Absurd-but-checked overflow guard (§7.3 step 3): the VBL counter is a
/// `u32`, so `cur + n - 1` must not wrap.
pub(crate) fn check_vbl_headroom(cur: u32, frames: u32) -> Result<(), McpError> {
    if u64::from(cur) + u64::from(frames) <= u64::from(u32::MAX) {
        Ok(())
    } else {
        Err(McpError::internal_error("VBL counter would overflow", None))
    }
}

pub(crate) async fn current_vbl(guard: &mut AnySession) -> Result<u32, McpError> {
    let proc = guard
        .process_mut()
        .ok_or_else(|| to_mcp(Error::Session("no live session".to_string())))?;
    debug::evaluate(proc, "VBL").await.map_err(to_mcp)
}

async fn build_output(
    guard: &mut AnySession,
    frames: u32,
    vbl_before: u32,
    fire: Fire,
) -> Result<RunFramesOutput, McpError> {
    match fire {
        Fire::TimedOut => Ok(timed_out_output(frames, vbl_before)),
        Fire::Fired { pc, output, ours } => {
            fired_output(guard, frames, vbl_before, pc, output, ours).await
        }
    }
}

fn timed_out_output(frames: u32, vbl_before: u32) -> RunFramesOutput {
    RunFramesOutput {
        requested: frames,
        completed: false,
        vbl_before,
        vbl_after: None,
        frames_run: None,
        pc: None,
        output: None,
        state: "running".to_string(),
    }
}

async fn fired_output(
    guard: &mut AnySession,
    frames: u32,
    vbl_before: u32,
    pc: u32,
    output: String,
    ours: bool,
) -> Result<RunFramesOutput, McpError> {
    let vbl_after = current_vbl(guard).await?;
    let frames_run = vbl_after.wrapping_sub(vbl_before);
    Ok(RunFramesOutput {
        requested: frames,
        completed: ours && frames_run == frames,
        vbl_before,
        vbl_after: Some(vbl_after),
        frames_run: Some(frames_run),
        pc: Some(Addr::new(pc).to_string()),
        output: Some(cap_output_tail(output)),
        state: "paused".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vbl_condition_builds() {
        assert_eq!(vbl_condition(35_936, 3), "VBL > 35938");
        assert_eq!(vbl_condition(0, 1), "VBL > 0");
    }

    #[test]
    fn default_timeout_scales_and_caps() {
        assert_eq!(default_timeout(1), 10_040);
        assert_eq!(default_timeout(5_000), 210_000);
        assert_eq!(
            default_timeout(FRAMES_MAX),
            (10_000 + 40 * u64::from(FRAMES_MAX)).min(600_000)
        );
    }

    #[test]
    fn validate_frames_rejects_zero_and_over_max() {
        assert!(validate_frames(0).is_err());
        assert!(validate_frames(FRAMES_MAX + 1).is_err());
        assert!(validate_frames(1).is_ok());
        assert!(validate_frames(FRAMES_MAX).is_ok());
    }

    #[test]
    fn vbl_headroom_rejects_overflow() {
        assert!(check_vbl_headroom(u32::MAX, 2).is_err());
        assert!(check_vbl_headroom(0, FRAMES_MAX).is_ok());
    }
}
