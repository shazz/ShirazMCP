//! The committed restore-stub blob (`stub.s`, assembled once with vasm) +
//! the pure field-patcher that stamps a frozen capture into a fresh copy
//! (P22 §5.1/§5.4). Pure: no session/emulator imports.

use crate::error::{Error, Result};
use crate::types::PluginId;

use super::{CpuRegs, HwRegs};

/// The assembled restore stub (`stub.s` -> `stub.bin`, vasm `-Fbin`).
pub static STUB: &[u8] = include_bytes!("stub.bin");

const MAGIC: &[u8; 4] = b"SNP1";

// Field offsets relative to the "SNP1" magic (§5.1 table — normative).
pub const F_FLAGS: usize = 4;
pub const F_REGION_END: usize = 6;
pub const F_PAYLOAD_LEN: usize = 10;
pub const F_PC: usize = 14;
pub const F_SR: usize = 18;
pub const F_USP: usize = 20;
pub const F_SSP: usize = 24;
pub const F_DREGS: usize = 28;
pub const F_AREGS: usize = 60;
pub const F_PAL: usize = 88;
pub const F_VIDEO: usize = 120;
pub const F_YM: usize = 128;
pub const F_MFP: usize = 144;
pub const F_DMASND: usize = 160;
/// Field block length (§5.1: `+168 — hole code follows`).
pub const FIELDS_LEN: usize = 168;

fn ripper_err(message: impl Into<String>) -> Error {
    Error::Ripper {
        plugin: PluginId::new("snapshot"),
        message: message.into(),
    }
}

/// Locate the single `"SNP1"` magic in `STUB` (scanned once, not hardcoded
/// — the offset moves whenever `stub.s` is re-authored). `Error::Ripper`
/// if it is absent or appears more than once (a corrupt/ambiguous blob).
pub fn magic_off() -> Result<usize> {
    let mut hits = STUB
        .windows(MAGIC.len())
        .enumerate()
        .filter_map(|(i, w)| (w == MAGIC).then_some(i));
    let first = hits
        .next()
        .ok_or_else(|| ripper_err("stub.bin has no \"SNP1\" magic"))?;
    if hits.next().is_some() {
        return Err(ripper_err("stub.bin has more than one \"SNP1\" magic"));
    }
    Ok(first)
}

/// `(fields..payload)` length — the block stage-0 copies to `HOLE_BASE`.
/// Tested `<= 576` (`$400 - $1C0`) by `hole_block_fits`.
pub fn hole_len() -> Result<usize> {
    Ok(STUB.len() - magic_off()?)
}

/// Copy `STUB` and write every §5.1 field, big-endian. `region_end`/
/// `payload_len` are computed by the caller (`build_snapshot_prg`), not
/// carried on `CpuRegs`/`HwRegs`.
pub fn patch(cpu: &CpuRegs, hw: &HwRegs, region_end: u32, payload_len: u32) -> Result<Vec<u8>> {
    let base = magic_off()?;
    let mut out = STUB.to_vec();
    let f = base;
    write_flags_and_lens(&mut out, f, hw.ste, region_end, payload_len);
    write_cpu(&mut out, f, cpu);
    write_hw(&mut out, f, hw);
    Ok(out)
}

fn put(out: &mut [u8], at: usize, bytes: &[u8]) {
    out[at..at + bytes.len()].copy_from_slice(bytes);
}

fn write_flags_and_lens(out: &mut [u8], f: usize, ste: bool, region_end: u32, payload_len: u32) {
    let flags: u16 = if ste { 1 } else { 0 };
    put(out, f + F_FLAGS, &flags.to_be_bytes());
    put(out, f + F_REGION_END, &region_end.to_be_bytes());
    put(out, f + F_PAYLOAD_LEN, &payload_len.to_be_bytes());
}

fn write_cpu(out: &mut [u8], f: usize, cpu: &CpuRegs) {
    put(out, f + F_PC, &cpu.pc.to_be_bytes());
    put(out, f + F_SR, &cpu.sr.to_be_bytes());
    put(out, f + F_USP, &cpu.usp.to_be_bytes());
    put(out, f + F_SSP, &cpu.ssp.to_be_bytes());
    for (i, d) in cpu.d.iter().enumerate() {
        put(out, f + F_DREGS + i * 4, &d.to_be_bytes());
    }
    for (i, a) in cpu.a.iter().enumerate() {
        put(out, f + F_AREGS + i * 4, &a.to_be_bytes());
    }
}

fn write_hw(out: &mut [u8], f: usize, hw: &HwRegs) {
    for (i, w) in hw.palette.iter().enumerate() {
        put(out, f + F_PAL + i * 2, &w.to_be_bytes());
    }
    put(out, f + F_VIDEO, &hw.video);
    put(out, f + F_YM, &hw.ym); // 14 bytes; 2 trailing pad bytes untouched
    put(out, f + F_MFP, &hw.mfp); // 15 bytes; 1 trailing pad byte untouched
    put(out, f + F_DMASND, &hw.dmasnd);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu() -> CpuRegs {
        CpuRegs {
            d: [0x1111_1111; 8],
            a: [0x2222_2222; 7],
            usp: 0x3333_3333,
            ssp: 0x4444_4444,
            sr: 0x2700,
            pc: 0x0001_2000,
        }
    }

    fn hw() -> HwRegs {
        HwRegs {
            ste: true,
            palette: [0x0777; 16],
            video: [1, 2, 3, 4, 5, 6, 7, 8],
            ym: [0xAA; 14],
            mfp: [0xBB; 15],
            dmasnd: [0xCC; 8],
        }
    }

    #[test]
    fn magic_found_once() {
        let off = magic_off().unwrap();
        assert_eq!(&STUB[off..off + 4], b"SNP1");
    }

    #[test]
    fn hole_block_fits() {
        assert!(hole_len().unwrap() <= 576);
        assert_eq!(STUB.len() % 2, 0, "blob length must be even");
    }

    #[test]
    fn patch_writes_all_fields() {
        let before = STUB.to_vec();
        let out = patch(&cpu(), &hw(), 0x0020_0000, 0x0001_0000).unwrap();
        let base = magic_off().unwrap();
        assert_eq!(&out[base..base + 4], b"SNP1");
        assert_eq!(
            u16::from_be_bytes(out[base + F_FLAGS..base + F_FLAGS + 2].try_into().unwrap()),
            1
        );
        assert_eq!(
            u32::from_be_bytes(
                out[base + F_REGION_END..base + F_REGION_END + 4]
                    .try_into()
                    .unwrap()
            ),
            0x0020_0000
        );
        assert_eq!(
            u32::from_be_bytes(out[base + F_PC..base + F_PC + 4].try_into().unwrap()),
            0x0001_2000
        );
        assert_eq!(
            u16::from_be_bytes(out[base + F_SR..base + F_SR + 2].try_into().unwrap()),
            0x2700
        );
        assert_eq!(&out[base + F_VIDEO..base + F_VIDEO + 8], &hw().video);
        assert_eq!(STUB, before.as_slice(), "STUB itself must never be mutated");
    }

    #[test]
    fn patch_rejects_nothing_but_returns_result() {
        // No failure path exists once STUB is well-formed; this documents
        // the Result signature is for a corrupt/ambiguous blob, not args.
        assert!(patch(&cpu(), &hw(), 1, 1).is_ok());
    }
}
