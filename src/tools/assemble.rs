//! `assemble`/`patch` (ADR-18) — the freezer cartridge's built-in
//! assembler: hot-patch 68k code into the running/frozen ST with no PRG
//! rebuild. `assemble` is session-independent (pure vasm drive); `patch`
//! assembles at `addr`, writes through the existing `debug::write_bytes`
//! (`memwrite`) path — which chunks internally to a safe per-line size,
//! §22 — then readback-verifies with `memory::acquire_range` + the P2
//! Capstone disasm.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::analyze::parse_even_addr;
use super::assemble_wire::{readback_listing, PATCH_MAX_BYTES};
use super::mem::scan::parse_addr;
use super::{to_mcp, wrong_state, HatariServer};
use crate::assemble as asm;
use crate::types::Addr;

// Re-exported so integration tests (`tests/assemble.rs`) can call
// `HatariServer::assemble`/`patch` directly with typed args — `assemble_wire`
// itself stays `pub(crate)` (analyze.rs/rip_wire.rs convention).
pub use super::assemble_wire::{AssembleArgs, AssembleOutput, PatchArgs, PatchOutput};

#[tool_router(router = assemble_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "assemble",
        description = "Assemble Motorola 68000 source (vasm Motorola \
                       syntax — the ST demo toolchain dialect: labels, \
                       $hex, dc.b/dc.w, move.w/.l…) to machine code. No \
                       emulator needed. Returns hex bytes + length; \
                       assembly errors come back in `errors` — fix the \
                       source and call again. Optional org sets the load \
                       address for absolute label math (the source must \
                       not contain its own org/rorg when org is given). \
                       listing=true adds the assembler listing. Requires \
                       the vasm binary (override path with SHIRAZMCP_VASM)."
    )]
    pub async fn assemble(
        &self,
        Parameters(a): Parameters<AssembleArgs>,
    ) -> Result<Json<AssembleOutput>, McpError> {
        let org = a.org.as_deref().map(parse_addr).transpose()?;
        let prepared = asm::source::prepare_source(&a.source, org)
            .map_err(|m| McpError::invalid_params(m, None))?;
        let r = asm::assemble(&asm::vasm_path(), &prepared, a.listing.unwrap_or(false))
            .await
            .map_err(to_mcp)?;
        Ok(Json(AssembleOutput::from_result(r)))
    }

    #[tool(
        name = "patch",
        description = "Hot-patch the ST: assemble 68000 source at addr \
                       (org is forced to addr) and write it into emulator \
                       RAM — test a code change in place with no PRG \
                       rebuild. Best used while paused (frozen); running \
                       works but the CPU may execute half-written code \
                       mid-patch. After writing, the region is read back \
                       and disassembled (`readback`): verified=true means \
                       RAM now holds exactly the assembled bytes (a ROM or \
                       unmapped destination fails verification). Max 4096 \
                       bytes per patch."
    )]
    pub async fn patch(
        &self,
        Parameters(a): Parameters<PatchArgs>,
    ) -> Result<Json<PatchOutput>, McpError> {
        let addr = parse_even_addr(&a.addr)?;
        let prepared = asm::source::prepare_source(&a.source, Some(addr))
            .map_err(|m| McpError::invalid_params(m, None))?;
        let r = asm::assemble(&asm::vasm_path(), &prepared, false)
            .await
            .map_err(to_mcp)?;
        if !r.ok {
            return Ok(Json(PatchOutput::failed_assembly(
                addr, r.errors, r.warnings,
            )));
        }
        if let Some(out) = size_gate(addr, &r) {
            return Ok(Json(out));
        }
        self.write_and_verify(addr, r).await.map(Json)
    }
}

impl HatariServer {
    async fn write_and_verify(
        &self,
        addr: Addr,
        r: asm::AsmResult,
    ) -> Result<PatchOutput, McpError> {
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        // `debug::write_bytes` chunks internally to whatever line size
        // Hatari's debugger console can actually parse (§ its own doc
        // comment) — no need to also chunk here.
        crate::debug::write_bytes(proc, addr, &r.bytes)
            .await
            .map_err(to_mcp)?;
        let mem = crate::session::memory::acquire_range(proc, addr, r.bytes.len())
            .await
            .map_err(to_mcp)?;
        drop(guard);
        Ok(patch_output(addr, r, &mem))
    }
}

/// §4.4 size gates (data, not an MCP error — the source assembled fine).
fn size_gate(addr: Addr, r: &asm::AsmResult) -> Option<PatchOutput> {
    let msg = if r.bytes.is_empty() {
        "assembled to 0 bytes — nothing to write".to_string()
    } else if r.bytes.len() > PATCH_MAX_BYTES {
        format!(
            "assembled to {} bytes, patch max is {PATCH_MAX_BYTES}",
            r.bytes.len()
        )
    } else {
        return None;
    };
    Some(PatchOutput::failed_assembly(
        addr,
        vec![msg],
        r.warnings.clone(),
    ))
}

fn patch_output(addr: Addr, r: asm::AsmResult, mem: &crate::types::MemImage) -> PatchOutput {
    let verified = mem.bytes() == r.bytes.as_slice();
    let (readback, readback_truncated) = readback_listing(mem, addr, r.bytes.len());
    PatchOutput {
        ok: verified,
        assembled: true,
        verified,
        addr: addr.to_string(),
        length: r.bytes.len() as u32,
        end_addr: Addr::new(addr.raw() + r.bytes.len() as u32).to_string(),
        errors: r.errors,
        warnings: r.warnings,
        readback,
        readback_truncated,
    }
}
