//! Embedded Capstone M68K disassembly over a `MemImage` (ADR-4): bulk,
//! offline, zero emulator round-trips. Hatari's own `disasm` remains the
//! live path (P2 `disasm` tool, source=live).

use capstone::arch::m68k::ArchMode;
use capstone::arch::BuildsCapstone;
use capstone::Capstone;

use crate::error::{Error, Result};
use crate::types::{Addr, MemImage};

/// Longest 68000 instruction (opcode + 4 extension words + …): 10 bytes.
/// Callers size snapshot windows as `count * MAX_INSN_LEN`.
pub const MAX_INSN_LEN: usize = 10;

/// One decoded instruction. `ops` is Capstone's operand text verbatim
/// (e.g. "#$777, $ffff8240.l"); `bytes` the exact instruction bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insn {
    pub addr: Addr,
    pub mnemonic: String,
    pub ops: String,
    pub bytes: Vec<u8>,
}

/// Disassemble up to `count` instructions starting at bus address `at`,
/// which must lie inside `mem` and be even (68000 alignment). Capstone
/// never stalls on undecodable words — it emits `dc.w $xxxx` and
/// continues — so the result length is `count` unless the image ends.
pub fn disasm(mem: &MemImage, at: Addr, count: usize) -> Result<Vec<Insn>> {
    if !at.raw().is_multiple_of(2) {
        return Err(Error::Parse {
            context: "disasm address".to_string(),
            input: at.to_string(),
        });
    }
    let off = (at.raw() as usize)
        .checked_sub(mem.base().raw() as usize)
        .filter(|&o| o < mem.len())
        .ok_or_else(|| Error::Parse {
            context: "disasm range".to_string(),
            input: at.to_string(),
        })?;

    let cs = build_capstone()?;
    let insns = cs
        .disasm_count(&mem.bytes()[off..], u64::from(at.raw()), count)
        .map_err(|e| Error::Parse {
            context: "capstone disasm".to_string(),
            input: e.to_string(),
        })?;
    Ok(insns.iter().map(to_insn).collect())
}

/// Built per call (construction is microseconds); `Capstone` is not
/// `Sync`, so it must never be stored in server state.
fn build_capstone() -> Result<Capstone> {
    Capstone::new()
        .m68k()
        .mode(ArchMode::M68k000)
        .build()
        .map_err(|e| Error::Parse {
            context: "capstone build".to_string(),
            input: e.to_string(),
        })
}

fn to_insn(i: &capstone::Insn) -> Insn {
    Insn {
        addr: Addr::new(i.address() as u32),
        mnemonic: i.mnemonic().unwrap_or("?").to_string(),
        ops: i.op_str().unwrap_or("").to_string(),
        bytes: i.bytes().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §6.2 pinned byte buffer: one instruction of each shape in the
    /// table, concatenated in order, base `$10000`.
    fn pinned_bytes() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&[0x33, 0xFC, 0x07, 0x77, 0xFF, 0xFF, 0x82, 0x40]); // move.w #$777,$ffff8240.l
        v.extend_from_slice(&[0x31, 0xFC, 0x07, 0x77, 0x82, 0x40]); // move.w #$777,$8240.w
        v.extend_from_slice(&[0x70, 0x00]); // moveq #$0,d0
        v.extend_from_slice(&[0x30, 0x10]); // move.w (a0),d0
        v.extend_from_slice(&[0x20, 0x18]); // move.l (a0)+,d0
        v.extend_from_slice(&[0xD0, 0x41]); // add.w d1,d0
        v.extend_from_slice(&[0x41, 0xF8, 0x82, 0x40]); // lea.l $8240.w,a0
        v.extend_from_slice(&[0x48, 0xE7, 0x80, 0x80]); // movem.l d0/a0,-(a7)
        v.extend_from_slice(&[0x51, 0xC8, 0xFF, 0xFE]); // dbra d0,$1001e
        v.extend_from_slice(&[0x66, 0xF8]); // bne.b $1001c
        v.extend_from_slice(&[0x4E, 0x75]); // rts
        v.extend_from_slice(&[0xFF, 0xFF]); // dc.w $ffff
        v
    }

    const EXPECTED: &[(&str, &str)] = &[
        ("move.w", "#$777, $ffff8240.l"),
        ("move.w", "#$777, $8240.w"),
        ("moveq", "#$0, d0"),
        ("move.w", "(a0), d0"),
        ("move.l", "(a0)+, d0"),
        ("add.w", "d1, d0"),
        ("lea.l", "$8240.w, a0"),
        ("movem.l", "d0/a0, -(a7)"),
        ("dbra", "d0, $1001e"),
        ("bne.b", "$1001c"),
        ("rts", ""),
        ("dc.w", "$ffff"),
    ];

    fn image() -> MemImage {
        MemImage::new(pinned_bytes(), Addr::new(0x10000))
    }

    #[test]
    fn disasm_pinned_stream() {
        let mem = image();
        let insns = disasm(&mem, Addr::new(0x10000), EXPECTED.len()).unwrap();
        assert_eq!(insns.len(), EXPECTED.len());
        let mut addr = 0x10000u32;
        for (insn, (mnemonic, ops)) in insns.iter().zip(EXPECTED) {
            assert_eq!(insn.mnemonic, *mnemonic, "at {addr:x}");
            assert_eq!(insn.ops, *ops, "at {addr:x}");
            assert_eq!(insn.addr, Addr::new(addr));
            addr += insn.bytes.len() as u32;
        }
    }

    #[test]
    fn disasm_count_truncates() {
        let mem = image();
        let insns = disasm(&mem, Addr::new(0x10000), 3).unwrap();
        assert_eq!(insns.len(), 3);
        let last = &insns[2];
        assert_eq!(last.addr, Addr::new(0x1000E));
    }

    #[test]
    fn disasm_odd_addr_rejected() {
        let mem = image();
        assert!(matches!(
            disasm(&mem, Addr::new(0x10001), 1),
            Err(Error::Parse { .. })
        ));
    }

    #[test]
    fn disasm_out_of_range() {
        let mem = MemImage::new(vec![0u8; 64], Addr::new(0x10000));
        assert!(matches!(
            disasm(&mem, Addr::new(0x20000), 1),
            Err(Error::Parse { .. })
        ));
    }

    #[test]
    fn disasm_illegal_becomes_dcw() {
        let mem = MemImage::new(vec![0xFF, 0xFF, 0x4E, 0x71], Addr::new(0));
        let insns = disasm(&mem, Addr::new(0), 2).unwrap();
        assert_eq!(insns[0].mnemonic, "dc.w");
        assert_eq!(insns[1].mnemonic, "nop");
    }
}
