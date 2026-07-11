//! contention_bus_misc.rs — bus/data-access cost for opcode nibbles 0x0
//! (bit ops + immediate group), 0x4 (misc: CLR/NEG/TST/JMP/JSR/LEA/PEA/
//! MOVEM/…) and 0x5 (ADDQ/SUBQ/Scc/DBcc). Mirrors decode_bits.rs's
//! `group_0` and decode_misc.rs's `group_4`/`group_5` one-for-one — see
//! contention_bus.rs for the entry point. `acc`/`is_mem` are duplicated
//! here rather than imported from contention_bus.rs: the same independence
//! the decode.rs/decode_misc.rs split already relies on (no peer imports).

use super::{ea_fields, ea_mode, word_at, EaMode};

const fn acc(long: bool) -> u32 {
    if long {
        8
    } else {
        4
    }
}

fn is_mem(ea: EaMode) -> bool {
    !matches!(ea, EaMode::Dn | EaMode::An | EaMode::Imm)
}

// ---- 0x0xxx: bit ops + immediate group (mirrors decode_bits.rs) ----

pub(super) fn group_0_data(word: u16) -> Option<u32> {
    if matches!(word, 0x003C | 0x007C | 0x023C | 0x027C | 0x0A3C | 0x0A7C) {
        return Some(0); // andi/ori/eori to ccr/sr
    }
    if word & 0xFF00 == 0x0800 || word & 0xF100 == 0x0100 {
        let (m, r) = ea_fields(word);
        return Some(bit_op_data((word >> 6) & 0x3, ea_mode(m, r)?));
    }
    let op = (word >> 9) & 0x7;
    let size = (word >> 6) & 0x3;
    if !matches!(op, 0 | 1 | 2 | 3 | 5 | 6) || size == 3 {
        return None;
    }
    let (m, r) = ea_fields(word);
    Some(immediate_data(ea_mode(m, r)?, size == 2, op == 6))
}

/// btst=4 (read); bchg/bclr/bset=8 (RMW byte); register dest=0 (internal).
fn bit_op_data(op: u16, ea: EaMode) -> u32 {
    if ea == EaMode::Dn {
        0
    } else if op == 0 {
        4
    } else {
        8
    }
}

/// addi/subi/andi/ori/eori mem: RMW (`2·acc`); cmpi mem: read-only (`acc`);
/// any Dn dest: 0 (internal only).
fn immediate_data(ea: EaMode, long: bool, is_cmpi: bool) -> u32 {
    if ea == EaMode::Dn {
        0
    } else if is_cmpi {
        acc(long)
    } else {
        2 * acc(long)
    }
}

// ---- 0x4xxx: misc bucket (mirrors decode_misc.rs's group_4) ----

pub(super) fn group_4_data(bytes: &[u8], word: u16) -> Option<u32> {
    if let Some(c) = fixed_4e_data(word) {
        return Some(c);
    }
    if let Some(c) = jsr_jmp_sr_data(word) {
        return Some(c);
    }
    if matches!(word & 0xFFF8, 0x4840 | 0x4880 | 0x48C0) {
        return Some(0); // swap / ext.w / ext.l
    }
    if word & 0xFFC0 == 0x4AC0 {
        let (m, r) = ea_fields(word);
        return Some(if ea_mode(m, r)? == EaMode::Dn { 0 } else { 8 }); // TAS
    }
    if word & 0xF1C0 == 0x41C0 {
        return Some(0); // LEA
    }
    group_4_rest_data(bytes, word)
}

