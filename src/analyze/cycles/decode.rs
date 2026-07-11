//! Opcode classifier: raw (un-rounded) 68000 table cycles per instruction,
//! decoded from big-endian opcode words (standard 68000 EA/opmode fields).
//! This file: MOVE/MOVEQ, ADD/SUB(X)/ADDA/SUBA, AND/OR/MUL/DIV/ABCD/SBCD/
//! EXG, EOR/CMP/CMPA/CMPM, shifts, branches. Bit-manipulation/immediate
//! (nibble 0x0) is `decode_bits.rs`; the 0x4xxx misc bucket and
//! ADDQ/SUBQ/Scc/DBcc (0x5xxx) are `decode_misc.rs` (§7.4's one permitted
//! split, applied twice to stay under the file-size ceiling).

use super::decode_misc;
use super::tables;
use super::{ea_cost, ea_fields, ea_mode, word_at, EaMode};

pub(super) fn insn_cycles(bytes: &[u8]) -> Option<u32> {
    let word = word_at(bytes, 0)?;
    match word >> 12 {
        0x0 => decode_misc::group_0(word),
        0x1..=0x3 => decode_move(word),
        0x4 => decode_misc::group_4(bytes, word),
        0x5 => decode_misc::group_5(word),
        0x6 => decode_branch(word),
        0x7 if word & 0x0100 == 0 => Some(4), // moveq
        0x8 => decode_or_family(word),
        0x9 | 0xD => decode_add_sub(word),
        0xB => decode_b_family(word),
        0xC => decode_and_family(word),
        0xE => decode_shift(word),
        _ => None,
    }
}

fn ea_of(word: u16) -> Option<EaMode> {
    let (m, r) = ea_fields(word);
    ea_mode(m, r)
}

fn decode_move(word: u16) -> Option<u32> {
    let long = match word >> 12 {
        0x1 | 0x3 => false,
        0x2 => true,
        _ => return None,
    };
    let src = ea_of(word)?;
    let dst_mode = ((word >> 6) & 0x7) as u8;
    let dst_reg = ((word >> 9) & 0x7) as u8;
    let dst_col = ea_mode(dst_mode, dst_reg)? as usize;
    if dst_col >= 9 {
        return None; // MOVE never targets PC-relative/immediate
    }
    Some(tables::move_cost(src, dst_col, long))
}

/// Shared `<ea>,Dn` (opmode 0-2) / `Dn,<ea>` (opmode 4-6) cost, used by
/// ADD/SUB/AND/OR — the four families share this exact formula.
fn ea_dn_dn_ea_cost(opmode: u8, ea: EaMode) -> Option<u32> {
    match opmode {
        0..=2 => {
            let long = opmode == 2;
            let bonus = u32::from(long && matches!(ea, EaMode::Dn | EaMode::An | EaMode::Imm)) * 2;
            Some(if long { 6 } else { 4 } + bonus + ea_cost(ea, long))
        }
        4..=6 => {
            let long = opmode == 6;
            Some(if long { 12 } else { 8 } + ea_cost(ea, long))
        }
        _ => None,
    }
}

/// ADD and SUB share an identical cost formula (family nibble is the only
/// difference), so one decoder covers both (0x9=SUB, 0xD=ADD).
fn decode_add_sub(word: u16) -> Option<u32> {
    // ADDX/SUBX: bit8=1, bits5-4=00 (predecrement flag is bit3).
    if word & 0x0130 == 0x0100 {
        let long = ((word >> 6) & 0x3) == 2;
        let predecrement = word & 0x8 != 0;
        return Some(match (predecrement, long) {
            (true, true) => 30,
            (true, false) => 18,
            (false, true) => 8,
            (false, false) => 4,
        });
    }
    let opmode = ((word >> 6) & 0x7) as u8;
    let ea = ea_of(word)?;
    match opmode {
        3 => Some(8 + ea_cost(ea, false)), // ADDA.W/SUBA.W <ea>,An
        7 => {
            let bonus = u32::from(matches!(ea, EaMode::Dn | EaMode::An | EaMode::Imm)) * 2;
            Some(6 + bonus + ea_cost(ea, true)) // ADDA.L/SUBA.L
        }
        _ => ea_dn_dn_ea_cost(opmode, ea),
    }
}

/// ABCD/SBCD (opmode 4, bits5-4=00) or EXG (opmode 5/6, `and_family` only —
/// EXG's opcode space is reserved exclusively under 0xC). `None` if `word`
/// isn't one of these narrow special forms.
fn packed_or_exg(word: u16, and_family: bool) -> Option<u32> {
    if word & 0x0100 == 0 || (word >> 3) & 0x3 != 0 {
        return None;
    }
    match (word >> 6) & 0x7 {
        4 => Some(if word & 0x8 != 0 { 18 } else { 6 }), // ABCD/SBCD
        5 | 6 if and_family => Some(tables::EXG),
        _ => None,
    }
}

fn decode_and_family(word: u16) -> Option<u32> {
    if let Some(c) = packed_or_exg(word, true) {
        return Some(c);
    }
    let ea = ea_of(word)?;
    match ((word >> 6) & 0x7) as u8 {
        3 | 7 => Some(tables::MULX + ea_cost(ea, false)), // MULU/MULS
        opmode => ea_dn_dn_ea_cost(opmode, ea),
    }
}

