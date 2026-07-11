//! The memory-difference search engine (P15): seed a candidate set from
//! one snapshot, then iteratively filter it against snapshot pairs. Pure
//! over `MemImage` + `CandidateSet` — NEVER touches the emulator; fully
//! unit-tested on synthetic buffers. Big-endian values; word/long
//! candidates are even-aligned (a 68000 word access to an odd address is
//! an address error — such a variable cannot exist).

use crate::analyze::cheat_set::CandidateSet;
use crate::types::MemImage;

/// Candidate value width. `repr` chosen so `bytes()` is trivially const.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Byte,
    Word,
    Long,
}

impl Width {
    pub const fn bytes(self) -> usize {
        match self {
            Self::Byte => 1,
            Self::Word => 2,
            Self::Long => 4,
        }
    }

    /// Max unsigned value representable (0xFF / 0xFFFF / 0xFFFF_FFFF).
    pub const fn max_value(self) -> u32 {
        match self {
            Self::Byte => 0xFF,
            Self::Word => 0xFFFF,
            Self::Long => 0xFFFF_FFFF,
        }
    }

    /// 1|2|4 → Some(width), else None (wire layer maps None → invalid_params).
    pub const fn from_bytes(n: u32) -> Option<Self> {
        match n {
            1 => Some(Self::Byte),
            2 => Some(Self::Word),
            4 => Some(Self::Long),
            _ => None,
        }
    }
}

/// Filter predicates — semantics pinned in P15.md §4's table, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Predicate {
    Changed,
    Unchanged,
    Increased,
    Decreased,
    Eq(u32),
    Ne(u32),
    Delta(i64),
}

/// The only predicates legal at seed time (no previous snapshot exists) —
/// a separate type so "changed on new" is unrepresentable past the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedFilter {
    Eq(u32),
    Ne(u32),
}

/// Big-endian unsigned value of `width` bytes at `off`.
/// Precondition (caller-guaranteed, debug_assert): off + width.bytes() <= bytes.len().
pub fn read_value(bytes: &[u8], off: usize, width: Width) -> u32 {
    debug_assert!(off + width.bytes() <= bytes.len());
    let w = &bytes[off..off + width.bytes()];
    match width {
        Width::Byte => w[0] as u32,
        Width::Word => u16::from_be_bytes([w[0], w[1]]) as u32,
        Width::Long => u32::from_be_bytes([w[0], w[1], w[2], w[3]]),
    }
}

/// Seed a candidate set from `mem`: every offset `o` in
/// `[start_off, end_off)` with `o + width.bytes() <= end_off`, where
/// `mem.base().raw() + o` is even for Word/Long (any parity for Byte),
/// and — if `f` is Some — the §4 seed predicate holds for the value at `o`.
/// `start_off`/`end_off` are snapshot offsets already clamped to
/// `[0, mem.len()]` by the caller (debug_assert both).
pub fn seed(
    mem: &MemImage,
    width: Width,
    f: Option<SeedFilter>,
    start_off: usize,
    end_off: usize,
) -> CandidateSet {
    debug_assert!(start_off <= mem.len() && end_off <= mem.len());
    let mut set = CandidateSet::new(mem.len());
    let step = if width == Width::Byte { 1 } else { 2 };
    let first = first_aligned_offset(mem, width, start_off);
    let bytes = mem.bytes();
    for o in (first..end_off).step_by(step) {
        if o + width.bytes() > end_off {
            break;
        }
        if seed_predicate_holds(f, bytes, o, width) {
            set.insert(o);
        }
    }
    set
}

fn first_aligned_offset(mem: &MemImage, width: Width, start_off: usize) -> usize {
    let abs_odd =
        width != Width::Byte && !(mem.base().raw() as usize + start_off).is_multiple_of(2);
    start_off + abs_odd as usize
}

fn seed_predicate_holds(f: Option<SeedFilter>, bytes: &[u8], o: usize, width: Width) -> bool {
    match f {
        None => true,
        Some(SeedFilter::Eq(x)) => read_value(bytes, o, width) == x,
        Some(SeedFilter::Ne(x)) => read_value(bytes, o, width) != x,
    }
}

