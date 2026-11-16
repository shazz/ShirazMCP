//! `profile_raw`: ONE profiler window over N whole VBL frames, summarised
//! (totals vs the expected 160256 cycles/frame, ST-RAM/TOS/cartridge split,
//! named ranges with entry-hit counts, top-N hottest addresses) so agents
//! stop hand-scraping the dump (ADR-47). Reuses `profile_frame`'s leg and
//! profiler on/off helpers; aggregation is pure in `profile_raw_wire.rs`.

use std::path::Path;
use std::sync::atomic::Ordering;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::frames;
use super::managed;
use super::profile_frame::{cleanup_profile, finish_profiling, frames_leg, live_proc};
use super::profile_raw_wire::{build_output, check_range, validate, NamedRange};
use super::resolve::resolve_addr;
use super::{to_mcp, AnySession, HatariServer};
use crate::debug;

pub use super::profile_raw_wire::{ProfileRawArgs, ProfileRawOutput, RangeArg};

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = profile_raw_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "profile_raw",
        description = "Profile N whole VBL frames (50 Hz) in ONE profiler \
                       window and summarise the dump: total cycles and \
                       instructions vs expected (N × 160256) with a \
                       coverage ratio, a region split (st_ram / rom_tos \
                       $E00000-$E40000 / cartridge $FA0000-$FC0000 / \
                       other), optional named ranges [{name, from, to}] \
                       with cycles, % and entry_hits (executions of the \
                       instruction at `from`), and the top-N hottest \
                       addresses (default 40). Use profile_frame instead \
                       for a per-frame series. Leaves the session PAUSED \
                       at a frame boundary; the raw dump is at dump_path."
    )]
    pub async fn profile_raw(
        &self,
        Parameters(a): Parameters<ProfileRawArgs>,
    ) -> Result<Json<ProfileRawOutput>, McpError> {
        let top = validate(&a)?;
        let seq = self.profile_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let timeout = a
            .timeout_ms
            .unwrap_or_else(|| frames::default_timeout(a.frames));
        let mut guard = self.session.lock().await;
        managed::ensure_paused(&mut guard).await.map_err(to_mcp)?;
        let ranges = resolve_ranges(&mut guard, a.ranges.as_deref().unwrap_or_default()).await?;
        let path = live_proc(&mut guard)?
            .config()
            .work_dir
            .join(format!("profile-raw-{seq:04}.txt"));
        run_window(&mut guard, a.frames, timeout, &path).await?;
        let entries = parse_dump(&path).await?;
        build_output(a.frames, &entries, &ranges, top, &path).map(Json)
    }
}

async fn resolve_ranges(
    guard: &mut AnySession,
    ranges: &[RangeArg],
) -> Result<Vec<NamedRange>, McpError> {
    let proc = live_proc(guard)?;
    let mut out = Vec::with_capacity(ranges.len());
    for r in ranges {
        let named = NamedRange {
            name: r.name.clone(),
            from: resolve_addr(proc, &r.from).await?,
            to: resolve_addr(proc, &r.to).await?,
        };
        check_range(&named)?;
        out.push(named);
    }
    Ok(out)
}

/// Align to a VBL boundary (unprofiled), profiler on, ONE N-frame leg,
/// save, profiler off. The profiler is disabled on every error path.
async fn run_window(
    guard: &mut AnySession,
    n: u32,
    timeout: u64,
    path: &Path,
) -> Result<(), McpError> {
    let vbl0 = frames::current_vbl(guard).await?;
    frames::check_vbl_headroom(vbl0, n + 1)?;
    frames_leg(guard, 1, frames::default_timeout(1)).await?;
    debug::profile::profile_on(live_proc(guard)?)
        .await
        .map_err(to_mcp)?;
    let saved = match frames_leg(guard, n, timeout).await {
        Ok(_) => debug::profile::profile_save(live_proc(guard)?, path)
            .await
            .map_err(to_mcp),
        Err(e) => Err(e),
    };
    if let Err(e) = saved {
        cleanup_profile(guard).await;
        return Err(e);
    }
    finish_profiling(guard).await
}

async fn parse_dump(path: &Path) -> Result<Vec<debug::profile::ProfiledAddr>, McpError> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| to_mcp(crate::error::Error::Io(e)))?;
    Ok(debug::profile::parse_profile_file(&contents))
}