fn decode_or_family(word: u16) -> Option<u32> {
    if let Some(c) = packed_or_exg(word, false) {
        return Some(c);
    }
    let ea = ea_of(word)?;
    match ((word >> 6) & 0x7) as u8 {
        3 => Some(tables::DIVU + ea_cost(ea, false)),
        7 => Some(tables::DIVS + ea_cost(ea, false)),
        opmode => ea_dn_dn_ea_cost(opmode, ea),
    }
}

fn decode_b_family(word: u16) -> Option<u32> {
    let (mode_bits, _) = ea_fields(word);
    match (word >> 6) & 0x7 {
        0..=2 => {
            let long = ((word >> 6) & 0x7) == 2;
            Some(if long { 6 } else { 4 } + ea_cost(ea_of(word)?, long)) // CMP <ea>,Dn
        }
        3 => Some(6 + ea_cost(ea_of(word)?, false)), // CMPA.W
        7 => Some(6 + ea_cost(ea_of(word)?, true)),  // CMPA.L
        opmode @ (4..=6) if mode_bits == 1 => {
            Some(if opmode == 6 { 20 } else { 12 }) // CMPM
        }
        opmode @ (4..=6) => {
            let long = opmode == 6;
            let ea = ea_of(word)?;
            if ea == EaMode::Dn {
                Some(if long { 8 } else { 4 })
            } else {
                Some(if long { 12 } else { 8 } + ea_cost(ea, long)) // EOR Dn,<ea>
            }
        }
        _ => None,
    }
}

fn decode_shift(word: u16) -> Option<u32> {
    let size = (word >> 6) & 0x3;
    if size == 3 {
        return Some(8 + ea_cost(ea_of(word)?, false)); // memory shift
    }
    let long = size == 2;
    let count = if word & 0x0020 != 0 {
        1 // register count: runtime value unknown; assume 1 (documented)
    } else {
        let imm = (word >> 9) & 0x7;
        if imm == 0 {
            8
        } else {
            u32::from(imm)
        }
    };
    Some(if long { 8 } else { 6 } + 2 * count)
}

fn decode_branch(word: u16) -> Option<u32> {
    match (word >> 8) & 0xF {
        0 => Some(10),                                    // bra
        1 => Some(18),                                    // bsr
        _ => Some(if word & 0xFF == 0 { 12 } else { 8 }), // Bcc not-taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P2.md §10.2 pinned vectors — raw (un-rounded) table cycles.
    #[test]
    fn pinned_insn_cycles() {
        let cases: &[(&[u8], u32)] = &[
            (&[0x70, 0x00], 4),                                      // moveq #0,d0
            (&[0x30, 0x01], 4),                                      // move.w d1,d0
            (&[0x30, 0x10], 8),                                      // move.w (a0),d0
            (&[0x20, 0x18], 12),                                     // move.l (a0)+,d0
            (&[0x31, 0xFC, 0x07, 0x77, 0x82, 0x40], 16),             // move.w #imm,abs.w
            (&[0x33, 0xFC, 0x07, 0x77, 0xFF, 0xFF, 0x82, 0x40], 20), // #imm,abs.l
            (&[0xD0, 0x41], 4),                                      // add.w d1,d0
            (&[0xD0, 0x81], 8),                                      // add.l d1,d0
            (&[0xD0, 0x90], 14),                                     // add.l (a0),d0
            (&[0xE7, 0x48], 12),                                     // lsl.w #3,d0
            (&[0x41, 0xF8, 0x82, 0x40], 8),                          // lea abs.w,a0
            (&[0x48, 0x50], 12),                                     // pea (a0)
            (&[0x4E, 0x90], 16),                                     // jsr (a0)
            (&[0x48, 0xE7, 0x80, 0x80], 24),                         // movem.l d0/a0,-(a7)
            (&[0x4C, 0xD7, 0x00, 0x03], 28),                         // movem.l (a7),d0/d1
            (&[0x51, 0xC8, 0xFF, 0xFE], 10),                         // dbf d0 (taken policy)
            (&[0x66, 0xF8], 8),                                      // bne.s
            (&[0x66, 0x00, 0xFF, 0xF8], 12),                         // bne.w
            (&[0x60, 0xFE], 10),                                     // bra.s
            (&[0x4E, 0x75], 16),                                     // rts
            (&[0x42, 0x40], 4),                                      // clr.w d0
            (&[0x42, 0x78, 0x12, 0x34], 16),                         // clr.w abs.w (8+8)
            (&[0x4A, 0x50], 8),                                      // tst.w (a0)
            (&[0xC1, 0xC1], 70),                                     // muls d1,d0
            (&[0x48, 0x40], 4),                                      // swap d0
            (&[0x4E, 0x71], 4),                                      // nop
            (&[0x08, 0x00, 0x00, 0x07], 10),                         // btst #7,d0
        ];
        for (bytes, expected) in cases {
            assert_eq!(insn_cycles(bytes), Some(*expected), "bytes {bytes:x?}");
        }
    }

    #[test]
    fn insn_cycles_illegal_is_none() {
        assert_eq!(insn_cycles(&[0xFF, 0xFF]), None);
    }
}
