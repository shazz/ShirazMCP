//! Boot-code virus-tell heuristics (§5, ADR-17): disasm-based tells via P2
//! `analyze::disasm`. HEURISTIC, honest by contract — reports tells +
//! confidence < 1.0, never a virus verdict; any decode failure ⇒ no tells,
//! confidence 0.0. // verify live (no real virus corpus run here). Reading a
//! vector is antivirus behaviour; only *writing* a watched vector is a tell
//! (see ZR_BLK.S).

use crate::analyze::disasm::{disasm, Insn};
use crate::types::{Addr, MemImage};

/// 512 bytes / 2-byte minimum instruction — the decode budget for one sector.
pub const BOOT_INSN_MAX: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TellKind {
    VectorWrite,
    SelfCopy,
    DiskTrap,
}

/// One annotated piece of evidence (§5.2) — never a verdict on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tell {
    pub kind: TellKind,
    pub addr: Addr,
    pub insn: String,
    pub note: &'static str,
}

/// Low-memory vectors whose *write* persists/spreads a boot virus (§5.3).
/// `ZR_BLK.S` reads these to detect exactly this tampering.
const WATCHED: &[(u32, &str)] = &[
    (0x426, "resvalid ($426)"),
    (0x42A, "resvector ($42A)"),
    (0x472, "hdv_bpb ($472)"),
    (0x476, "hdv_rw ($476)"),
    (0x47A, "hdv_boot ($47A)"),
    (0x47E, "hdv_mediach ($47E)"),
    (0x4CE, "vbl_list ($4CE)"),
];

const TRAP_OPS: &[&str] = &["#$d", "#13", "#$e", "#14"];

/// Where the boot code starts (§5.1): TOS runs from `$000`; a `bra` there
/// skips the BPB. Any unrecognized/out-of-range shape clamps to `$1E`.
pub fn boot_code_start(sector: &[u8]) -> usize {
    let target: isize = match be16(sector, 0) {
        Some(0x6000) => 2 + isize::from(be16(sector, 2).unwrap_or(0) as i16),
        Some(w) if w >> 8 == 0x60 && !matches!(w & 0xFF, 0x00 | 0xFF) => {
            2 + isize::from((w & 0xFF) as u8 as i8)
        }
        _ => 0x1E,
    };
    if !(0x1E..512).contains(&target) || target % 2 != 0 {
        0x1E
    } else {
        target as usize
    }
}