/// RESET/NOP/STOP/RTE/RTS/TRAPV/RTR/TRAP/LINK/UNLK/MOVE USP — fixed costs.
fn fixed_4e_data(word: u16) -> Option<u32> {
    match word {
        0x4E70 | 0x4E71 | 0x4E72 | 0x4E76 => return Some(0), // reset/nop/stop/trapv
        0x4E73 | 0x4E77 => return Some(12),                  // rte/rtr
        0x4E75 => return Some(8),                            // rts
        _ => {}
    }
    if word & 0xFFF0 == 0x4E40 {
        return Some(20); // trap: PC.l+SR.w push + vector read
    }
    if matches!(word & 0xFFF8, 0x4E50 | 0x4E58) {
        return Some(8); // link/unlk
    }
    if word & 0xFFF0 == 0x4E60 {
        return Some(0); // move usp
    }
    None
}

/// JSR/JMP and MOVE to/from SR/CCR — the EA-suffixed forms `fixed_4e_data`
/// doesn't cover.
fn jsr_jmp_sr_data(word: u16) -> Option<u32> {
    if word & 0xFFC0 == 0x4E80 {
        return Some(8); // JSR pushes the return PC
    }
    if word & 0xFFC0 == 0x4EC0 {
        return Some(0); // JMP
    }
    if word & 0xFFC0 == 0x40C0 {
        let (m, r) = ea_fields(word);
        return Some(if ea_mode(m, r)? == EaMode::Dn { 0 } else { 8 }); // move from SR (dest)
    }
    if matches!(word & 0xFFC0, 0x44C0 | 0x46C0) {
        let (m, r) = ea_fields(word);
        return Some(if ea_mode(m, r)? == EaMode::Dn { 0 } else { 4 }); // move to ccr/sr (src)
    }
    None
}

fn group_4_rest_data(bytes: &[u8], word: u16) -> Option<u32> {
    if word & 0xFFC0 == 0x4840 {
        return Some(8); // PEA
    }
    if word & 0xFF00 == 0x4A00 {
        let size = (word >> 6) & 0x3;
        let (m, r) = ea_fields(word);
        let ea = ea_mode(m, r)?;
        return Some(if is_mem(ea) { acc(size == 2) } else { 0 }); // TST
    }
    if word & 0xFFC0 == 0x4800 {
        let (m, r) = ea_fields(word);
        return Some(if ea_mode(m, r)? == EaMode::Dn { 0 } else { 8 }); // NBCD
    }
    if matches!(word & 0xFF00, 0x4000 | 0x4200 | 0x4400 | 0x4600) {
        return clr_neg_not_data(word);
    }
    movem_data(bytes, word)
}

fn clr_neg_not_data(word: u16) -> Option<u32> {
    let size = (word >> 6) & 0x3;
    if size == 3 {
        return None;
    }
    let (m, r) = ea_fields(word);
    let ea = ea_mode(m, r)?;
    Some(if is_mem(ea) { 2 * acc(size == 2) } else { 0 })
}

/// MOVEM: `n` = register-list popcount; store is n writes, load is n+1
/// reads (the extra read matches decode_misc.rs's `movem_cost` mask note).
fn movem_data(bytes: &[u8], word: u16) -> Option<u32> {
    if word & 0xFB80 != 0x4880 {
        return None;
    }
    let long = word & 0x0040 != 0;
    let mask = word_at(bytes, 2)?;
    let n = mask.count_ones();
    let per_reg = acc(long);
    Some(if word & 0x0400 != 0 {
        n * per_reg + 4 // load
    } else {
        n * per_reg // store
    })
}

// ---- 0x5xxx: ADDQ/SUBQ/Scc/DBcc (mirrors decode_misc.rs's group_5) ----

pub(super) fn group_5_data(word: u16) -> Option<u32> {
    let size = (word >> 6) & 0x3;
    let (mode_bits, reg_bits) = ea_fields(word);
    if size == 3 {
        if mode_bits == 1 {
            return Some(0); // DBcc
        }
        let ea = ea_mode(mode_bits, reg_bits)?;
        return Some(if is_mem(ea) { 8 } else { 0 }); // Scc
    }
    let ea = ea_mode(mode_bits, reg_bits)?;
    Some(if is_mem(ea) { 2 * acc(size == 2) } else { 0 }) // addq/subq
}
