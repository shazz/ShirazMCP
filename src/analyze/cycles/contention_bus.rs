//! contention_bus.rs — memory-access (bus) cycle counting per instruction
//! (P18 §3.3): bus = 4·words + operand data accesses, clamped to the
//! rounded total. Mirrors decode.rs's dispatch (nibbles 1-3/6-9/B-E; 0x0/
//! 0x4/0x5 are contention_bus_misc.rs); a gap here falls back to "fully
//! contended", never to "no contention". Prefetch refill at branch targets
//! is deliberately not modelled (≤8 cycles, inside the ±10% honesty band).

use super::contention_bus_misc;
use super::{ea_fields, ea_mode, word_at, EaMode};

/// Bus share of one instruction whose ST-rounded total is `rounded_total`.
pub(super) fn bus_cycles(bytes: &[u8], rounded_total: u32) -> u32 {
    match data_access_cycles(bytes) {
        Some(d) => (4 * (bytes.len() as u32 / 2) + d).min(rounded_total),
        None => rounded_total,
    }
}

/// Operand data reads/writes only (no fetches), or None when the opcode is
/// outside the classified set.
fn data_access_cycles(bytes: &[u8]) -> Option<u32> {
    let word = word_at(bytes, 0)?;
    match word >> 12 {
        0x0 => contention_bus_misc::group_0_data(word),
        0x1..=0x3 => move_data(word),
        0x4 => contention_bus_misc::group_4_data(bytes, word),
        0x5 => contention_bus_misc::group_5_data(word),
        0x6 => Some(if (word >> 8) & 0xF == 1 { 8 } else { 0 }), // bsr pushes
        0x7 if word & 0x0100 == 0 => Some(0),                    // moveq
        0x8 | 0x9 | 0xB | 0xC | 0xD => alu_data(word),
        0xE => shift_data(word),
        _ => None,
    }
}

pub(super) const fn acc(long: bool) -> u32 {
    if long {
        8
    } else {
        4
    }
}

pub(super) fn is_mem(ea: EaMode) -> bool {
    !matches!(ea, EaMode::Dn | EaMode::An | EaMode::Imm)
}

fn ea_of(word: u16) -> Option<EaMode> {
    let (m, r) = ea_fields(word);
    ea_mode(m, r)
}

/// MOVE/MOVEA (nibbles 1-3): both operands counted independently — dst
/// fields mirror decode.rs's `decode_move` exactly (PC-relative/Imm dst is
/// invalid for MOVE and falls to None like decode).
fn move_data(word: u16) -> Option<u32> {
    let long = (word >> 12) == 2;
    let src = ea_of(word)?;
    let dst_mode = ((word >> 6) & 0x7) as u8;
    let dst_reg = ((word >> 9) & 0x7) as u8;
    let dst = ea_mode(dst_mode, dst_reg)?;
    if (dst as usize) >= 9 {
        return None;
    }
    let a = acc(long);
    Some(u32::from(is_mem(src)) * a + u32::from(is_mem(dst)) * a)
}

/// ADD/SUB/AND/OR/CMP/EOR family (0x8/0x9/0xB/0xC/0xD) — dispatches on the
/// nibble like decode.rs's four sibling `decode_*_family` fns.
fn alu_data(word: u16) -> Option<u32> {
    match word >> 12 {
        0x9 | 0xD => add_sub_data(word),
        0x8 => or_family_data(word),
        0xC => and_family_data(word),
        0xB => b_family_data(word),
        _ => None,
    }
}

/// ADDX/SUBX (mem form: `3·acc`, 2 reads + 1 write) or ADDA/SUBA/ADD/SUB —
/// mirrors decode.rs's `decode_add_sub`.
fn add_sub_data(word: u16) -> Option<u32> {
    if word & 0x0130 == 0x0100 {
        let long = ((word >> 6) & 0x3) == 2;
        return Some(if word & 0x8 != 0 { 3 * acc(long) } else { 0 });
    }
    let opmode = ((word >> 6) & 0x7) as u8;
    let ea = ea_of(word)?;
    match opmode {
        3 | 7 => Some(u32::from(is_mem(ea)) * acc(opmode == 7)), // ADDA/SUBA
        _ => ea_dn_dn_ea_data(opmode, ea),
    }
}

/// ABCD/SBCD (opmode 4) or EXG (opmode 5/6, AND-family only) — mirrors
/// decode.rs's `packed_or_exg`.
fn packed_data(word: u16, and_family: bool) -> Option<u32> {
    if word & 0x0100 == 0 || (word >> 3) & 0x3 != 0 {
        return None;
    }
    match (word >> 6) & 0x7 {
        4 => Some(if word & 0x8 != 0 { 12 } else { 0 }), // ABCD/SBCD
        5 | 6 if and_family => Some(0),                  // EXG
        _ => None,
    }
}

fn or_family_data(word: u16) -> Option<u32> {
    if let Some(c) = packed_data(word, false) {
        return Some(c);
    }
    let ea = ea_of(word)?;
    match ((word >> 6) & 0x7) as u8 {
        3 | 7 => Some(u32::from(is_mem(ea)) * 4), // DIVU/DIVS
        opmode => ea_dn_dn_ea_data(opmode, ea),
    }
}

fn and_family_data(word: u16) -> Option<u32> {
    if let Some(c) = packed_data(word, true) {
        return Some(c);
    }
    let ea = ea_of(word)?;
    match ((word >> 6) & 0x7) as u8 {
        3 | 7 => Some(u32::from(is_mem(ea)) * 4), // MULU/MULS
        opmode => ea_dn_dn_ea_data(opmode, ea),
    }
}

