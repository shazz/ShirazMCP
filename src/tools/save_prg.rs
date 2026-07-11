//! `save_prg`: persist a live hack. mode="repatch" (P16) re-emits the
//! running, patched program as a modified copy of its original `.PRG`;
//! mode="snapshot" (P22, ADR-38) freezes the whole machine into a
//! self-restoring `.PRG` (`snapshot_prg.rs`). Choreography, not capability
//! — basepage discovery (`debug::basepage`) → source parse
//! (`ripper::prg_file`) → ranged RAM acquire (ADR-9) → pure diff/apply
//! (`ripper::repatch`) → one file write, for repatch. Paused-only in both
//! modes: saving a hack is deliberate; a frozen machine is the only
//! coherent thing to persist (freezer-cartridge semantics, ADR-30).

use std::path::PathBuf;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::repatch_wire::{convert_regions, summarize};
use super::save_prg_wire::{
    validate_args, validate_mode, validate_paths, Mode, SavePrgArgs as WireArgs,
};
use super::{to_mcp, wrong_state, HatariServer};
use crate::debug::basepage::{self, Basepage};
use crate::ripper::prg_file::{self, PrgHeader};
use crate::ripper::repatch::{self, RepatchPlan};
use crate::session::{memory, AnySession};
use crate::types::MemImage;

// Re-exported for `tests/save_prg_live.rs` (P16) and `tests/
// snapshot_prg_live.rs` (P22) — `save_prg_wire` convention (mem/mod.rs).
pub use super::save_prg_wire::{RepatchOutput, SavePrgArgs, SavePrgOutput};
pub use super::snapshot_wire::SnapshotOutput;

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = save_prg_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "save_prg",
        description = "Persist a live hack. mode=\"repatch\" (default) re-emits the running \
program as a modified copy of its original .PRG, with your RAM patches mapped back to their \
file offsets — requires a PAUSED session and the path of the original .PRG (source_prg). \
Patches to loaded TEXT/DATA persist; patches inside relocated pointers and anything in \
BSS/runtime state are reported but NOT representable in the file. Returns the patch runs \
applied, relocation warnings, and divergence facts (or pass regions to restrict what is \
applied). mode=\"snapshot\" instead freezes the whole machine into a self-restoring .PRG (full \
RAM + CPU + hardware registers; no original file needed, source_prg/regions are not used) — \
see the returned not_restored list for what a resume cannot bring back."
    )]
    pub async fn save_prg(
        &self,
        Parameters(a): Parameters<WireArgs>,
    ) -> Result<Json<SavePrgOutput>, McpError> {
        let mode = validate_mode(a.mode.as_deref())?;
        validate_args(&a, &mode)?;
        match mode {
            Mode::Repatch => self.save_repatch(a).await,
            Mode::Snapshot => self.save_snapshot(a).await,
        }
    }
}

impl HatariServer {
    /// The pre-P22 `save_prg` body (§7.1), unchanged in behavior — only
    /// its entry point moved (mode dispatch now lives in `save_prg`).
    async fn save_repatch(&self, a: WireArgs) -> Result<Json<SavePrgOutput>, McpError> {
        let (source, out) = validate_paths(&a)?;
        let (bp, ram) = self.gather_live().await?;
        let regions = convert_regions(a.regions.as_deref(), &bp)?;
        tokio::task::spawn_blocking(move || finish_blocking(source, out, bp, ram, regions))
            .await
            .map_err(|e| McpError::internal_error(format!("save_prg task failed: {e}"), None))?
            .map(SavePrgOutput::Repatch)
            .map(Json)
    }

    /// Paused gate + the two live reads (basepage + ranged RAM). No
    /// `mem::replace` — no state transition happens; the lock is held
    /// across both awaits (the `machine_state.rs`/`tools/mod.rs` contract)
    /// and drops when this returns.
    async fn gather_live(&self) -> Result<(Basepage, MemImage), McpError> {
        let mut guard = self.session.lock().await;
        match &mut *guard {
            AnySession::Paused(s) => {
                let proc = s.process_mut();
                let ram_len = proc.config().ram_len();
                let bp = basepage::current_basepage(proc, ram_len)
                    .await
                    .map_err(to_mcp)?;
                let ram =
                    memory::acquire_range(proc, bp.text_base, (bp.text_len + bp.data_len) as usize)
                        .await
                        .map_err(to_mcp)?;
                Ok((bp, ram))
            }
            AnySession::Running(_) => Err(wrong_state(
                "running",
                "paused — pause first so the save is coherent",
            )),
            AnySession::Stopped(_) => Err(wrong_state("stopped", "paused")),
        }
    }
}