/// One narrowing step: drop every candidate that does not satisfy `pred`
/// for (old, new) per §4. Preconditions (caller-validated, debug_assert):
/// old.len() == new.len() == set.capacity(), old.base() == new.base().
pub fn filter(
    set: &mut CandidateSet,
    old: &MemImage,
    new: &MemImage,
    width: Width,
    pred: Predicate,
) {
    debug_assert_eq!(old.len(), new.len());
    debug_assert_eq!(old.len(), set.capacity());
    debug_assert_eq!(old.base(), new.base());
    let (ob, nb) = (old.bytes(), new.bytes());
    set.retain(|o| eval(pred, read_value(ob, o, width), read_value(nb, o, width)));
}

fn eval(pred: Predicate, old: u32, new: u32) -> bool {
    match pred {
        Predicate::Changed => new != old,
        Predicate::Unchanged => new == old,
        Predicate::Increased => new > old,
        Predicate::Decreased => new < old,
        Predicate::Eq(x) => new == x,
        Predicate::Ne(x) => new != x,
        Predicate::Delta(n) => (new as i64) - (old as i64) == n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Addr;

    fn mem(bytes: &[u8]) -> MemImage {
        MemImage::new(bytes.to_vec(), Addr::new(0))
    }

    fn mem_at(bytes: &[u8], base: u32) -> MemImage {
        MemImage::new(bytes.to_vec(), Addr::new(base))
    }

    #[test]
    fn width_roundtrip_and_max() {
        assert_eq!(Width::from_bytes(1), Some(Width::Byte));
        assert_eq!(Width::from_bytes(2), Some(Width::Word));
        assert_eq!(Width::from_bytes(4), Some(Width::Long));
        assert_eq!(Width::from_bytes(3), None);
        assert_eq!(Width::Byte.max_value(), 0xFF);
        assert_eq!(Width::Word.max_value(), 0xFFFF);
        assert_eq!(Width::Long.max_value(), 0xFFFF_FFFF);
    }

    #[test]
    fn read_value_widths() {
        let b = [0xDE, 0xAD, 0xBE, 0xEF];
        assert_eq!(read_value(&b, 0, Width::Byte), 0xDE);
        assert_eq!(read_value(&b, 0, Width::Word), 0xDEAD);
        assert_eq!(read_value(&b, 2, Width::Word), 0xBEEF);
        assert_eq!(read_value(&b, 0, Width::Long), 0xDEADBEEF);
    }

    #[test]
    fn seed_counts_by_width() {
        let m = mem(&[0u8; 8]);
        assert_eq!(seed(&m, Width::Byte, None, 0, 8).count(), 8);
        assert_eq!(seed(&m, Width::Word, None, 0, 8).count(), 4);
        assert_eq!(seed(&m, Width::Long, None, 0, 8).count(), 3);
    }

    #[test]
    fn seed_alignment_uses_bus_address() {
        let m = mem_at(&[0u8; 8], 1);
        let set = seed(&m, Width::Word, None, 0, 8);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![1, 3, 5]);
    }

    #[test]
    fn seed_region_bounds() {
        let m = mem(&[0u8; 16]);
        let set = seed(&m, Width::Word, None, 4, 12);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![4, 6, 8, 10]);
    }

    #[test]
    fn seed_eq_ne() {
        let m = mem(&[0x00, 0x03, 0x00, 0x05, 0x00, 0x03]);
        let eq = seed(&m, Width::Word, Some(SeedFilter::Eq(3)), 0, 6);
        assert_eq!(eq.iter().collect::<Vec<_>>(), vec![0, 4]);
        let ne = seed(&m, Width::Word, Some(SeedFilter::Ne(3)), 0, 6);
        assert_eq!(ne.iter().collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn filter_changed_unchanged() {
        let old = mem(&[1, 2, 3, 4]);
        let new = mem(&[1, 9, 3, 4]);
        let mut s = seed(&old, Width::Byte, None, 0, 4);
        filter(&mut s, &old, &new, Width::Byte, Predicate::Changed);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![1]);

        let mut s = seed(&old, Width::Byte, None, 0, 4);
        filter(&mut s, &old, &new, Width::Byte, Predicate::Unchanged);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![0, 2, 3]);
    }

    #[test]
    fn filter_increased_decreased_unsigned() {
        let old = mem(&[0xFF, 0xFF, 0x00, 0x10]);
        let new = mem(&[0x00, 0x00, 0x00, 0x20]);
        let mut s = seed(&old, Width::Word, None, 0, 4);
        filter(&mut s, &old, &new, Width::Word, Predicate::Increased);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![2]);

        let mut s = seed(&old, Width::Word, None, 0, 4);
        filter(&mut s, &old, &new, Width::Word, Predicate::Decreased);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn filter_eq_ne_use_new_value() {
        let old = mem(&[7]);
        let new = mem(&[9]);
        let mut s = seed(&old, Width::Byte, None, 0, 1);
        filter(&mut s, &old, &new, Width::Byte, Predicate::Eq(9));
        assert_eq!(s.count(), 1);

        let mut s = seed(&old, Width::Byte, None, 0, 1);
        filter(&mut s, &old, &new, Width::Byte, Predicate::Eq(7));
        assert_eq!(s.count(), 0);

        let mut s = seed(&old, Width::Byte, None, 0, 1);
        filter(&mut s, &old, &new, Width::Byte, Predicate::Ne(9));
        assert_eq!(s.count(), 0);
    }

    #[test]
    fn filter_delta_exact_no_wrap() {
        let old = mem(&[0xFF, 0x10]);
        let new = mem(&[0x00, 0x11]);
        let mut s = seed(&old, Width::Byte, None, 0, 2);
        filter(&mut s, &old, &new, Width::Byte, Predicate::Delta(-255));
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![0]);

        let mut s = seed(&old, Width::Byte, None, 0, 2);
        filter(&mut s, &old, &new, Width::Byte, Predicate::Delta(1));
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![1]);

        let mut s = seed(&old, Width::Byte, None, 0, 2);
        filter(&mut s, &old, &new, Width::Byte, Predicate::Delta(0));
        assert_eq!(s.count(), 0);
    }

    #[test]
    fn filter_delta_zero_is_unchanged() {
        let old = mem(&[5, 6]);
        let new = mem(&[5, 6]);
        let mut s = seed(&old, Width::Byte, None, 0, 2);
        filter(&mut s, &old, &new, Width::Byte, Predicate::Delta(0));
        assert_eq!(s.count(), 2);
    }

    #[test]
    fn filter_long_boundary() {
        let old = mem(&[0u8; 8]);
        let mut new_bytes = [0u8; 8];
        new_bytes[7] = 1;
        let new = mem(&new_bytes);
        let mut s = seed(&old, Width::Long, None, 0, 8);
        filter(&mut s, &old, &new, Width::Long, Predicate::Changed);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![4]);
    }

    #[test]
    fn narrowing_sequence_converges() {
        let mut s0 = vec![0u8; 32];
        write_word(&mut s0, 10, 100);
        write_word(&mut s0, 20, 5);
        let mut s1 = s0.clone();
        write_word(&mut s1, 10, 101);
        write_word(&mut s1, 20, 6);
        let mut s2 = s1.clone();
        write_word(&mut s2, 10, 102);

        let (m0, m1, m2) = (mem(&s0), mem(&s1), mem(&s2));
        let mut set = seed(&m0, Width::Word, None, 0, 32);
        assert_eq!(set.count(), 16);
        filter(&mut set, &m0, &m1, Width::Word, Predicate::Changed);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![10, 20]);
        filter(&mut set, &m1, &m2, Width::Word, Predicate::Changed);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![10]);
        filter(&mut set, &m1, &m2, Width::Word, Predicate::Increased);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![10]);
    }

    fn write_word(buf: &mut [u8], off: usize, v: u16) {
        buf[off..off + 2].copy_from_slice(&v.to_be_bytes());
    }

    #[test]
    fn filter_empty_set_stays_empty() {
        let old = mem(&[1, 2]);
        let new = mem(&[3, 4]);
        let mut s = CandidateSet::new(2);
        filter(&mut s, &old, &new, Width::Byte, Predicate::Changed);
        assert_eq!(s.count(), 0);
    }
}
