//! `profile`: measured (Hatari profiler) + static cycle cost between two
//! PCs, composed from two managed run-to-breakpoint legs (§8.1/ADR-15).
//! Split from `analyze.rs` (same feature group) to stay under the
//! file-size ceiling.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::managed::{self, Fire};
use super::mem::scan::parse_addr;
use super::profile_wire::{build_output, PROFILE_LEG_TIMEOUT_MS, PROFILE_MAX_RANGE};
use super::{to_mcp, AnySession, HatariServer};
use crate::analyze::{self, cycles};
use crate::debug;
use crate::session::{memory, HatariProcess};
use crate::types::Addr;

// Re-exported for `tests/analyze_live.rs` (see analyze.rs's re-export note).
pub use super::profile_wire::{ProfileArgs, ProfileOutput, RegionArg};

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = profile_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "profile",
        description = "Measure a routine: runs the machine to pc=from, \
                       enables Hatari's profiler, runs to pc=to, and \
                       reports measured cycles (ground truth incl. \
                       emulated bus contention, summed over addresses \
                       inside [from,to) — interrupt time outside the \
                       range excluded) alongside a static straight-line \
                       estimate given two ways: contention-BLIND (lower \
                       bound) and with the STE Shifter bus-contention \
                       model applied to its memory-access cycles for \
                       `region` (visible ×1.5 — default, worst case; \
                       vblank ×1; mixed = scanline-weighted). Leaves the \
                       session PAUSED at `to`. vbl_fraction is against the \
                       160256-cycle 50Hz frame. The raw profiler dump is \
                       saved to profile_path."
    )]
    pub async fn profile(
        &self,
        Parameters(a): Parameters<ProfileArgs>,
    ) -> Result<Json<ProfileOutput>, McpError> {
        let from = parse_addr(&a.from)?;
        let to = parse_addr(&a.to)?;
        validate_range(from, to)?;
        let timeout = a.timeout_ms.unwrap_or(PROFILE_LEG_TIMEOUT_MS);
        let region = a.region.unwrap_or(RegionArg::Visible);
        let n = self
            .profile_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let mut guard = self.session.lock().await;
        run_profile(&mut guard, from, to, timeout, region, n)
            .await
            .map(Json)
    }
}

fn validate_range(from: Addr, to: Addr) -> Result<(), McpError> {
    if to.raw() <= from.raw() {
        return Err(McpError::invalid_params(
            "to must be greater than from",
            None,
        ));
    }
    if to.raw() - from.raw() > PROFILE_MAX_RANGE {
        let msg = format!("to - from exceeds {PROFILE_MAX_RANGE}");
        return Err(McpError::invalid_params(msg, None));
    }
    Ok(())
}

async fn run_profile(
    guard: &mut AnySession,
    from: Addr,
    to: Addr,
    timeout_ms: u64,
    region: RegionArg,
    seq: u64,
) -> Result<ProfileOutput, McpError> {
    run_to_pc(guard, from, timeout_ms).await?;
    let proc = live_proc(guard)?;
    debug::profile::profile_on(proc).await.map_err(to_mcp)?;

    if let Err(e) = run_to_pc(guard, to, timeout_ms).await {
        if let Some(proc) = guard.process_mut() {
            let _ = debug::profile::profile_off(proc).await; // best-effort
        }
        return Err(e);
    }
    finish_profile(live_proc(guard)?, from, to, region, seq).await
}

fn live_proc(guard: &mut AnySession) -> Result<&mut HatariProcess, McpError> {
    guard
        .process_mut()
        .ok_or_else(|| McpError::internal_error("session lost its process mid-profile", None))
}

/// Run to `pc == want` via the managed primitive; maps `TimedOut` to the
/// §8.7 `internal_error` and verifies a foreign breakpoint didn't win.
async fn run_to_pc(guard: &mut AnySession, want: Addr, timeout_ms: u64) -> Result<(), McpError> {
    let condition = format!("pc = ${:x}", want.raw());
    match managed::run_to_breakpoint(guard, &condition, timeout_ms)
        .await
        .map_err(to_mcp)?
    {
        Fire::Fired { pc, .. } if pc == want.raw() => Ok(()),
        Fire::Fired { pc, .. } => Err(McpError::internal_error(
            format!("a foreign breakpoint fired first: pc=${pc:x}, wanted {want}"),
            None,
        )),
        Fire::TimedOut => Err(McpError::internal_error(
            format!("never reached {want} within {timeout_ms} ms"),
            None,
        )),
    }
}

async fn finish_profile(
    proc: &mut HatariProcess,
    from: Addr,
    to: Addr,
    region: RegionArg,
    seq: u64,
) -> Result<ProfileOutput, McpError> {
    let path = proc.config().work_dir.join(format!("profile-{seq:04}.txt"));
    debug::profile::profile_save(proc, &path)
        .await
        .map_err(to_mcp)?;
    debug::profile::profile_off(proc).await.map_err(to_mcp)?;

    let (instructions, measured_cycles) = measured_range(&path, from, to).await?;
    let estimate = static_estimate(proc, from, to).await?;
    Ok(build_output(
        from,
        to,
        instructions,
        measured_cycles,
        estimate,
        region,
        path,
    ))
}

/// Hatari-measured (instructions, cycles) summed over `[from, to)` from the
/// just-written `profile save` file (ADR-5: bulk data stays on disk).
async fn measured_range(
    path: &std::path::Path,
    from: Addr,
    to: Addr,
) -> Result<(u64, u64), McpError> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| to_mcp(crate::error::Error::Io(e)))?;
    let entries = debug::profile::parse_profile_file(&contents);
    Ok(debug::profile::sum_range(&entries, from, to))
}

/// Static, contention-blind straight-line estimate for one pass `from..to`.
async fn static_estimate(
    proc: &mut HatariProcess,
    from: Addr,
    to: Addr,
) -> Result<cycles::CycleEstimate, McpError> {
    let range = (to.raw() - from.raw()) as usize;
    let mem = memory::acquire_range(proc, from, range)
        .await
        .map_err(to_mcp)?;
    let insns = analyze::disasm(&mem, from, range / 2).map_err(to_mcp)?;
    let taken: Vec<_> = insns
        .into_iter()
        .take_while(|i| i.addr.raw() < to.raw())
        .collect();
    Ok(cycles::estimate(&taken))
}