/// CMP/CMPA/CMPM/EOR — mirrors decode.rs's `decode_b_family`.
fn b_family_data(word: u16) -> Option<u32> {
    let (mode_bits, _) = ea_fields(word);
    match (word >> 6) & 0x7 {
        opmode @ (0..=2) => Some(u32::from(is_mem(ea_of(word)?)) * acc(opmode == 2)),
        opmode @ (3 | 7) => Some(u32::from(is_mem(ea_of(word)?)) * acc(opmode == 7)),
        opmode @ (4..=6) if mode_bits == 1 => Some(2 * acc(opmode == 6)), // CMPM
        opmode @ (4..=6) => {
            let ea = ea_of(word)?;
            Some(if ea == EaMode::Dn {
                0
            } else {
                2 * acc(opmode == 6)
            }) // EOR
        }
        _ => None,
    }
}

/// Shared `<ea>,Dn` (opmode 0-2, read-only) / `Dn,<ea>` (opmode 4-6, RMW)
/// formula used by ADD/SUB/AND/OR.
fn ea_dn_dn_ea_data(opmode: u8, ea: EaMode) -> Option<u32> {
    match opmode {
        0..=2 => Some(u32::from(is_mem(ea)) * acc(opmode == 2)),
        4..=6 => Some(if is_mem(ea) { 2 * acc(opmode == 6) } else { 0 }),
        _ => None,
    }
}

/// Shift/rotate (0xE): register form (Dn only) never touches the bus;
/// memory form (`size==3`) is a single word RMW — architect ruling (§5.2's
/// table omits this row): 8(1/1) per M68000UM, 2·acc(word)=8, matching the
/// Scc/tas/ALU-mem RMW idiom used elsewhere in this classification.
fn shift_data(word: u16) -> Option<u32> {
    if (word >> 6) & 0x3 == 3 {
        ea_of(word)?; // same validity gate as decode_shift's memory arm
        Some(8)
    } else {
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::cycles::{insn_cycles, round_st};

    fn bus(bytes: &[u8]) -> u32 {
        let total = round_st(insn_cycles(bytes).unwrap_or(4));
        bus_cycles(bytes, total)
    }

    /// P18 §7.1 pinned vectors: (bytes, expected bus).
    #[test]
    fn pinned_bus_vectors() {
        let cases: &[(&[u8], u32)] = &[
            (&[0x70, 0x00], 4),                                      // moveq #0,d0
            (&[0x32, 0xD8], 12),                                     // move.w (a0)+,(a1)+
            (&[0x20, 0x18], 12),                                     // move.l (a0)+,d0
            (&[0x33, 0xFC, 0x07, 0x77, 0xFF, 0xFF, 0x82, 0x40], 20), // move.w #imm,abs.l
            (&[0xD0, 0x41], 4),                                      // add.w d1,d0
            (&[0xD0, 0x90], 12),                                     // add.l (a0),d0
            (&[0xC1, 0xC1], 4),                                      // muls d1,d0
            (&[0xE7, 0x48], 4),                                      // lsl.w #3,d0
            (&[0x41, 0xF8, 0x82, 0x40], 8),                          // lea abs.w,a0
            (&[0x4E, 0x90], 12),                                     // jsr (a0)
            (&[0x4E, 0x75], 12),                                     // rts
            (&[0x48, 0xE7, 0x80, 0x80], 24),                         // movem.l d0/a0,-(a7)
            (&[0x4C, 0xD7, 0x00, 0x03], 28),                         // movem.l (a7),d0/d1
            (&[0x51, 0xC8, 0xFF, 0xFE], 8),                          // dbf d0
            (&[0x66, 0xF8], 4),                                      // bne.s (not-taken)
            (&[0x61, 0x00, 0x00, 0x10], 16),                         // bsr.w
            (&[0x42, 0x50], 12),                                     // clr.w (a0)
            (&[0x4A, 0x50], 8),                                      // tst.w (a0)
            (&[0x52, 0x50], 12),                                     // addq.w #1,(a0)
            (&[0x08, 0x00, 0x00, 0x07], 8),                          // btst #7,d0
            (&[0xFF, 0xFF], 4),                                      // illegal (placeholder)
        ];
        for (bytes, expected) in cases {
            assert_eq!(bus(bytes), *expected, "bytes {bytes:x?}");
        }
    }

    #[test]
    fn unclassified_opcode_falls_back_to_fully_contended() {
        assert_eq!(data_access_cycles(&[0xFF, 0xFF]), None);
        assert_eq!(bus_cycles(&[0xFF, 0xFF], 4), 4);
    }

    /// The decomposition never exceeds the rounded total for any classified
    /// opcode (the `min` clamp in `bus_cycles` is a safety net, not the
    /// normal path — cross-check it against every pinned vector).
    #[test]
    fn unclamped_bus_never_exceeds_rounded_total() {
        let cases: &[&[u8]] = &[
            &[0x32, 0xD8],
            &[0x20, 0x18],
            &[0x33, 0xFC, 0x07, 0x77, 0xFF, 0xFF, 0x82, 0x40],
            &[0xD0, 0x90],
            &[0xC1, 0xC1],
            &[0xE7, 0x48],
            &[0x4E, 0x90],
            &[0x4E, 0x75],
            &[0x48, 0xE7, 0x80, 0x80],
            &[0x4C, 0xD7, 0x00, 0x03],
            &[0x51, 0xC8, 0xFF, 0xFE],
            &[0x61, 0x00, 0x00, 0x10],
        ];
        for bytes in cases {
            let total = round_st(insn_cycles(bytes).unwrap_or(4));
            let words = 4 * (bytes.len() as u32 / 2);
            let data = data_access_cycles(bytes).unwrap_or(total);
            assert!(
                words + data <= total,
                "bytes {bytes:x?}: {words}+{data} > {total}"
            );
        }
    }
}