/// §7.1 step 5: parse source → cross-check → diff → apply → write → warn.
fn finish_blocking(
    source: PathBuf,
    out: PathBuf,
    bp: Basepage,
    ram: MemImage,
    regions: Option<Vec<(usize, usize)>>,
) -> Result<super::save_prg_wire::RepatchOutput, McpError> {
    let prg = prg_file::read_prg_file(&source).map_err(to_mcp)?;
    check_match(&prg.header, &bp)?;
    let expected = repatch::expected_ram(&prg, bp.text_base);
    let plan = repatch::diff(
        &prg,
        &expected,
        ram.bytes(),
        bp.text_base,
        regions.as_deref(),
    );
    let out_bytes = repatch::apply(&prg, &plan, ram.bytes());
    std::fs::write(&out, &out_bytes).map_err(|e| to_mcp(crate::error::Error::Io(e)))?;
    let warnings = build_warnings(&bp, &plan, prg.seg_len());
    Ok(summarize(
        &out,
        out_bytes.len() as u64,
        &bp,
        &plan,
        warnings,
    ))
}

/// The §4.3 caveat closer: catches "wrong file" and packed/self-relocating
/// programs before any bytes are diffed.
fn check_match(h: &PrgHeader, bp: &Basepage) -> Result<(), McpError> {
    if h.tlen != bp.text_len || h.dlen != bp.data_len || h.blen != bp.bss_len {
        return Err(McpError::invalid_params(
            format!(
                "source PRG does not match the running program (text/data/bss {}/{}/{} file vs \
                 {}/{}/{} live) — wrong file, or the program is packed/self-relocating",
                h.tlen, h.dlen, h.blen, bp.text_len, bp.data_len, bp.bss_len
            ),
            None,
        ));
    }
    Ok(())
}

fn build_warnings(bp: &Basepage, plan: &RepatchPlan, seg_len: usize) -> Vec<String> {
    let mut warnings = Vec::new();
    if bp.text_base.raw() != bp.addr.raw() + 0x100 {
        warnings.push(format!(
            "text base {} is not basepage+$100 — unusual loader // verify live",
            bp.text_base
        ));
    }
    if plan.changed_bytes.saturating_mul(10) > seg_len {
        warnings.push(format!(
            "{} of {seg_len} bytes differ from the file image — the program self-modifies or \
             runtime DATA writes are being persisted; review the runs (or pass regions)",
            plan.changed_bytes
        ));
    }
    if !plan.reloc_skips.is_empty() {
        warnings.push(format!(
            "{} patched relocated pointer(s) NOT persisted (not file-representable)",
            plan.reloc_skips.len()
        ));
    }
    if plan.filtered_bytes > 0 {
        warnings.push(format!(
            "{} changed byte(s) outside the given regions were not applied",
            plan.filtered_bytes
        ));
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Addr;

    fn basepage() -> Basepage {
        Basepage {
            addr: Addr::new(0x20000),
            text_base: Addr::new(0x20100),
            text_len: 0x1000,
            data_base: Addr::new(0x21100),
            data_len: 0x200,
            bss_base: Addr::new(0x21300),
            bss_len: 0x80,
        }
    }

    fn header() -> PrgHeader {
        PrgHeader {
            tlen: 0x1000,
            dlen: 0x200,
            blen: 0x80,
            slen: 0,
            absflag: 0,
        }
    }

    #[test]
    fn check_match_ok() {
        assert!(check_match(&header(), &basepage()).is_ok());
    }

    #[test]
    fn check_match_tlen_mismatch() {
        let mut h = header();
        h.tlen += 4;
        let err = check_match(&h, &basepage()).unwrap_err();
        assert!(err.message.contains("does not match"));
    }

    #[test]
    fn check_match_dlen_mismatch() {
        let mut h = header();
        h.dlen += 4;
        assert!(check_match(&h, &basepage()).is_err());
    }

    #[test]
    fn check_match_blen_mismatch() {
        let mut h = header();
        h.blen += 4;
        assert!(check_match(&h, &basepage()).is_err());
    }
}
