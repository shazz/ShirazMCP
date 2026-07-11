//! Memory-change-scan candidate set (P15): one bit per snapshot byte
//! offset (bit set = a candidate value STARTS there). Fixed 1 bit/byte —
//! 512 KiB for a 4 MiB snapshot regardless of how many candidates remain —
//! with ascending-offset iteration for free. Pure; no emulator, no I/O.

/// Bit `i` ⇔ snapshot offset `i` is a live candidate.
#[derive(Debug, Clone)]
pub struct CandidateSet {
    bits: Vec<u64>,
    capacity: usize,
    count: u64,
}

impl CandidateSet {
    /// Empty set over a snapshot of `capacity` bytes.
    pub fn new(capacity: usize) -> Self {
        Self {
            bits: vec![0u64; capacity.div_ceil(64)],
            capacity,
            count: 0,
        }
    }

    /// Number of live candidates. O(1).
    pub const fn count(&self) -> u64 {
        self.count
    }

    /// The snapshot length this set indexes into.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Set bit `off`. No-op if already set.
    pub fn insert(&mut self, off: usize) {
        debug_assert!(
            off < self.capacity,
            "offset {off} out of range (capacity {})",
            self.capacity
        );
        let mask = 1u64 << (off % 64);
        let word = &mut self.bits[off / 64];
        if *word & mask == 0 {
            *word |= mask;
            self.count += 1;
        }
    }

    pub fn contains(&self, off: usize) -> bool {
        off < self.capacity && self.bits[off / 64] & (1u64 << (off % 64)) != 0
    }

    /// Clear every candidate for which `keep(offset)` is false. One linear
    /// word walk; per set word, iterate set bits via `trailing_zeros`.
    pub fn retain(&mut self, mut keep: impl FnMut(usize) -> bool) {
        for (wi, word) in self.bits.iter_mut().enumerate() {
            let base = wi * 64;
            let mut remaining = *word;
            while remaining != 0 {
                let bit = remaining.trailing_zeros();
                remaining &= remaining - 1;
                if !keep(base + bit as usize) {
                    *word &= !(1u64 << bit);
                    self.count -= 1;
                }
            }
        }
    }

    /// Ascending offsets of live candidates.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.bits.iter().enumerate().flat_map(|(wi, &word)| {
            let base = wi * 64;
            let mut remaining = word;
            std::iter::from_fn(move || {
                if remaining == 0 {
                    return None;
                }
                let bit = remaining.trailing_zeros() as usize;
                remaining &= remaining - 1;
                Some(base + bit)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_set_counts_zero() {
        let s = CandidateSet::new(16);
        assert_eq!(s.count(), 0);
        assert_eq!(s.iter().count(), 0);
        assert!(!s.contains(0));
    }

    #[test]
    fn insert_contains_count() {
        let mut s = CandidateSet::new(16);
        s.insert(0);
        s.insert(5);
        s.insert(15);
        assert_eq!(s.count(), 3);
        assert!(s.contains(0) && s.contains(5) && s.contains(15));
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![0, 5, 15]);
    }

    #[test]
    fn insert_idempotent() {
        let mut s = CandidateSet::new(16);
        s.insert(5);
        s.insert(5);
        assert_eq!(s.count(), 1);
    }

    #[test]
    fn retain_drops_and_recounts() {
        let mut s = CandidateSet::new(16);
        for o in [0, 5, 15] {
            s.insert(o);
        }
        s.retain(|o| o % 2 == 0);
        assert_eq!(s.count(), 1);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn spans_word_boundaries() {
        let mut s = CandidateSet::new(200);
        for o in [63, 64, 127, 128, 199] {
            s.insert(o);
        }
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![63, 64, 127, 128, 199]);
        assert_eq!(s.count(), 5);
        s.retain(|o| o >= 64);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![64, 127, 128, 199]);
    }
}
