//! RNC ProPack method 1 decoder — a semantic port of
//! `Atari_ST_Sources/C/Mike Watson (Xerud)/NEW_DPAK/DEPACKER.S/RNC_1_2.S`,
//! P9 §4.C.2–4. Little-endian-word LSB-first bit stream (values fold
//! LSB-first, contrast Pack-Ice/Atomik/PowerPacker), per-chunk canonical
//! Huffman tables, byte-aligned literal runs interleaved at the current
//! lookahead position. The old 12-byte-header layout and the in-place
//! leeway machinery are out of scope (see `rnc`'s module doc). Fail-closed:
//! any bound violation is `None`, never a panic (Depacker contract,
//! ADR-2/ADR-14).

use crate::ripper::registry::MAX_UNPACKED;

/// §4.C.2 verbatim. `p` is the byte offset of the lookahead word — spliced
/// into `buf`/`count` but zero of its bits consumed, which is why a
/// literal run may claim the bytes at `p` directly and `fix()` afterwards.
struct Bits<'a> {
    data: &'a [u8],
    p: usize,
    buf: u32,
    count: u32,
}

impl<'a> Bits<'a> {
    /// Out-of-range bytes read as `0x00` (§4.C.6): a genuine file never
    /// consumes those bits; a corrupt one fails the output CRC instead.
    fn le16(data: &[u8], at: usize) -> u32 {
        let lo = data.get(at).copied().unwrap_or(0);
        let hi = data.get(at + 1).copied().unwrap_or(0);
        u32::from(lo) | (u32::from(hi) << 8)
    }

    fn new(data: &'a [u8]) -> Self {
        Self {
            buf: Self::le16(data, 0),
            data,
            p: 0,
            count: 16,
        }
    }

    fn peek(&self, n: u32) -> u32 {
        self.buf & ((1u32 << n) - 1)
    }

    fn advance(&mut self, n: u32) {
        self.buf >>= n;
        self.count -= n;
        if self.count < 16 {
            self.p += 2;
            self.buf |= Self::le16(self.data, self.p) << self.count;
            self.count += 16;
        }
    }

    fn read(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.advance(n);
        v
    }

    /// Re-splice the lookahead word at the new `p`, after a literal run
    /// claimed the previously-spliced word's bytes directly.
    fn fix(&mut self) {
        self.count -= 16;
        self.buf &= (1u32 << self.count) - 1;
        self.buf |= Self::le16(self.data, self.p) << self.count;
        self.count += 16;
    }
}

struct Huf {
    len: u32,
    rev: u32,
    value: u32,
}

/// Canonical Huffman build (§4.C.3): codes ascending by length then symbol
/// order from a 32-bit left-justified accumulator, stored bit-reversed for
/// LSB-first matching against `peek`. A `lens[sym] == 0` symbol is unused.
fn canonical_table(lens: &[u32]) -> Vec<Huf> {
    let mut entries = Vec::new();
    let mut codeb: u32 = 0;
    for len in 1..=15u32 {
        for (sym, &l) in lens.iter().enumerate() {
            if l == len {
                let rev = codeb.reverse_bits() & ((1u32 << len) - 1);
                entries.push(Huf {
                    len,
                    rev,
                    value: sym as u32,
                });
                codeb = codeb.wrapping_add(1u32 << (32 - len));
            }
        }
    }
    entries
}

/// §4.C.3: `count = read(5)` symbols, each a 4-bit code length.
fn read_table(s: &mut Bits) -> Vec<Huf> {
    let count = s.read(5);
    let lens: Vec<u32> = (0..count).map(|_| s.read(4)).collect();
    canonical_table(&lens)
}

/// §4.C.3 `huf_read`: first table entry (build order) whose length-prefix
/// matches wins; then expand `value < 2` verbatim or `(1<<n) | read(n)`.
fn huf_read(s: &mut Bits, table: &[Huf]) -> Option<usize> {
    let entry = table.iter().find(|e| s.peek(e.len) == e.rev)?;
    s.advance(entry.len);
    let v = entry.value;
    if v < 2 {
        return Some(v as usize);
    }
    let n = v - 1;
    if n > 16 {
        return None; // the asm's word-sized mask cannot read more
    }
    Some(((1u32 << n) | s.read(n)) as usize)
}

/// A literal run: `run` raw bytes at the (unconsumed) lookahead position,
/// then re-splice (§4.C.4). `run == 0` is a no-op — `p`/`fix` untouched.
fn literal_run(
    s: &mut Bits,
    stream: &[u8],
    out: &mut Vec<u8>,
    unpacked: usize,
    run: usize,
) -> Option<()> {
    if run == 0 {
        return Some(());
    }
    if s.p + run > stream.len() || out.len() + run > unpacked {
        return None;
    }
    out.extend_from_slice(&stream[s.p..s.p + run]);
    s.p += run;
    s.fix();
    Some(())
}

/// A match: `len` forward bytes copied from `dist` back (overlap
/// replicates; `dist == 1` is RLE).
fn match_copy(out: &mut Vec<u8>, dist: usize, len: usize, unpacked: usize) -> Option<()> {
    if dist == 0 || dist > out.len() || out.len() + len > unpacked {
        return None;
    }
    for _ in 0..len {
        out.push(out[out.len() - dist]);
    }
    Some(())
}

