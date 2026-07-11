//! Snapshot `.PRG` builder (P22, ADR-38): pure over an already-acquired
//! `MemImage` + captured register structs — no session/emulator imports,
//! same purity contract as the rest of `ripper/` (ADR-2). Packs the frozen
//! RAM with SNAP1 (`pack.rs`), patches the restore stub (`stub.rs`), and
//! wraps the result as a GEMDOS `.PRG` via `extract::prg_wrap`.

pub mod pack;
pub mod stub;

use std::ops::Range;

use crate::error::{Error, Result};
use crate::types::{MemImage, PluginId};

/// Restore-stub hole base (68000 exception vectors 112-255 + the crash
/// page, never hardware-raised on ST(E)) — the single Rust-side pin; must
/// equal `stub.s`'s `HOLE_BASE`.
pub const HOLE_BASE: u32 = 0x1C0;
const MAX_IMAGE_LEN: usize = 4 * 1024 * 1024;

/// Frozen CPU context, plain values (tools maps `debug::Registers` into
/// this — ripper stays free of debug-layer types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuRegs {
    pub d: [u32; 8],
    pub a: [u32; 7], // A0-A6; A7 is usp/ssp below
    pub usp: u32,
    pub ssp: u32,
    pub sr: u16,
    pub pc: u32,
}

/// Reinstallable hardware set (§4.1). All raw register bytes/words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HwRegs {
    pub ste: bool,
    pub palette: [u16; 16],
    pub video: [u8; 8], // basehi,basemid,baselo,linewidth,hscroll,sync,res,memcfg
    pub ym: [u8; 14],
    pub mfp: [u8; 15],   // field order of §5.1 f_mfp
    pub dmasnd: [u8; 8], // control,mode,start hi/mid/lo,end hi/mid/lo
}

/// Compact facts for the wire — never bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFacts {
    pub file_len: usize,
    pub region_end: u32,
    pub payload_len: usize,
    pub packed_pct: u8, // payload_len * 100 / region_end
    pub entry_pc: u32,
    pub sr: u16,
    pub hole: (u32, u32),          // (HOLE_BASE, HOLE_BASE + hole_len)
    pub frame_scratch: (u32, u32), // (ssp-6, ssp)
}

fn ripper_err(message: impl Into<String>) -> Error {
    Error::Ripper {
        plugin: PluginId::new("snapshot"),
        message: message.into(),
    }
}

/// `image.base()` must be 0 and `image.len()` the frozen `ram_len` (≤ 4 MB).
/// Pipeline: pack (§6.2) → patch stub fields (§6.3) → `prg_wrap`-shaped emit.
pub fn build_snapshot_prg(
    image: &MemImage,
    cpu: &CpuRegs,
    hw: &HwRegs,
) -> Result<(Vec<u8>, SnapshotFacts)> {
    validate_inputs(image, cpu)?;
    let hole = hole_range()?;
    let stream = pack::pack(image.bytes(), hole.clone()).map_err(pack_err)?;
    let region_end = image.len() as u32;
    let payload_len = stream.len() as u32;
    let stub_patched = stub::patch(cpu, hw, region_end, payload_len)?;
    let text = assemble_text(stub_patched, stream);
    let bytes = super::extract::prg_wrap(&text);
    let facts = build_facts(bytes.len(), region_end, payload_len, cpu, &hole);
    Ok((bytes, facts))
}

fn validate_inputs(image: &MemImage, cpu: &CpuRegs) -> Result<()> {
    if image.base().raw() != 0 {
        return Err(ripper_err(format!(
            "snapshot image must start at bus address $0, got {}",
            image.base()
        )));
    }
    if image.len() > MAX_IMAGE_LEN {
        return Err(ripper_err("snapshot image exceeds 4 MB"));
    }
    if !cpu.pc.is_multiple_of(2) {
        return Err(ripper_err(format!("frozen PC ${:x} is odd", cpu.pc)));
    }
    Ok(())
}

fn hole_range() -> Result<Range<usize>> {
    let len = stub::hole_len()?;
    let start = HOLE_BASE as usize;
    Ok(start..start + len)
}

fn pack_err(e: pack::PackError) -> Error {
    let pack::PackError::Dense { payload, budget } = e;
    ripper_err(format!(
        "RAM too dense to self-restore: packed {payload} bytes exceeds the {budget}-byte budget"
    ))
}

