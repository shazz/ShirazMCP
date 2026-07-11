//! `save_prg` mode="snapshot" (P22 §8.3): freeze the whole machine into a
//! self-restoring `.PRG`. Choreography, not capability — the same
//! Paused-only, one-lock-hold contract as P16's `save_prg::gather_live`
//! (full RAM + registers must come from one coherent instant); the actual
//! pack/patch/wrap work is `ripper::snapshot::build_snapshot_prg`.

use std::path::PathBuf;

use rmcp::handler::server::wrapper::Json;
use rmcp::ErrorData as McpError;

use super::save_prg_wire::{validate_out_only, SavePrgArgs, SavePrgOutput};
use super::snapshot_capture::{ssp_warning, to_capture};
use super::{snapshot_wire, to_mcp, wrong_state, HatariServer};
use crate::analyze::dmasnd::{DMASND_BLOCK_ADDR, DMASND_BLOCK_LEN};
use crate::debug::parse::MfpRegs;
use crate::debug::{self, Registers};
use crate::error::Error;
use crate::ripper::snapshot::{self, CpuRegs, HwRegs, SnapshotFacts};
use crate::session::{memory, AnySession};
use crate::types::{Addr, MemImage};

const SHIFTER_BLOCK: Addr = Addr::new(0x00FF_8200);
const SHIFTER_BLOCK_LEN: usize = 0x66;
const MEMCFG_REG: Addr = Addr::new(0x00FF_8001);

/// Everything read under the Paused-session lock, already length-checked
/// (`to_capture` below is a pure, infallible mapper over well-shaped
/// input — the shape guarantee lives here, not there).
struct RawCapture {
    ram: MemImage,
    regs: Registers,
    ym: [u8; 16],
    mfp: MfpRegs,
    video_block: Vec<u8>,
    memcfg: u8,
    dmasnd_block: Option<Vec<u8>>,
    ste: bool,
}

fn short_block(context: &str, len: usize, need: usize) -> Error {
    Error::Parse {
        context: context.to_string(),
        input: format!("{len} bytes, need >= {need}"),
    }
}

/// The P16 `gather_live` contract: one session-lock hold across every read
/// (§4.1 — a coherent frozen instant, not a torn one).
async fn gather(proc: &mut crate::session::HatariProcess) -> crate::error::Result<RawCapture> {
    let ram = memory::acquire(proc).await?;
    let regs = debug::registers(proc).await?;
    let ym = debug::ym_registers(proc).await?;
    let mfp = debug::mfp_registers(proc).await?;
    let video_block = gather_video(proc).await?;
    let memcfg = debug::read_bytes(proc, MEMCFG_REG, 1)
        .await?
        .first()
        .copied()
        .unwrap_or(0);
    let ste = proc.config().machine.eq_ignore_ascii_case("ste");
    let dmasnd_block = gather_dmasnd(proc, ste).await?;
    Ok(RawCapture {
        ram,
        regs,
        ym,
        mfp,
        video_block,
        memcfg,
        dmasnd_block,
        ste,
    })
}

async fn gather_video(proc: &mut crate::session::HatariProcess) -> crate::error::Result<Vec<u8>> {
    let block = debug::read_bytes(proc, SHIFTER_BLOCK, SHIFTER_BLOCK_LEN).await?;
    if block.len() < SHIFTER_BLOCK_LEN {
        return Err(short_block(
            "snapshot video block",
            block.len(),
            SHIFTER_BLOCK_LEN,
        ));
    }
    Ok(block)
}

async fn gather_dmasnd(
    proc: &mut crate::session::HatariProcess,
    ste: bool,
) -> crate::error::Result<Option<Vec<u8>>> {
    if !ste {
        return Ok(None);
    }
    let block = debug::read_bytes(proc, Addr::new(DMASND_BLOCK_ADDR), DMASND_BLOCK_LEN).await?;
    if block.len() < DMASND_BLOCK_LEN {
        return Err(short_block(
            "snapshot dmasnd block",
            block.len(),
            DMASND_BLOCK_LEN,
        ));
    }
    Ok(Some(block))
}

fn finish_blocking(
    out: PathBuf,
    ram: MemImage,
    cpu: CpuRegs,
    hw: HwRegs,
) -> Result<SnapshotFacts, McpError> {
    let (bytes, facts) = snapshot::build_snapshot_prg(&ram, &cpu, &hw).map_err(to_mcp)?;
    std::fs::write(&out, &bytes).map_err(|e| to_mcp(Error::Io(e)))?;
    Ok(facts)
}

impl HatariServer {
    /// Paused gate + all capture reads under one lock hold (§4.1); mapping
    /// and the pack/patch/wrap work happen after the lock is released.
    pub(crate) async fn save_snapshot(
        &self,
        a: SavePrgArgs,
    ) -> Result<Json<SavePrgOutput>, McpError> {
        let out_path = validate_out_only(&a.out_path)?;
        let raw = self.gather_snapshot().await?;
        let ram_len = raw.ram.len() as u32;
        let (cpu, hw, mut warnings) = to_capture(
            &raw.regs,
            raw.ym,
            &raw.mfp,
            &raw.video_block,
            raw.memcfg,
            raw.dmasnd_block.as_deref(),
            raw.ste,
        );
        warnings.extend(ssp_warning(raw.regs.isp, ram_len));
        let machine = if raw.ste { "ste" } else { "st" };
        let out_display = out_path.display().to_string();
        let (out_for_task, ram) = (out_path, raw.ram);
        let facts =
            tokio::task::spawn_blocking(move || finish_blocking(out_for_task, ram, cpu, hw))
                .await
                .map_err(|e| {
                    McpError::internal_error(format!("snapshot build task failed: {e}"), None)
                })??;
        Ok(Json(SavePrgOutput::Snapshot(snapshot_wire::build(
            &out_display,
            &facts,
            machine,
            warnings,
        ))))
    }
}

impl HatariServer {
    async fn gather_snapshot(&self) -> Result<RawCapture, McpError> {
        let mut guard = self.session.lock().await;
        match &mut *guard {
            AnySession::Paused(s) => gather(s.process_mut()).await.map_err(to_mcp),
            AnySession::Running(_) => Err(wrong_state(
                "running",
                "paused — pause first so the snapshot is coherent",
            )),
            AnySession::Stopped(_) => Err(wrong_state("stopped", "paused")),
        }
    }
}
