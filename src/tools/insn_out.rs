//! `InsnOut` + the shared instruction-mapping helper used by both the
//! `disasm` snapshot path (P2) and `patch` readback (P8). Split out of
//! `analyze_wire.rs` (not new logic — a straight extraction) to stay under
//! the file-size ceiling: with `insns_out` inlined, `analyze_wire.rs` would
//! land over 200 non-test lines (§7.2 fallback).

use schemars::JsonSchema;
use serde::Serialize;

use super::analyze_wire::{annotate_ops, track_psg};
use super::mem::scan::hex_encode;
use super::mem::types::AnnotationOut;
use crate::analyze::{self, cycles};
use crate::types::Addr;

#[derive(Debug, Serialize, JsonSchema)]
pub struct InsnOut {
    pub addr: String,      // "$00D106"
    pub mnemonic: String,  // "move.w" / live: first asm token
    pub ops: String,       // capstone op_str / live: rest of asm text
    pub bytes_hex: String, // lowercase, no spaces
    /// Static 68000 cycle estimate for this instruction (ST-rounded);
    /// None when the instruction is outside the cycle model. Snapshot
    /// source only.
    pub cycles_est: Option<u32>,
    pub annotations: Vec<AnnotationOut>,
}

/// Map decoded instructions to wire InsnOut (cycle estimate + PSG-aware
/// hardware annotations) and compute the next address. Shared by the
/// disasm tool (snapshot source) and patch readback (P8).
pub(crate) fn insns_out(insns: &[analyze::disasm::Insn], base: Addr) -> (Vec<InsnOut>, Addr) {
    let mut psg = None;
    let mut next = base;
    let out = insns
        .iter()
        .map(|i| {
            psg = track_psg(psg, &i.mnemonic, &i.ops);
            next = Addr::new(i.addr.raw() + i.bytes.len() as u32);
            InsnOut {
                addr: i.addr.to_string(),
                mnemonic: i.mnemonic.clone(),
                ops: i.ops.clone(),
                bytes_hex: hex_encode(&i.bytes),
                cycles_est: cycles::insn_cycles(&i.bytes).map(cycles::round_st),
                annotations: annotate_ops(&i.ops, psg),
            }
        })
        .collect();
    (out, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::disasm::Insn;

    #[test]
    fn insns_out_maps_and_advances() {
        let insns = vec![Insn {
            addr: Addr::new(0x1000),
            mnemonic: "nop".to_string(),
            ops: String::new(),
            bytes: vec![0x4E, 0x71],
        }];
        let (out, next) = insns_out(&insns, Addr::new(0x1000));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].mnemonic, "nop");
        assert_eq!(out[0].bytes_hex, "4e71");
        assert_eq!(next, Addr::new(0x1002));
    }

    #[test]
    fn insns_out_empty_keeps_base() {
        let (out, next) = insns_out(&[], Addr::new(0x2000));
        assert!(out.is_empty());
        assert_eq!(next, Addr::new(0x2000));
    }
}
