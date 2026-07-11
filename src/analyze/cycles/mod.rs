//! Static 68000 cycle estimation (m68000-cycles skill tables). The base
//! estimate is contention-blind; `contention.rs` (P18) layers the STE
//! Shifter bus model on its memory-access share. Hatari's profiler
//! (debug/profile.rs) is ground truth. Raw table values live in
//! `tables.rs`; opcode-word decoding in `decode.rs`/`decode_bits.rs`/
//! `decode_misc.rs` (the latter two split off `decode.rs` for the
//! file-size ceiling — see §7.4's "one permitted split" note).

pub mod contention;
mod contention_bus;
mod contention_bus_misc;
mod decode;
mod decode_bits;
mod decode_misc;
mod tables;

pub use contention::{contended_cycles, Region};

use crate::analyze::disasm::Insn;

/// One 50 Hz frame on the 8 MHz ST.
pub const VBL_CYCLES: u32 = 160_256;
/// One scanline (313 lines/frame).
pub const SCANLINE_CYCLES: u32 = 512;

/// ST bus rounding: every instruction pads to a multiple of 4 clocks
/// (reference: "you need to round all times to multiples of four, i.e 10
/// becomes 12"; confirmed live — `dbf` taken measured 12 where the table
/// says 10, `move.w (a4,d2.w),d7` measured 16 where the table says 14).
pub const fn round_st(raw: u32) -> u32 {
    (raw + 3) & !3
}

/// Raw (un-rounded) table cycles for ONE instruction's bytes, or None if
/// the instruction is outside the covered set (§7.4 list).
pub fn insn_cycles(bytes: &[u8]) -> Option<u32> {
    decode::insn_cycles(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleEstimate {
    /// Sum of round_st(per-instruction cycles); unknowns count 4.
    pub cycles: u64,
    /// Memory-access (bus) share of `cycles` — instruction fetches +
    /// operand reads/writes, always <= cycles. This is the part the STE
    /// contention model scales (contention.rs).
    pub bus_cycles: u64,
    pub instructions: usize,
    /// Instructions that fell back to the 4-cycle placeholder.
    pub unknown: usize,
}

/// Straight-line estimate over a decoded stream. **Branch policy:** a
/// static walk assumes fall-through — `Bcc` uses the NOT-taken cost
/// (.b 8 / .w 12), `DBcc` uses the taken-loop cost 10 (the loop is the
/// common case), unconditionals use their only cost (`bra` 10, `bsr` 18).
pub fn estimate(insns: &[Insn]) -> CycleEstimate {
    insns.iter().fold(
        CycleEstimate {
            cycles: 0,
            bus_cycles: 0,
            instructions: 0,
            unknown: 0,
        },
        |mut acc, i| {
            acc.instructions += 1;
            let total = match insn_cycles(&i.bytes) {
                Some(c) => round_st(c),
                None => {
                    acc.unknown += 1;
                    4
                }
            };
            acc.cycles += u64::from(total);
            acc.bus_cycles += u64::from(contention_bus::bus_cycles(&i.bytes, total));
            acc
        },
    )
}

/// `cycles / VBL_CYCLES` as a fraction of one 50 Hz frame.
pub fn vbl_fraction(cycles: u64) -> f64 {
    cycles as f64 / f64::from(VBL_CYCLES)
}

/// EA calculation cost (bw, l) by resolved mode — reference §"Time to
/// calculate effective addresses". Index = EaMode discriminant.
/// Dn/An = 0 (register direct adds nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EaMode {
    Dn,
    An,
    AnInd,
    AnPost,
    AnPre,
    AnDisp,
    AnIdx,
    AbsW,
    AbsL,
    PcDisp,
    PcIdx,
    Imm,
}

pub(crate) const fn ea_cost(mode: EaMode, long: bool) -> u32 {
    match (mode, long) {
        (EaMode::Dn, _) | (EaMode::An, _) => 0,
        (EaMode::AnInd, false) => 4,
        (EaMode::AnInd, true) => 8,
        (EaMode::AnPost, false) => 4,
        (EaMode::AnPost, true) => 8,
        (EaMode::AnPre, false) => 6,
        (EaMode::AnPre, true) => 10,
        (EaMode::AnDisp, false) => 8,
        (EaMode::AnDisp, true) => 12,
        (EaMode::AnIdx, false) => 10,
        (EaMode::AnIdx, true) => 14,
        (EaMode::AbsW, false) => 8,
        (EaMode::AbsW, true) => 12,
        (EaMode::AbsL, false) => 12,
        (EaMode::AbsL, true) => 16,
        (EaMode::PcDisp, false) => 8,
        (EaMode::PcDisp, true) => 12,
        (EaMode::PcIdx, false) => 10,
        (EaMode::PcIdx, true) => 14,
        (EaMode::Imm, false) => 4,
        (EaMode::Imm, true) => 8,
    }
}

// ---- opcode-word primitives shared by decode.rs and decode_misc.rs ----

pub(crate) fn word_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let b = bytes.get(offset..offset + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

/// Raw (mode, reg) fields from bits 5-3 / 2-0 of `word`.
pub(crate) fn ea_fields(word: u16) -> (u8, u8) {
    (((word >> 3) & 0x7) as u8, (word & 0x7) as u8)
}

/// Resolve raw (mode,reg) into an `EaMode`, sub-decoding mode 7.
pub(crate) fn ea_mode(mode: u8, reg: u8) -> Option<EaMode> {
    match mode {
        0 => Some(EaMode::Dn),
        1 => Some(EaMode::An),
        2 => Some(EaMode::AnInd),
        3 => Some(EaMode::AnPost),
        4 => Some(EaMode::AnPre),
        5 => Some(EaMode::AnDisp),
        6 => Some(EaMode::AnIdx),
        7 => match reg {
            0 => Some(EaMode::AbsW),
            1 => Some(EaMode::AbsL),
            2 => Some(EaMode::PcDisp),
            3 => Some(EaMode::PcIdx),
            4 => Some(EaMode::Imm),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_st_pads_to_four() {
        assert_eq!(round_st(10), 12);
        assert_eq!(round_st(8), 8);
        assert_eq!(round_st(14), 16);
        assert_eq!(round_st(0), 0);
    }

    fn insn(bytes: Vec<u8>) -> Insn {
        Insn {
            addr: crate::types::Addr::new(0),
            mnemonic: String::new(),
            ops: String::new(),
            bytes,
        }
    }

    #[test]
    fn estimate_rounds_and_counts_unknown() {
        let stream = vec![
            insn(vec![0x70, 0x00]),             // moveq #0,d0 -> 4
            insn(vec![0x51, 0xC8, 0xFF, 0xFE]), // dbf d0 -> 10 -> rounds to 12
            insn(vec![0xFF, 0xFF]),             // illegal -> unknown, 4
        ];
        let e = estimate(&stream);
        assert_eq!(e.cycles, 4 + 12 + 4);
        assert_eq!(e.bus_cycles, 4 + 8 + 4);
        assert_eq!(e.instructions, 3);
        assert_eq!(e.unknown, 1);
    }

    #[test]
    fn vbl_constants() {
        assert_eq!(VBL_CYCLES, 160_256);
        assert_eq!(SCANLINE_CYCLES, 512);
        assert_eq!(vbl_fraction(80_128), 0.5);
    }
}
