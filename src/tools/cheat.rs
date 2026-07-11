//! `cheat_scan` — iterative memory-change search ("cheat search", P15). The
//! Action-Replay way to find a variable (lives, timer, scroll cursor) with
//! NO disassembly: `new` snapshots + seeds candidates, `filter` re-snapshots
//! and narrows against a predicate, `list`/`reset` are pure over the store.
//! Never changes session state (§3.6) — the agent drives run intervals via
//! resume/pause or `run_frames` (P14). Choreography only; the diff/filter
//! engine (`analyze::cheatscan`) and the wire helpers (`cheat_wire`,
//! `cheat_out`, ADR-29) are pure and emulator-free.

use std::sync::atomic::Ordering;
use std::sync::PoisonError;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::cheat_out::{page_candidates, page_output, SAMPLE_DEFAULT, SAMPLE_MAX};
use super::cheat_wire::{
    cheat_region, geometry_matches, parse_action, parse_width, predicate, seed_filter,
    validate_params, Action, CheatStore,
};
use super::{to_mcp, wrong_state, HatariServer};
use crate::analyze;
use crate::session::memory;
use crate::types::MemImage;

// Re-exported for the live test (tests/cheat_live.rs), the mem/mod.rs
// ReadMemoryArgs precedent.
pub use super::cheat_out::{CandidateOut, CheatScanOutput};
pub use super::cheat_wire::CheatScanArgs;

#[tool_router(router = cheat_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "cheat_scan",
        description = "Iterative memory-change search over ST RAM — the Action-Replay way to find a variable (lives, \
timer, scroll cursor) with NO disassembly. action=new: snapshot RAM and seed candidates (every \
byte/word/long start — words/longs even-aligned — optionally region-bounded with start/end, or \
pre-narrowed with predicate eq/ne + value). Then let the machine run (run_frames, or \
resume+pause) and action=filter with a predicate vs the previous snapshot: changed | unchanged \
| increased | decreased | eq | ne | delta (value; delta is signed and exact — a byte wrapping \
$FF→$00 is delta -255). Repeat until few remain; action=list pages survivors (address + value \
from the last snapshot) — hand them to read_memory/watch/write_memory. Comparisons are \
big-endian unsigned; width (1/2/4, default 2) is fixed at action=new. Returns count + a capped \
sample, never a dump. Snapshots work Running or Paused; pause (or run_frames, which ends \
paused) for frame-exact narrowing. list/reset never touch the emulator. One search at a time; \
new replaces it, reset discards it."
    )]
    pub async fn cheat_scan(
        &self,
        Parameters(a): Parameters<CheatScanArgs>,
    ) -> Result<Json<CheatScanOutput>, McpError> {
        match parse_action(&a.action).map_err(ip)? {
            Action::New => self.cheat_new(a).await,
            Action::Filter => self.cheat_filter(a).await,
            Action::List => self.cheat_list(&a),
            Action::Reset => self.cheat_reset(&a),
        }
    }
}

impl HatariServer {
    async fn cheat_new(&self, a: CheatScanArgs) -> Result<Json<CheatScanOutput>, McpError> {
        validate_params(&a, Action::New).map_err(ip)?;
        let width = parse_width(a.width).map_err(ip)?;
        let sf = seed_filter(a.predicate.as_deref(), a.value.as_deref(), width).map_err(ip)?;
        let limit = a.limit.unwrap_or(SAMPLE_DEFAULT).min(SAMPLE_MAX);
        let mem = self.acquire_mem().await?;
        let (start_off, end_off) = cheat_region(&mem, a.start.as_deref(), a.end.as_deref())?;
        let search_id = self.cheat_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let candidates = analyze::seed(&mem, width, sf, start_off, end_off);
        let store = CheatStore {
            search_id,
            width,
            step: 0,
            candidates,
            snapshot: mem,
        };
        let (sample, has_more) =
            page_candidates(&store.candidates, &store.snapshot, width, 0, limit);
        let out = page_output("new", &store, 0, sample, has_more);
        self.store_cheat(store);
        Ok(Json(out))
    }

