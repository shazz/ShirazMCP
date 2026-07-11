//! `disasm`: Capstone snapshot disasm, or Hatari's own live `d`. The
//! `profile` tool (same feature group) is `analyze_profile.rs` — split to
//! stay under the file-size ceiling; both share `analyze_wire.rs`.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::analyze_wire::{
    annotate_ops, parse_live_line, track_psg, LiveInsn, DISASM_DEFAULT, DISASM_MAX,
};
use super::insn_out::insns_out;
use super::mem::scan::parse_addr;
use super::resolve::resolve_even_addr;
use super::{to_mcp, wrong_state, HatariServer};
use crate::analyze::{self, PsgSelect};
use crate::session::HatariProcess;
use crate::types::Addr;

// Re-exported so live integration tests (`tests/analyze_live.rs`) can call
// `HatariServer::disasm` directly with typed args — `analyze_wire` itself
// stays `pub(crate)` (rip_wire.rs/view_wire.rs convention).
pub use super::analyze_wire::{DisasmArgs, DisasmOutput, DisasmSource};
pub use super::insn_out::InsnOut;

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = analyze_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "disasm",
        description = "Disassemble 68000 code. source=snapshot (default) \
                       decodes an acquired RAM snapshot with the embedded \
                       Capstone — fast, typed, includes a static cycle \
                       estimate per instruction (flagged estimate: ST bus \
                       contention not modelled). source=live asks \
                       Hatari's own debugger (knows effective-address \
                       hints). Hardware operand addresses come back \
                       annotated. Default 32 instructions, max 128 — page \
                       with next_addr. addr accepts a symbol name once a \
                       map is loaded (symbols tool)."
    )]
    pub async fn disasm(
        &self,
        Parameters(a): Parameters<DisasmArgs>,
    ) -> Result<Json<DisasmOutput>, McpError> {
        let count = a.count.unwrap_or(DISASM_DEFAULT).min(DISASM_MAX);
        let source = a.source.unwrap_or(DisasmSource::Snapshot);
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        let addr = resolve_even_addr(proc, &a.addr).await?;
        let (insns, next_addr) = match source {
            DisasmSource::Snapshot => snapshot_insns(proc, addr, count).await?,
            DisasmSource::Live => live_insns(proc, addr, count).await?,
        };
        Ok(Json(DisasmOutput {
            source: source_name(source).to_string(),
            insns,
            next_addr: next_addr.to_string(),
        }))
    }
}

pub(crate) fn parse_even_addr(s: &str) -> Result<Addr, McpError> {
    let addr = parse_addr(s)?;
    if addr.raw() % 2 != 0 {
        return Err(McpError::invalid_params(
            "disasm address must be even",
            None,
        ));
    }
    Ok(addr)
}

fn source_name(source: DisasmSource) -> &'static str {
    if source == DisasmSource::Snapshot {
        "snapshot"
    } else {
        "live"
    }
}

async fn snapshot_insns(
    proc: &mut HatariProcess,
    addr: Addr,
    count: u32,
) -> Result<(Vec<InsnOut>, Addr), McpError> {
    let len = count as usize * analyze::disasm::MAX_INSN_LEN;
    let mem = crate::session::memory::acquire_range(proc, addr, len)
        .await
        .map_err(to_mcp)?;
    let insns = analyze::disasm(&mem, addr, count as usize).map_err(to_mcp)?;
    Ok(insns_out(&insns, addr))
}

async fn live_insns(
    proc: &mut HatariProcess,
    addr: Addr,
    count: u32,
) -> Result<(Vec<InsnOut>, Addr), McpError> {
    let to = addr.raw() + count * 10;
    let raw = proc
        .debug(&format!("d ${:x}-${:x}", addr.raw(), to))
        .await
        .map_err(to_mcp)?;
    let mut psg = None;
    let mut next = addr;
    let mut out = Vec::new();
    for li in raw.lines().filter_map(parse_live_line).take(count as usize) {
        psg = track_psg(psg, &li.mnemonic, &li.ops);
        next = Addr::new(li.addr.raw() + li.word_count as u32 * 2);
        out.push(live_insn_out(&li, psg));
    }
    Ok((out, next))
}

fn live_insn_out(li: &LiveInsn, psg: Option<PsgSelect>) -> InsnOut {
    InsnOut {
        addr: li.addr.to_string(),
        mnemonic: li.mnemonic.clone(),
        ops: li.ops.clone(),
        bytes_hex: li.bytes_hex.clone(),
        cycles_est: None,
        annotations: annotate_ops(&li.ops, psg),
    }
}
