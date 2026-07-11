//! `profile_frame`: measure per-frame cycle cost over N whole VBL frames —
//! the frame-windowed sibling of `profile` (ADR-42, MCP_TODOS §1). The
//! measurement window is TIME (one profiler `save` per VBL boundary,
//! exploiting Hatari's reset-on-continue profiler semantics); `from`/`to`
//! demote to a pure address FILTER over which executed code counts as
//! "yours". Split from `profile_frame_wire.rs` (wire schema + pure math)
//! for the file-size ceiling — the `profile`/`profile_wire.rs` precedent.

use std::path::Path;
use std::sync::atomic::Ordering;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::frames;
use super::managed::{self, Fire};
use super::profile_frame_wire::{build_output, frame_path, validate};
use super::{to_mcp, AnySession, HatariServer};
use crate::debug;
use crate::session::HatariProcess;
use crate::types::Addr;

// Re-exported for `tests/profile_frame_live.rs` (the `analyze_profile.rs`
// line-19 precedent).
pub use super::profile_frame_wire::{ProfileFrameArgs, ProfileFrameOutput};

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = profile_frame_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "profile_frame",
        description = "Measure per-frame cycle cost over N whole VBL \
                       frames (50 Hz). The measurement window is TIME (one \
                       profiler window per frame), NOT a PC range — so \
                       whole-frame cost, bsr'd callees and interrupts are \
                       all counted and loop shape doesn't matter. Optional \
                       from/to are an ADDRESS FILTER, not an entry/exit \
                       pair: give your PRG's text extent (or one routine's \
                       extent) and range_* reports 'your code's' cycles per \
                       frame, while total_* always reports every executed \
                       address. Interpretation: a vsync-locked program \
                       burns ~160256 TOTAL cycles/frame by construction \
                       (the idle spin absorbs the rest), so total is a \
                       validity check and RANGE is the cost metric; if the \
                       vsync wait-spin lies inside [from,to), expect \
                       headroom ~0 — exclude it from the extent. \
                       fits_50hz/headroom_pct use the WORST (max-cost) \
                       frame. Reports min/avg/max plus the per-frame \
                       series; vbl_fraction/hbl_count are for the AVG \
                       frame. Leaves the session PAUSED at the last frame \
                       boundary. Raw per-frame profiler dumps live in \
                       profile_dir; worst_frame_path is the one to drill \
                       into."
    )]
    pub async fn profile_frame(
        &self,
        Parameters(a): Parameters<ProfileFrameArgs>,
    ) -> Result<Json<ProfileFrameOutput>, McpError> {
        let extent = validate(&a)?;
        let seq = self.profile_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let leg_timeout = a.timeout_ms.unwrap_or_else(|| frames::default_timeout(1));
        let mut guard = self.session.lock().await;
        run_profile_frame(&mut guard, &a, extent, leg_timeout, seq)
            .await
            .map(Json)
    }
}

async fn run_profile_frame(
    guard: &mut AnySession,
    a: &ProfileFrameArgs,
    extent: Option<(Addr, Addr)>,
    leg_timeout: u64,
    seq: u64,
) -> Result<ProfileFrameOutput, McpError> {
    managed::ensure_paused(guard).await.map_err(to_mcp)?;
    let vbl0 = frames::current_vbl(guard).await?;
    frames::check_vbl_headroom(vbl0, a.frames + 1)?;
    let dir = make_dump_dir(guard, seq).await?;

    // Alignment leg (§6): parks the session on a VBL boundary before the
    // profiler is even enabled, so window 1 below is a whole frame.
    one_frame_leg(guard, leg_timeout).await?;
    debug::profile::profile_on(live_proc(guard)?)
        .await
        .map_err(to_mcp)?;

    let (totals, ranges) = match sample_frames(guard, a.frames, leg_timeout, &dir, extent).await {
        Ok(v) => v,
        Err(e) => {
            cleanup_profile(guard).await;
            return Err(e);
        }
    };
    finish_profiling(guard).await?;
    Ok(build_output(a.frames, extent, totals, ranges, &dir))
}

async fn make_dump_dir(guard: &mut AnySession, seq: u64) -> Result<std::path::PathBuf, McpError> {
    let proc = live_proc(guard)?;
    let dir = proc
        .config()
        .work_dir
        .join(format!("profile-frame-{seq:04}"));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| to_mcp(crate::error::Error::Io(e)))?;
    Ok(dir)
}