    async fn cheat_filter(&self, a: CheatScanArgs) -> Result<Json<CheatScanOutput>, McpError> {
        validate_params(&a, Action::Filter).map_err(ip)?;
        let limit = a.limit.unwrap_or(SAMPLE_DEFAULT).min(SAMPLE_MAX);
        let new_mem = self.acquire_mem().await?;
        self.apply_filter(&a, new_mem, limit)
    }

    /// The narrowing step once a fresh snapshot is in hand: lock `cheat`,
    /// diff against the stored snapshot, replace it, page the survivors.
    fn apply_filter(
        &self,
        a: &CheatScanArgs,
        new_mem: MemImage,
        limit: u32,
    ) -> Result<Json<CheatScanOutput>, McpError> {
        let mut guard = self.cheat.lock().unwrap_or_else(PoisonError::into_inner);
        let store = guard.as_mut().ok_or_else(no_search)?;
        if !geometry_matches(&store.snapshot, &new_mem) {
            return Err(bad_geometry());
        }
        let pred_name = a.predicate.as_deref().ok_or_else(filter_needs_predicate)?;
        let pred = predicate(pred_name, a.value.as_deref(), store.width).map_err(ip)?;
        let prev_count = store.candidates.count();
        analyze::filter(
            &mut store.candidates,
            &store.snapshot,
            &new_mem,
            store.width,
            pred,
        );
        store.snapshot = new_mem;
        store.step += 1;
        let (sample, has_more) =
            page_candidates(&store.candidates, &store.snapshot, store.width, 0, limit);
        let mut out = page_output("filter", store, 0, sample, has_more);
        out.prev_count = Some(prev_count);
        out.predicate = Some(super::cheat_out::predicate_str(pred));
        Ok(Json(out))
    }

    fn cheat_list(&self, a: &CheatScanArgs) -> Result<Json<CheatScanOutput>, McpError> {
        validate_params(a, Action::List).map_err(ip)?;
        let limit = a.limit.unwrap_or(SAMPLE_DEFAULT).min(SAMPLE_MAX);
        let offset = a.offset.unwrap_or(0);
        let guard = self.cheat.lock().unwrap_or_else(PoisonError::into_inner);
        let store = guard.as_ref().ok_or_else(no_search)?;
        let (sample, has_more) = page_candidates(
            &store.candidates,
            &store.snapshot,
            store.width,
            offset,
            limit,
        );
        Ok(Json(page_output("list", store, offset, sample, has_more)))
    }

    fn cheat_reset(&self, a: &CheatScanArgs) -> Result<Json<CheatScanOutput>, McpError> {
        validate_params(a, Action::Reset).map_err(ip)?;
        let mut guard = self.cheat.lock().unwrap_or_else(PoisonError::into_inner);
        let cleared = guard.take().is_some();
        Ok(Json(CheatScanOutput {
            action: "reset".to_string(),
            cleared: Some(cleared),
            ..Default::default()
        }))
    }

    /// Fresh RAM snapshot; the tokio session lock is dropped before any std
    /// `cheat` lock (§6 — never both at once with the std lock outer).
    async fn acquire_mem(&self) -> Result<MemImage, McpError> {
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        memory::acquire(proc).await.map_err(to_mcp)
    }

    fn store_cheat(&self, store: CheatStore) {
        let mut guard = self.cheat.lock().unwrap_or_else(PoisonError::into_inner);
        *guard = Some(store);
    }
}

fn ip(msg: impl Into<String>) -> McpError {
    McpError::invalid_params(msg.into(), None)
}

fn no_search() -> McpError {
    ip("no cheat search in memory: run cheat_scan(action=\"new\") first")
}

fn filter_needs_predicate() -> McpError {
    ip("predicate is required for action=\"filter\"")
}

fn bad_geometry() -> McpError {
    ip("snapshot geometry changed (session restarted with a different memory size?) — start a new search with action=\"new\"")
}
