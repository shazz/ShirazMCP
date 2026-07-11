//! Static data cross-references over a `MemImage` (P14): which
//! instructions reference a target address via statically-resolvable
//! addressing modes (abs.l / abs.w / d16(pc) / d8(pc,ix)). Pure over
//! `types` + `analyze::disasm` — never touches the emulator. Register-
//! indirect references are invisible to this scan by design (use the
//! watch tool for those).

use std::collections::BTreeMap;

use crate::analyze::disasm::{self, Insn};
use crate::analyze::xrefs_ops::{classify, resolve_operands, split_operands};
pub use crate::analyze::xrefs_ops::{RefAccess, RefMode};
pub use crate::analyze::xrefs_scan::XREFS_CANDIDATE_CAP;
use crate::analyze::xrefs_scan::{collect_candidates, in_range};
use crate::error::{Error, Result};
use crate::types::{Addr, MemImage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xref {
    pub insn: Insn,
    /// The resolved operand address that fell inside the target range.
    pub ref_addr: Addr,
    pub mode: RefMode,
    pub access: RefAccess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrefScan {
    /// Sorted by insn addr, deduplicated, truncated to `limit`.
    pub hits: Vec<Xref>,
    /// More hits existed than `limit`.
    pub truncated: bool,
    /// The candidate pass hit XREFS_CANDIDATE_CAP — results may be partial.
    pub candidates_capped: bool,
}

/// Best-effort identification of the instruction that ENDED at `pc` (the
/// watch tool's "who wrote it": §3.1 fact 3 — the hit PC is the
/// instruction after the writer). `references_target` = its operands
/// resolve into `[target, target + width)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriterGuess {
    pub insn: Insn,
    pub references_target: bool,
}

/// Scan `[from, from + len)` of `mem` for instructions referencing any
/// address in `[target, target + target_len)`.
pub fn xrefs(
    mem: &MemImage,
    target: Addr,
    target_len: u32,
    from: Addr,
    len: usize,
    limit: usize,
) -> Result<XrefScan> {
    let start_off = from
        .raw()
        .checked_sub(mem.base().raw())
        .filter(|&o| (o as usize) <= mem.len())
        .ok_or_else(|| Error::Parse {
            context: "xrefs range".to_string(),
            input: from.to_string(),
        })? as usize;
    let end_off = (start_off + len).min(mem.len());
    let t0 = target.raw();
    let t1 = t0 + target_len;
    let (candidates, candidates_capped) = collect_candidates(mem, t0, t1, start_off, end_off);
    let mut hits: BTreeMap<u32, Xref> = BTreeMap::new();
    for (o, expected) in candidates {
        if let Some(xref) = confirm(mem, o, expected, t0, t1) {
            hits.entry(xref.insn.addr.raw()).or_insert(xref);
        }
    }
    let truncated = hits.len() > limit;
    Ok(XrefScan {
        hits: hits.into_values().take(limit).collect(),
        truncated,
        candidates_capped,
    })
}

/// Best-effort: for `delta in [2,4,6,8,10]`, try the instruction starting
/// at `pc - delta`; the first candidate whose bytes end exactly at `pc` AND
/// references `[target, target+width)` wins outright; failing that, the
/// first candidate ending at `pc` at all (non-referencing) is the fallback.
pub fn writer_at(mem: &MemImage, pc: Addr, target: Addr, width: u32) -> Option<WriterGuess> {
    let t0 = target.raw();
    let t1 = t0 + width;
    let mut fallback: Option<Insn> = None;
    for delta in [2u32, 4, 6, 8, 10] {
        let Some(start) = pc.raw().checked_sub(delta) else {
            continue;
        };
        if start < mem.base().raw() {
            continue;
        }
        let Some(insn) = ending_at(mem, start, pc.raw()) else {
            continue;
        };
        if resolve_operands(&insn)
            .iter()
            .any(|(a, _, _)| in_range(*a, t0, t1))
        {
            return Some(WriterGuess {
                insn,
                references_target: true,
            });
        }
        fallback.get_or_insert(insn);
    }
    fallback.map(|insn| WriterGuess {
        insn,
        references_target: false,
    })
}

/// One decode at `start`, kept only if its bytes end exactly at `end`.
fn ending_at(mem: &MemImage, start: u32, end: u32) -> Option<Insn> {
    let insn = disasm::disasm(mem, Addr::new(start), 1)
        .ok()?
        .into_iter()
        .next()?;
    if start + insn.bytes.len() as u32 == end {
        Some(insn)
    } else {
        None
    }
}

/// Nearest-instruction-start-first confirm decode for one candidate.
fn confirm(mem: &MemImage, o: usize, expected: RefMode, t0: u32, t1: u32) -> Option<Xref> {
    let ext_width = if expected == RefMode::AbsL { 4 } else { 2 };
    for delta in [2usize, 4, 6, 8] {
        let Some(start_off) = o.checked_sub(delta) else {
            continue;
        };
        if let Some(xref) = try_confirm_at(mem, start_off, o, ext_width, expected, t0, t1) {
            return Some(xref);
        }
    }
    None
}

fn try_confirm_at(
    mem: &MemImage,
    start_off: usize,
    o: usize,
    ext_width: usize,
    expected: RefMode,
    t0: u32,
    t1: u32,
) -> Option<Xref> {
    let start_addr = Addr::new(mem.base().raw() + start_off as u32);
    let insn = disasm::disasm(mem, start_addr, 1)
        .ok()?
        .into_iter()
        .next()?;
    if start_off + insn.bytes.len() < o + ext_width {
        return None;
    }
    let n_ops = split_operands(&insn.ops).len();
    let (ref_addr, mode, op_index) = resolve_operands(&insn)
        .into_iter()
        .find(|(a, m, _)| *m == expected && in_range(*a, t0, t1))?;
    let access = classify(&insn, op_index, n_ops);
    Some(Xref {
        insn,
        ref_addr: Addr::new(ref_addr),
        mode,
        access,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §9.1 pinned fixture, base `$20000`.
    fn fixture() -> MemImage {
        let mut v = Vec::new();
        v.extend_from_slice(&[0x20, 0x39, 0x00, 0x04, 0x72, 0x50]); // move.l $47250.l,d0
        v.extend_from_slice(&[0x23, 0xC0, 0x00, 0x04, 0x72, 0x50]); // move.l d0,$47250.l
        v.extend_from_slice(&[0x41, 0xFA, 0x00, 0x0E]); // lea.l $2001c(pc),a0
        v.extend_from_slice(&[0x31, 0xC0, 0x12, 0x34]); // move.w d0,$1234.w
        v.extend_from_slice(&[0xB0, 0x78, 0x12, 0x34]); // cmp.w $1234.w,d0
        v.extend_from_slice(&[0x4E, 0xB9, 0x00, 0x04, 0x72, 0x50]); // jsr $47250.l
        v.extend_from_slice(&[0x4E, 0x75]); // rts
        MemImage::new(v, Addr::new(0x20000))
    }

    #[test]
    fn xrefs_abs_l_read_write_exec() {
        let mem = fixture();
        let scan = xrefs(
            &mem,
            Addr::new(0x47250),
            1,
            Addr::new(0x20000),
            mem.len(),
            32,
        )
        .unwrap();
        assert!(!scan.truncated);
        assert_eq!(scan.hits.len(), 3);
        assert_eq!(scan.hits[0].insn.addr, Addr::new(0x20000));
        assert_eq!(scan.hits[0].mode, RefMode::AbsL);
        assert_eq!(scan.hits[0].access, RefAccess::Read);
        assert_eq!(scan.hits[0].ref_addr, Addr::new(0x47250));
        assert_eq!(scan.hits[1].insn.addr, Addr::new(0x20006));
        assert_eq!(scan.hits[1].access, RefAccess::Write);
        assert_eq!(scan.hits[2].insn.addr, Addr::new(0x20018));
        assert_eq!(scan.hits[2].access, RefAccess::Exec);
    }

    #[test]
    fn xrefs_abs_w_write_and_cmp_read() {
        let mem = fixture();
        let scan = xrefs(
            &mem,
            Addr::new(0x1234),
            1,
            Addr::new(0x20000),
            mem.len(),
            32,
        )
        .unwrap();
        assert_eq!(scan.hits.len(), 2);
        assert_eq!(scan.hits[0].insn.addr, Addr::new(0x20010));
        assert_eq!(scan.hits[0].mode, RefMode::AbsW);
        assert_eq!(scan.hits[0].access, RefAccess::Write);
        assert_eq!(scan.hits[1].insn.addr, Addr::new(0x20014));
        assert_eq!(scan.hits[1].access, RefAccess::Read);
    }

    #[test]
    fn xrefs_pc_relative() {
        let mem = fixture();
        let scan = xrefs(
            &mem,
            Addr::new(0x2001C),
            1,
            Addr::new(0x20000),
            mem.len(),
            32,
        )
        .unwrap();
        assert_eq!(scan.hits.len(), 1);
        assert_eq!(scan.hits[0].insn.addr, Addr::new(0x2000C));
        assert_eq!(scan.hits[0].mode, RefMode::PcDisp);
        assert_eq!(scan.hits[0].access, RefAccess::Ea);
    }

    #[test]
    fn xrefs_target_range_widens() {
        let mem = fixture();
        let scan = xrefs(
            &mem,
            Addr::new(0x4724E),
            4,
            Addr::new(0x20000),
            mem.len(),
            32,
        )
        .unwrap();
        assert_eq!(scan.hits.len(), 3);
    }

    #[test]
    fn xrefs_no_hits() {
        let mem = fixture();
        let scan = xrefs(
            &mem,
            Addr::new(0x99999),
            1,
            Addr::new(0x20000),
            mem.len(),
            32,
        )
        .unwrap();
        assert_eq!(scan.hits.len(), 0);
        assert!(!scan.truncated);
        assert!(!scan.candidates_capped);
    }

    #[test]
    fn xrefs_limit_truncates() {
        let mem = fixture();
        let scan = xrefs(
            &mem,
            Addr::new(0x47250),
            1,
            Addr::new(0x20000),
            mem.len(),
            1,
        )
        .unwrap();
        assert_eq!(scan.hits.len(), 1);
        assert_eq!(scan.hits[0].insn.addr, Addr::new(0x20000));
        assert!(scan.truncated);
    }

    #[test]
    fn xrefs_scan_window_bounds() {
        let mem = fixture();
        let scan = xrefs(&mem, Addr::new(0x47250), 1, Addr::new(0x20010), 16, 32).unwrap();
        assert_eq!(scan.hits.len(), 1);
        assert_eq!(scan.hits[0].insn.addr, Addr::new(0x20018));
    }

    #[test]
    fn xrefs_bad_range_is_parse_error() {
        let mem = fixture();
        assert!(matches!(
            xrefs(&mem, Addr::new(0x47250), 1, Addr::new(0x10), 16, 32),
            Err(Error::Parse { .. })
        ));
    }

    #[test]
    fn writer_at_prefers_target_reference() {
        let mem = fixture();
        let guess = writer_at(&mem, Addr::new(0x2000C), Addr::new(0x47250), 4).unwrap();
        assert_eq!(guess.insn.addr, Addr::new(0x20006));
        assert!(guess.references_target);
    }

    #[test]
    fn writer_at_fallback_non_referencing() {
        let mem = fixture();
        let guess = writer_at(&mem, Addr::new(0x20014), Addr::new(0x99999), 2).unwrap();
        assert_eq!(guess.insn.addr, Addr::new(0x20010));
        assert!(!guess.references_target);
    }

    #[test]
    fn writer_at_none_at_buffer_start() {
        let mem = fixture();
        assert!(writer_at(&mem, Addr::new(0x20000), Addr::new(0x47250), 2).is_none());
    }
}
