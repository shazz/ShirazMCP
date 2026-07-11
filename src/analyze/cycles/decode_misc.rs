//! §7.4's one permitted split: the 0x4xxx misc bucket (CLR/NEG/TST/JMP/
//! JSR/LEA/PEA/MOVEM/…) and 0x5xxx (ADDQ/SUBQ/Scc/DBcc) opcode nibbles —
//! kept out of `decode.rs` to stay under the file-size ceiling. The
//! 0x0xxx bit-manipulation/immediate nibble is `decode_bits.rs` (same
//! reason, split again off this file).

use super::tables;
use super::{ea_cost, ea_fields, ea_mode, word_at, EaMode};

pub(super) use super::decode_bits::group_0;

pub(super) fn group_4(bytes: &[u8], word: u16) -> Option<u32> {
    if let Some(c) = fixed_4e_cost(word) {
        return Some(c);
    }
    if let Some(c) = jsr_jmp_sr_cost(word) {
        return Some(c);
    }
    if word & 0xFFF8 == 0x4840 {
        return Some(tables::SWAP);
    }
    if word & 0xFFF8 == 0x4880 || word & 0xFFF8 == 0x48C0 {
        return Some(tables::EXT); // EXT.W/EXT.L (mode=000 sub-case)
    }
    if word & 0xFFC0 == 0x4AC0 {
        let (m, r) = ea_fields(word);
        let ea = ea_mode(m, r)?;
        return Some(if ea == EaMode::Dn {
            4
        } else {
            10 + ea_cost(ea, false)
        }); // TAS
    }
    if word & 0xF1C0 == 0x41C0 {
        let (m, r) = ea_fields(word);
        return tables::lea_cost(ea_mode(m, r)?); // LEA (reg field bits11-9 excluded)
    }
    group_4_rest(bytes, word)
}

/// The exact/near-exact 0x4Exx forms: RESET/NOP/STOP/RTE/RTS/TRAPV/RTR
/// (single opcodes), TRAP/LINK/UNLK/MOVE USP (narrow masks) — all fixed
/// costs, no EA involved.
fn fixed_4e_cost(word: u16) -> Option<u32> {
    match word {
        0x4E70 => return Some(tables::RESET),
        0x4E71 => return Some(tables::NOP),
        0x4E72 => return Some(tables::STOP),
        0x4E73 => return Some(tables::RTE),
        0x4E75 => return Some(tables::RTS),
        0x4E76 => return Some(tables::TRAPV),
        0x4E77 => return Some(tables::RTR),
        _ => {}
    }
    if word & 0xFFF0 == 0x4E40 {
        return Some(tables::TRAP);
    }
    if word & 0xFFF8 == 0x4E50 {
        return Some(tables::LINK);
    }
    if word & 0xFFF8 == 0x4E58 {
        return Some(tables::UNLK);
    }
    if word & 0xFFF0 == 0x4E60 {
        return Some(tables::MOVE_USP);
    }
    None
}

/// JSR/JMP (0x4E80/0x4EC0) and MOVE to/from SR/CCR (0x40C0/0x44C0/0x46C0)
/// — the EA-suffixed 0x4Exx/0x40xx forms `fixed_4e_cost` doesn't cover.
fn jsr_jmp_sr_cost(word: u16) -> Option<u32> {
    if word & 0xFFC0 == 0x4E80 {
        let (m, r) = ea_fields(word);
        return tables::jsr_cost(ea_mode(m, r)?);
    }
    if word & 0xFFC0 == 0x4EC0 {
        let (m, r) = ea_fields(word);
        return tables::jmp_cost(ea_mode(m, r)?);
    }
    if word & 0xFFC0 == 0x40C0 {
        let (m, r) = ea_fields(word);
        let ea = ea_mode(m, r)?;
        return Some(if ea == EaMode::Dn {
            6
        } else {
            8 + ea_cost(ea, false)
        });
    }
    if word & 0xFFC0 == 0x44C0 || word & 0xFFC0 == 0x46C0 {
        let (m, r) = ea_fields(word);
        return Some(12 + ea_cost(ea_mode(m, r)?, false));
    }
    None
}

fn group_4_rest(bytes: &[u8], word: u16) -> Option<u32> {
    if word & 0xFFC0 == 0x4840 {
        let (m, r) = ea_fields(word);
        return tables::pea_cost(ea_mode(m, r)?);
    }
    if word & 0xFF00 == 0x4A00 {
        let size = (word >> 6) & 0x3;
        let (m, r) = ea_fields(word);
        return Some(4 + ea_cost(ea_mode(m, r)?, size == 2));
    }
    if word & 0xFFC0 == 0x4800 {
        let (m, r) = ea_fields(word);
        let ea = ea_mode(m, r)?;
        return Some(if ea == EaMode::Dn {
            6
        } else {
            8 + ea_cost(ea, false)
        });
    }
    if matches!(word & 0xFF00, 0x4000 | 0x4200 | 0x4400 | 0x4600) {
        return clr_neg_not_cost(word);
    }
    movem_cost(bytes, word)
}

fn clr_neg_not_cost(word: u16) -> Option<u32> {
    let size = (word >> 6) & 0x3;
    if size == 3 {
        return None;
    }
    let long = size == 2;
    let (m, r) = ea_fields(word);
    let ea = ea_mode(m, r)?;
    if ea == EaMode::Dn {
        Some(if long { 6 } else { 4 })
    } else {
        Some(if long { 12 } else { 8 } + ea_cost(ea, long))
    }
}

/// MOVEM: bits 11/9/8/7 = 1/0/0/1 identify the instruction regardless of
/// direction (bit10: 0=store,1=load) or size (bit6: 0=word,1=long) — both
/// of those must stay OUTSIDE the identity mask/comparison (masking them
/// IN would make the store/load target constants unreachable, since the
/// masked word's bit10 is always forced to the mask's own value).
fn movem_cost(bytes: &[u8], word: u16) -> Option<u32> {
    if word & 0xFB80 != 0x4880 {
        return None;
    }
    let long = word & 0x0040 != 0;
    let per_reg = if long { 8 } else { 4 };
    let mask = word_at(bytes, 2)?;
    let regs = mask.count_ones();
    let (m, r) = ea_fields(word);
    let ea = ea_mode(m, r)?;
    let base = if word & 0x0400 != 0 {
        tables::movem_load_base(ea)?
    } else {
        tables::movem_store_base(ea)?
    };
    Some(base + per_reg * regs)
}

pub(super) fn group_5(word: u16) -> Option<u32> {
    let size = (word >> 6) & 0x3;
    let (mode_bits, reg_bits) = ea_fields(word);
    if size == 3 {
        if mode_bits == 1 {
            return Some(10); // DBcc: taken-loop policy (§7.1)
        }
        let ea = ea_mode(mode_bits, reg_bits)?;
        return Some(if ea == EaMode::Dn {
            6
        } else {
            8 + ea_cost(ea, false)
        });
    }
    let long = size == 2;
    let ea = ea_mode(mode_bits, reg_bits)?;
    Some(match ea {
        EaMode::Dn => {
            if long {
                8
            } else {
                4
            }
        }
        EaMode::An => 8,
        _ => (if long { 12 } else { 8 }) + ea_cost(ea, long),
    })
}