/// `text = stub_patched ‖ payload`, word-padded (GEMDOS PRG text is
/// word-sized; `prg_wrap` does not pad itself).
fn assemble_text(stub_patched: Vec<u8>, stream: Vec<u8>) -> Vec<u8> {
    let mut text = stub_patched;
    text.extend_from_slice(&stream);
    if !text.len().is_multiple_of(2) {
        text.push(0);
    }
    text
}

fn build_facts(
    file_len: usize,
    region_end: u32,
    payload_len: u32,
    cpu: &CpuRegs,
    hole: &Range<usize>,
) -> SnapshotFacts {
    SnapshotFacts {
        file_len,
        region_end,
        payload_len: payload_len as usize,
        packed_pct: (u64::from(payload_len) * 100 / u64::from(region_end.max(1))) as u8,
        entry_pc: cpu.pc,
        sr: cpu.sr,
        hole: (hole.start as u32, hole.end as u32),
        frame_scratch: (cpu.ssp.saturating_sub(6), cpu.ssp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ripper::prg_file;
    use crate::types::Addr;

    fn cpu() -> CpuRegs {
        CpuRegs {
            d: [0; 8],
            a: [0; 7],
            usp: 0x1000,
            ssp: 0x2000,
            sr: 0x2700,
            pc: 0x1200,
        }
    }

    fn hw() -> HwRegs {
        HwRegs {
            ste: true,
            palette: [0; 16],
            video: [0; 8],
            ym: [0; 14],
            mfp: [0; 15],
            dmasnd: [0; 8],
        }
    }

    fn image(len: usize) -> MemImage {
        MemImage::new(vec![0u8; len], Addr::new(0))
    }

    /// A packable image: mostly zero, with a small non-zero tail (real ST
    /// RAM always has *something* — TOS vars, stack contents — near the
    /// top; an entirely-zero region has nothing to anchor the mandatory
    /// final raw run against, which `pack`'s own tests document costs
    /// close to the whole region — see `ripper::snapshot::pack`'s module
    /// doc on why a run reaching exactly `region_end` can never itself
    /// stay zero-encoded).
    fn packable_image(len: usize) -> MemImage {
        let mut bytes = vec![0u8; len];
        bytes[len - 16..].copy_from_slice(&[0xEFu8; 16]);
        MemImage::new(bytes, Addr::new(0))
    }

    #[test]
    fn build_emits_parseable_prg() {
        let (bytes, facts) = build_snapshot_prg(&packable_image(1 << 20), &cpu(), &hw()).unwrap();
        let prg = prg_file::parse(bytes.clone()).unwrap();
        assert_eq!(
            prg.header.tlen as usize,
            bytes.len() - prg_file::HEADER_LEN - 4
        );
        assert_eq!(prg.header.dlen, 0);
        assert_eq!(prg.header.blen, 0);
        assert_eq!(prg.header.slen, 0);
        assert_eq!(prg.header.absflag, 0);
        assert!(prg.reloc.is_empty());
        assert_eq!(facts.file_len, bytes.len());
    }

    #[test]
    fn facts_are_consistent() {
        let (_, facts) = build_snapshot_prg(&packable_image(1 << 20), &cpu(), &hw()).unwrap();
        assert_eq!(facts.region_end, 1 << 20);
        assert_eq!(facts.entry_pc, 0x1200);
        assert_eq!(facts.hole.0, HOLE_BASE);
        assert!(facts.hole.1 > facts.hole.0);
        assert_eq!(facts.frame_scratch, (0x2000 - 6, 0x2000));
        assert!(facts.packed_pct < 100);
    }

    #[test]
    fn rejects_odd_pc() {
        let mut c = cpu();
        c.pc = 0x1201;
        let err = build_snapshot_prg(&image(1 << 16), &c, &hw()).unwrap_err();
        assert!(matches!(err, Error::Ripper { .. }));
    }

    #[test]
    fn rejects_nonzero_base() {
        let img = MemImage::new(vec![0u8; 1 << 16], Addr::new(0x1000));
        let err = build_snapshot_prg(&img, &cpu(), &hw()).unwrap_err();
        assert!(matches!(err, Error::Ripper { .. }));
    }

    #[test]
    fn dense_maps_to_ripper_error() {
        // Region == hole.end + stage0 reserve exactly -> the pack budget is
        // 0, so any content is unpackable (mirrors pack::tests::dense_image_errors).
        let hole_end = HOLE_BASE as usize + stub::hole_len().unwrap();
        let len = hole_end + 0x400;
        let mut x = 12345u32;
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect();
        let err =
            build_snapshot_prg(&MemImage::new(bytes, Addr::new(0)), &cpu(), &hw()).unwrap_err();
        assert!(matches!(err, Error::Ripper { .. }));
    }
}
