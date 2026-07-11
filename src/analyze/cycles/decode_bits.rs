//! The 0x0xxx nibble: static/dynamic bit manipulation (BTST/BCHG/BCLR/
//! BSET) and the ORI/ANDI/SUBI/ADDI/EORI/CMPI immediate group, split out
//! of `decode_misc.rs` (itself already §7.4's one permitted split) to
//! stay under the file-size ceiling — no behavior boundary beyond that.

use super::{ea_cost, ea_fields, ea_mode, EaMode};

pub(super) fn group_0(word: u16) -> Option<u32> {
    if matches!(word, 0x003C | 0x007C | 0x023C | 0x027C | 0x0A3C | 0x0A7C) {
        return Some(20); // andi/ori/eori to ccr/sr
    }
    if word & 0xFF00 == 0x0800 {
        let (m, r) = ea_fields(word);
        return Some(static_bit_cost((word >> 6) & 0x3, ea_mode(m, r)?));
    }
    if word & 0xF100 == 0x0100 {
        let (m, r) = ea_fields(word);
        return Some(dynamic_bit_cost((word >> 6) & 0x3, ea_mode(m, r)?));
    }
    let op = (word >> 9) & 0x7;
    let size = (word >> 6) & 0x3;
    if !matches!(op, 0 | 1 | 2 | 3 | 5 | 6) || size == 3 {
        return None;
    }
    let (m, r) = ea_fields(word);
    Some(immediate_group_cost(ea_mode(m, r)?, size == 2, op == 6))
}

fn static_bit_cost(op: u16, ea: EaMode) -> u32 {
    if ea == EaMode::Dn {
        match op {
            0 => 10,
            1 => 12,
            2 => 14,
            _ => 12,
        }
    } else {
        (if op == 0 { 8 } else { 12 }) + ea_cost(ea, false)
    }
}

fn dynamic_bit_cost(op: u16, ea: EaMode) -> u32 {
    if ea == EaMode::Dn {
        match op {
            0 => 6,
            1 => 8,
            2 => 10,
            _ => 8,
        }
    } else {
        (if op == 0 { 4 } else { 8 }) + ea_cost(ea, false)
    }
}

fn immediate_group_cost(ea: EaMode, long: bool, is_cmpi: bool) -> u32 {
    if ea == EaMode::Dn {
        match (is_cmpi, long) {
            (true, true) => 14,
            (true, false) => 8,
            (false, true) => 16,
            (false, false) => 8,
        }
    } else {
        let (bw, l) = if is_cmpi { (8, 12) } else { (12, 20) };
        (if long { l } else { bw }) + ea_cost(ea, long)
    }
}