/// §4.C.4 entry. `stream` = `blob[18 .. 18+packed]`. None = corrupt,
/// truncated, or absurd — never a panic. The caller re-checks the output
/// CRC (§4.C.1); this function only enforces length bookkeeping.
pub(crate) fn unpack(stream: &[u8], unpacked: usize) -> Option<Vec<u8>> {
    if unpacked == 0 || unpacked > MAX_UNPACKED {
        return None;
    }
    let mut s = Bits::new(stream);
    s.read(2); // 2 pad bits, discarded
    let mut out = Vec::with_capacity(unpacked);
    while out.len() < unpacked {
        let raw_t = read_table(&mut s);
        let dist_t = read_table(&mut s);
        let len_t = read_table(&mut s);
        let ch_count = s.read(16);
        if ch_count == 0 {
            return None; // the asm would dbf-wrap
        }
        let before = out.len();
        for i in 0..ch_count {
            if i > 0 {
                let dist = huf_read(&mut s, &dist_t)? + 1;
                let len = huf_read(&mut s, &len_t)? + 2;
                match_copy(&mut out, dist, len, unpacked)?;
            }
            let run = huf_read(&mut s, &raw_t)?;
            literal_run(&mut s, stream, &mut out, unpacked, run)?;
        }
        if out.len() == before {
            return None; // zero-progress chunk set
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const M1: &[u8] = &[
        0x88, 0x88, 0x10, 0x03, 0x20, 0x04, 0x00, 0x12, 0x41, 0x00, 0x00,
    ];

    #[test]
    fn pinned_stream_decodes() {
        assert_eq!(unpack(M1, 6), Some(b"AAAAAA".to_vec()));
    }

    #[test]
    fn bit_reader_word_order() {
        let data = [0x01u8, 0x80, 0x03, 0x00];
        let mut s = Bits::new(&data);
        assert_eq!(s.read(1), 1);
        assert_eq!(s.read(16), 0xC000);
    }

    #[test]
    fn huffman_canonical_build() {
        let t = canonical_table(&[1, 1]);
        assert_eq!(rows(&t), vec![(1, 0, 0), (1, 1, 1)]);

        let t = canonical_table(&[1, 2, 2]);
        assert_eq!(rows(&t), vec![(1, 0, 0), (2, 1, 1), (2, 3, 2)]);

        let t = canonical_table(&[0, 0, 1]);
        assert_eq!(rows(&t), vec![(1, 0, 2)]);
    }

    fn rows(t: &[Huf]) -> Vec<(u32, u32, u32)> {
        t.iter().map(|e| (e.len, e.rev, e.value)).collect()
    }

    #[test]
    fn huf_read_expansion() {
        let table = [Huf {
            len: 1,
            rev: 0,
            value: 2,
        }];
        let data = [0x02u8, 0x00];
        assert_eq!(huf_read(&mut Bits::new(&data), &table), Some(3));

        let table = [Huf {
            len: 1,
            rev: 0,
            value: 5,
        }];
        let data = [0x00u8, 0x00];
        assert_eq!(huf_read(&mut Bits::new(&data), &table), Some(16));

        let table = [Huf {
            len: 1,
            rev: 0,
            value: 0,
        }];
        let data = [0x01u8, 0x00];
        assert_eq!(huf_read(&mut Bits::new(&data), &table), None);

        let table = [Huf {
            len: 1,
            rev: 0,
            value: 18,
        }];
        let data = [0x00u8, 0x00];
        assert_eq!(huf_read(&mut Bits::new(&data), &table), None);
    }

    /// Test-only mirror of the writer side: pushes bits LSB-first into
    /// consumption order, matching `Bits::peek`'s low-bits-first values.
    struct M1Writer {
        bits: Vec<bool>,
    }

    impl M1Writer {
        fn new() -> Self {
            Self { bits: Vec::new() }
        }
        fn push_value(&mut self, value: u32, width: u32) {
            for i in 0..width {
                self.bits.push((value >> i) & 1 != 0);
            }
        }
        fn finish(self) -> Vec<u8> {
            let mut out = vec![0u8; self.bits.len().div_ceil(8)];
            for (k, &b) in self.bits.iter().enumerate() {
                if b {
                    out[k / 8] |= 1 << (k % 8);
                }
            }
            out
        }
    }

    #[test]
    fn zero_progress_chunkset_is_none() {
        let mut w = M1Writer::new();
        w.push_value(0, 2); // pad
        w.push_value(1, 5); // raw table: count 1
        w.push_value(1, 4); // raw table: lens[0] = 1
        w.push_value(0, 5); // dist table: count 0
        w.push_value(0, 5); // len table: count 0
        w.push_value(1, 16); // ch_count = 1
        w.push_value(0, 1); // raw_t huf_read: value 0 -> run 0
        let stream = w.finish();
        assert_eq!(unpack(&stream, 4), None);
    }

    #[test]
    fn truncated_stream_is_none() {
        assert_eq!(unpack(&[], 6), None);
    }

    #[test]
    fn absurd_lengths_are_none() {
        assert_eq!(unpack(M1, 0), None);
        assert_eq!(unpack(M1, MAX_UNPACKED + 1), None);
    }

    #[test]
    fn corruption_never_panics() {
        for pos in 0..M1.len() {
            for byte in 0..=u8::MAX {
                let mut mutated = M1.to_vec();
                mutated[pos] = byte;
                let _ = unpack(&mutated, 6);
            }
        }
    }
}