fn be16(bytes: &[u8], o: usize) -> Option<u16> {
    let b = bytes.get(o..o + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

/// Disassemble the boot code and collect tells + confidence. Never errors:
/// an out-of-range offset or a capstone decode failure degrades to
/// `(vec![], 0.0)`.
pub fn analyze_code(mem: &MemImage, at_off: usize) -> (Vec<Tell>, f32) {
    let Some(off) = u32::try_from(at_off).ok() else {
        return (Vec::new(), 0.0);
    };
    let Some(at_raw) = mem.base().raw().checked_add(off) else {
        return (Vec::new(), 0.0);
    };
    let insns = disasm(mem, Addr::new(at_raw), BOOT_INSN_MAX).unwrap_or_default();
    let mut tells: Vec<Tell> = insns
        .iter()
        .filter_map(|i| vector_write(i).or_else(|| disk_trap(i)))
        .collect();
    if let Some(t) = self_copy(&insns) {
        tells.push(t);
    }
    let confidence = confidence(&tells);
    (tells, confidence)
}

fn confidence(tells: &[Tell]) -> f32 {
    let vector_writes = tells
        .iter()
        .filter(|t| t.kind == TellKind::VectorWrite)
        .count() as f32;
    let self_copy = f32::from(u8::from(tells.iter().any(|t| t.kind == TellKind::SelfCopy)));
    let disk_traps = tells
        .iter()
        .filter(|t| t.kind == TellKind::DiskTrap)
        .count() as f32;
    (0.25 * (vector_writes + self_copy) + 0.10 * disk_traps).min(0.9)
}

/// `move*` whose destination resolves to a watched vector (§5.3) — a write,
/// never a read (`dest_addr` only looks at the destination operand).
fn vector_write(insn: &Insn) -> Option<Tell> {
    if !insn.mnemonic.starts_with("move") {
        return None;
    }
    let dest = dest_addr(&insn.ops)?;
    let (_, note) = WATCHED.iter().find(|(a, _)| *a == dest)?;
    Some(Tell {
        kind: TellKind::VectorWrite,
        addr: insn.addr,
        insn: format!("{} {}", insn.mnemonic, insn.ops),
        note,
    })
}

/// Resolve a destination operand's absolute bus address (§5.3). `None` for
/// a register/indirect destination (`d0`, `(a1)`, …) — the read-safety
/// proof: a *source* watched address never reaches this function.
fn dest_addr(ops: &str) -> Option<u32> {
    let dst = ops.rsplit_once(", ").map_or(ops, |(_, d)| d).trim();
    let dst = dst.strip_prefix('$')?;
    let (hex, suffix) = dst.split_once('.').unwrap_or((dst, ""));
    let addr = u32::from_str_radix(hex, 16).ok()?;
    let addr = if suffix == "w" && addr & 0x8000 != 0 {
        addr | 0xFFFF_0000
    } else {
        addr
    };
    crate::types::Addr::bus24(addr)
}

/// Block-copy loop (§5.4): a post-increment `move` plus a `db*` anywhere in
/// the stream — one tell, anchored at the first post-increment move.
fn self_copy(insns: &[Insn]) -> Option<Tell> {
    let mv = insns
        .iter()
        .find(|i| i.mnemonic.starts_with("move") && i.ops.contains(")+"))?;
    insns
        .iter()
        .any(|i| i.mnemonic.starts_with("db"))
        .then(|| Tell {
            kind: TellKind::SelfCopy,
            addr: mv.addr,
            insn: format!("{} {}", mv.mnemonic, mv.ops),
            note: "block-copy loop (self-relocation)",
        })
}

/// BIOS (`#13`/`#$d`) or XBIOS (`#14`/`#$e`) trap — weak evidence (§5.5).
fn disk_trap(insn: &Insn) -> Option<Tell> {
    if insn.mnemonic != "trap" || !TRAP_OPS.contains(&insn.ops.as_str()) {
        return None;
    }
    Some(Tell {
        kind: TellKind::DiskTrap,
        addr: insn.addr,
        insn: format!("{} {}", insn.mnemonic, insn.ops),
        note: "BIOS/XBIOS trap (possible disk write / vector install)",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MemImage;

    #[test]
    fn boot_code_start_bra_s() {
        let sector = [0x60, 0x2E];
        assert_eq!(boot_code_start(&sector), 0x30);
    }

    #[test]
    fn boot_code_start_bra_w() {
        let sector = [0x60, 0x00, 0x00, 0x40];
        assert_eq!(boot_code_start(&sector), 0x42);
    }

    #[test]
    fn boot_code_start_no_branch_defaults_1e() {
        let sector = [0x4E, 0x71];
        assert_eq!(boot_code_start(&sector), 0x1E);
    }

    #[test]
    fn boot_code_start_clamps_odd() {
        // bra.s +0x01 -> target = 2 + 1 = 3 (odd) -> clamp to 0x1E.
        let sector = [0x60, 0x01];
        assert_eq!(boot_code_start(&sector), 0x1E);
    }

    #[test]
    fn vector_write_flagged() {
        // move.l a0,$472.w
        let code = [0x21, 0xC8, 0x04, 0x72];
        let mem = MemImage::new(code.to_vec(), Addr::new(0x1E));
        let (tells, confidence) = analyze_code(&mem, 0);
        assert_eq!(tells.len(), 1);
        assert_eq!(tells[0].kind, TellKind::VectorWrite);
        assert!(tells[0].note.contains("hdv_bpb"));
        assert!(confidence > 0.0);
    }

    #[test]
    fn vector_read_not_flagged() {
        // move.l $426.w,d0 (the ZR_BLK antivirus read).
        let code = [0x20, 0x38, 0x04, 0x26];
        let mem = MemImage::new(code.to_vec(), Addr::new(0x1E));
        let (tells, confidence) = analyze_code(&mem, 0);
        assert!(tells.is_empty());
        assert_eq!(confidence, 0.0);
    }

    #[test]
    fn self_copy_loop_flagged() {
        let mut code = vec![0x22, 0xD8]; // move.l (a0)+,(a1)+
        code.extend_from_slice(&[0x51, 0xC8, 0xFF, 0xFC]); // dbra d0,-4
        let mem = MemImage::new(code, Addr::new(0x1E));
        let (tells, _) = analyze_code(&mem, 0);
        assert_eq!(
            tells
                .iter()
                .filter(|t| t.kind == TellKind::SelfCopy)
                .count(),
            1
        );
    }

    #[test]
    fn disk_trap_is_weak() {
        let code = [0x4E, 0x4E]; // trap #14
        let mem = MemImage::new(code.to_vec(), Addr::new(0x1E));
        let (tells, confidence) = analyze_code(&mem, 0);
        assert_eq!(tells.len(), 1);
        assert_eq!(tells[0].kind, TellKind::DiskTrap);
        assert!(confidence > 0.0 && confidence <= 0.9);
    }

    #[test]
    fn undecodable_degrades() {
        let mem = MemImage::new(vec![0u8; 4], Addr::new(0));
        // Odd offset -> disasm rejects -> graceful degrade.
        let (tells, confidence) = analyze_code(&mem, 1);
        assert!(tells.is_empty());
        assert_eq!(confidence, 0.0);

        // Out-of-range offset -> graceful degrade.
        let (tells, confidence) = analyze_code(&mem, 1000);
        assert!(tells.is_empty());
        assert_eq!(confidence, 0.0);
    }

    #[test]
    fn confidence_capped() {
        let mut code = Vec::new();
        code.extend_from_slice(&[0x21, 0xC8, 0x04, 0x72]); // move.l a0,$472.w
        code.extend_from_slice(&[0x23, 0xC9, 0x04, 0x2A]); // move.l a1,$42A.w
        code.extend_from_slice(&[0x21, 0xCA, 0x04, 0x76]); // move.l a2,$476.w
        code.extend_from_slice(&[0x21, 0xCB, 0x04, 0x7A]); // move.l a3,$47A.w
        let mem = MemImage::new(code, Addr::new(0x1E));
        let (_, confidence) = analyze_code(&mem, 0);
        assert!(confidence <= 0.9);
    }

    #[test]
    fn dest_addr_abs_short_sign_extend() {
        assert_eq!(dest_addr("a0, $42a.w"), Some(0x42A));
    }

    #[test]
    fn dest_addr_abs_long() {
        assert_eq!(dest_addr("d0, $ffff8800.l"), Some(0xFF8800));
    }

    #[test]
    fn dest_addr_register_none() {
        // Watched addr is SOURCE; dest is the register d0 -> None.
        assert_eq!(dest_addr("$472.w, d0"), None);
    }

    #[test]
    fn disk_trap_token_pin() {
        // trap #14: 0x4E4E. Assert the exact operand text capstone emits;
        // if it changes, update TRAP_OPS to match (§5.5 fallback).
        let code = [0x4E, 0x4E];
        let mem = MemImage::new(code.to_vec(), Addr::new(0));
        let insns = disasm(&mem, Addr::new(0), 1).unwrap();
        assert_eq!(insns[0].mnemonic, "trap");
        assert!(
            TRAP_OPS.contains(&insns[0].ops.as_str()),
            "unexpected trap operand text: {:?} (add it to TRAP_OPS)",
            insns[0].ops
        );
    }
}