/// Hard error on failure (§7): profiling left enabled would silently slow
/// the session, so this is not best-effort like `cleanup_profile`.
async fn finish_profiling(guard: &mut AnySession) -> Result<(), McpError> {
    debug::profile::profile_off(live_proc(guard)?)
        .await
        .map_err(|e| {
            McpError::internal_error(
                format!(
                    "{}; profiler may still be enabled; stop/start to clear",
                    to_mcp(e).message
                ),
                None,
            )
        })
}

fn live_proc(guard: &mut AnySession) -> Result<&mut HatariProcess, McpError> {
    guard
        .process_mut()
        .ok_or_else(|| McpError::internal_error("session lost its process mid-profile", None))
}

/// Run exactly one VBL frame via the managed primitive. Errors unless OUR
/// breakpoint fired AND the VBL counter advanced by exactly 1 (frame
/// isolation would otherwise be lost).
async fn one_frame_leg(guard: &mut AnySession, timeout_ms: u64) -> Result<(), McpError> {
    let cur = frames::current_vbl(guard).await?;
    let condition = frames::vbl_condition(cur, 1);
    match managed::run_to_breakpoint(guard, &condition, timeout_ms)
        .await
        .map_err(to_mcp)?
    {
        Fire::Fired { ours: true, .. } => {
            let after = frames::current_vbl(guard).await?;
            let delta = after.wrapping_sub(cur);
            if delta == 1 {
                Ok(())
            } else {
                Err(McpError::internal_error(
                    format!("VBL advanced by {delta} (expected 1) — frame isolation lost"),
                    None,
                ))
            }
        }
        Fire::Fired {
            pc, ours: false, ..
        } => Err(McpError::internal_error(
            format!("a foreign breakpoint fired first: pc=${pc:x}"),
            None,
        )),
        Fire::TimedOut => Err(McpError::internal_error(
            format!(
                "VBL boundary not reached within {timeout_ms} ms \
                 (machine left running, managed breakpoint disarmed)"
            ),
            None,
        )),
    }
}

/// Best-effort profiler disable on an error path; never masks the original
/// error (after `TimedOut` the session is Running — the attempt may fail;
/// acceptable, matching `run_profile`'s precedent).
async fn cleanup_profile(guard: &mut AnySession) {
    if let Some(proc) = guard.process_mut() {
        let _ = debug::profile::profile_off(proc).await;
    }
}

async fn sample_frames(
    guard: &mut AnySession,
    n: u32,
    leg_timeout: u64,
    dir: &Path,
    extent: Option<(Addr, Addr)>,
) -> Result<(Vec<u64>, Option<Vec<u64>>), McpError> {
    let mut totals = Vec::with_capacity(n as usize);
    let mut ranges: Option<Vec<u64>> = extent.map(|_| Vec::with_capacity(n as usize));
    for i in 1..=n {
        let (total, range) = one_sample(guard, i, n, leg_timeout, dir, extent).await?;
        totals.push(total);
        if let (Some(r), Some(v)) = (&mut ranges, range) {
            r.push(v);
        }
    }
    Ok((totals, ranges))
}

async fn one_sample(
    guard: &mut AnySession,
    i: u32,
    n: u32,
    leg_timeout: u64,
    dir: &Path,
    extent: Option<(Addr, Addr)>,
) -> Result<(u64, Option<u64>), McpError> {
    one_frame_leg(guard, leg_timeout)
        .await
        .map_err(|e| prefix(e, i, n))?;
    let path = frame_path(dir, i);
    debug::profile::profile_save(live_proc(guard)?, &path)
        .await
        .map_err(|e| prefix(to_mcp(e), i, n))?;
    parse_frame_dump(&path, extent)
        .await
        .map_err(|e| prefix(e, i, n))
}

async fn parse_frame_dump(
    path: &Path,
    extent: Option<(Addr, Addr)>,
) -> Result<(u64, Option<u64>), McpError> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| to_mcp(crate::error::Error::Io(e)))?;
    let entries = debug::profile::parse_profile_file(&contents);
    let total = debug::profile::sum_all(&entries).1;
    let range = extent.map(|(from, to)| debug::profile::sum_range(&entries, from, to).1);
    Ok((total, range))
}

fn prefix(e: McpError, i: u32, n: u32) -> McpError {
    McpError::internal_error(format!("frame {i}/{n}: {}", e.message), None)
}
