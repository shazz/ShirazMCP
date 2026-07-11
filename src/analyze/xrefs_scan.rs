//! Byte-level candidate scanning for `xrefs` (P14 §6.2 step 2) — the
//! linear extension-word walk that proposes `(offset, RefMode)` candidates
//! ahead of the confirm-decode pass. Split from `xrefs.rs` purely for the
//! line-ceiling budget (the `atomik_bits.rs`/`rnc_m1.rs` sibling-helper
//! precedent) — `pub(crate)`-consumed, not a registration point.

use crate::analyze::xrefs_ops::RefMode;
use crate::types::MemImage;

/// Hard cap on candidate extension-word offsets examined per scan — low
/// targets (e.g. $4C6) alias ordinary data everywhere; the cap bounds the
/// confirm-decode cost. Exceeding it sets `candidates_capped`.
pub const XREFS_CANDIDATE_CAP: usize = 4096;

pub(crate) fn in_range(addr: u32, t0: u32, t1: u32) -> bool {
    (t0..t1).contains(&(addr & 0xFF_FFFF))
}

fn sign_ext16(w: u16) -> u32 {
    w as i16 as i32 as u32
}

fn be_u16(bytes: &[u8], o: usize) -> Option<u16> {
    bytes
        .get(o..o + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
}

fn be_u32(bytes: &[u8], o: usize) -> Option<u32> {
    bytes
        .get(o..o + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// One linear walk over even offsets, pushing every addressing-mode
/// candidate at each; stops at `XREFS_CANDIDATE_CAP`.
pub(crate) fn collect_candidates(
    mem: &MemImage,
    t0: u32,
    t1: u32,
    start_off: usize,
    end_off: usize,
) -> (Vec<(usize, RefMode)>, bool) {
    let bytes = mem.bytes();
    let base = mem.base().raw();
    let mut out = Vec::new();
    let mut capped = false;
    for o in (start_off..end_off).step_by(2) {
        push_candidates(bytes, o, base, t0, t1, &mut out);
        if out.len() >= XREFS_CANDIDATE_CAP {
            out.truncate(XREFS_CANDIDATE_CAP);
            capped = true;
            break;
        }
    }
    (out, capped)
}

/// Every addressing-mode candidate at extension-word offset `o` (§6.2
/// step 2's four bullet rules).
fn push_candidates(
    bytes: &[u8],
    o: usize,
    base: u32,
    t0: u32,
    t1: u32,
    out: &mut Vec<(usize, RefMode)>,
) {
    if let Some(w32) = be_u32(bytes, o) {
        let top = w32 >> 24;
        if (top == 0x00 || top == 0xFF) && in_range(w32, t0, t1) {
            out.push((o, RefMode::AbsL));
        }
    }
    let Some(w16) = be_u16(bytes, o) else {
        return;
    };
    if in_range(sign_ext16(w16), t0, t1) {
        out.push((o, RefMode::AbsW));
    }
    let here = base.wrapping_add(o as u32);
    if in_range(here.wrapping_add_signed(w16 as i16 as i32), t0, t1) {
        out.push((o, RefMode::PcDisp));
    }
    if w16 & 0x0700 == 0 {
        let disp = (w16 & 0xFF) as u8 as i8 as i32;
        if in_range(here.wrapping_add_signed(disp), t0, t1) {
            out.push((o, RefMode::PcIdx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Addr;

    #[test]
    fn in_range_checks_half_open_interval() {
        assert!(in_range(0x47250, 0x47250, 0x47251));
        assert!(!in_range(0x47251, 0x47250, 0x47251));
        assert!(!in_range(0x47249, 0x47250, 0x47251));
    }

    #[test]
    fn collect_candidates_finds_abs_l() {
        let mem = MemImage::new(vec![0x00, 0x04, 0x72, 0x50], Addr::new(0x20000));
        let (candidates, capped) = collect_candidates(&mem, 0x47250, 0x47251, 0, 4);
        assert!(!capped);
        assert!(candidates.contains(&(0, RefMode::AbsL)));
    }

    #[test]
    fn collect_candidates_empty_window_is_empty() {
        let mem = MemImage::new(vec![0x00, 0x04, 0x72, 0x50], Addr::new(0x20000));
        let (candidates, capped) = collect_candidates(&mem, 0x47250, 0x47251, 0, 0);
        assert!(candidates.is_empty());
        assert!(!capped);
    }
}
