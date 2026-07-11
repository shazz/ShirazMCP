//! ZX0 v2 bit-stream decoder — a semantic port of einar-saukas/ZX0 v2.2's
//! `dzx0.c` (P12 §4; v1-classic/backwards variants deferred, §4.0/ADR-25).
//! Interlaced Elias-gamma bit stream with backtrack, one forward cursor
//! shared by bit reloads and raw reads. Fail-closed: any bound violation or
//! corrupt stream is `None`, never a panic (Depacker contract, ADR-2/ADR-14).

use crate::ripper::registry::MAX_UNPACKED;

/// §4.1: one forward cursor shared by bit reloads and raw reads.
struct Stream<'a> {
    data: &'a [u8],
    pos: usize,
    mask: u8, // 0 = register empty (initial state)
    byte: u8,
    last: u8,
    backtrack: bool,
}

impl<'a> Stream<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            mask: 0,
            byte: 0,
            last: 0,
            backtrack: false,
        }
    }

    /// Next byte at the cursor (None if exhausted); also records `last`
    /// (the byte a backtracked bit reads from, §4.1/§4.3).
    fn take_byte(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        self.last = b;
        Some(b)
    }

    /// §4.1 verbatim: backtrack (set only after an offset-LSB byte) takes
    /// priority over the normal MSB-first bit register.
    fn bit(&mut self) -> Option<bool> {
        if self.backtrack {
            self.backtrack = false;
            return Some(self.last & 1 != 0);
        }
        self.mask >>= 1;
        if self.mask == 0 {
            self.mask = 0x80;
            self.byte = self.take_byte()?;
        }
        Some(self.byte & self.mask != 0)
    }

    /// §4.2: interlaced Elias gamma. The `MAX_UNPACKED` cap is our guard
    /// (the C exits on file errors instead) against an all-zero input
    /// doubling `value` forever.
    fn gamma(&mut self, inverted: bool) -> Option<u32> {
        let mut value: u32 = 1;
        while !self.bit()? {
            let data_bit = u32::from(self.bit()? ^ inverted);
            value = (value << 1) | data_bit;
            if value as usize > MAX_UNPACKED {
                return None;
            }
        }
        Some(value)
    }
}

/// Forward per-byte copy; recomputes src each byte so overlap replicates
/// (offset 1 = RLE). memmove/copy_within would BREAK runs — keep the loop.
fn copy_from(out: &mut Vec<u8>, offset: usize, n: usize) -> Option<()> {
    if offset == 0 || offset > out.len() || out.len().checked_add(n)? > MAX_UNPACKED {
        return None;
    }
    for _ in 0..n {
        out.push(out[out.len() - offset]);
    }
    Some(())
}

/// `n` raw bytes cursor -> out; caps at MAX_UNPACKED like `copy_from`.
fn literals(s: &mut Stream, out: &mut Vec<u8>, n: usize) -> Option<()> {
    if out.len().checked_add(n)? > MAX_UNPACKED {
        return None;
    }
    for _ in 0..n {
        out.push(s.take_byte()?);
    }
    Some(())
}

/// One copy event (§4.3). Returns `Some(true)` iff the end marker was read
/// (only reachable with `new_offset`).
fn copy_event(
    s: &mut Stream,
    out: &mut Vec<u8>,
    last_offset: &mut usize,
    new_offset: bool,
) -> Option<bool> {
    let n = if new_offset {
        let msb = s.gamma(true)?; // INVERTED data bits (v2)
        if msb == 256 {
            return Some(true); // end marker
        }
        if msb > 256 {
            return None; // corrupt (encoder max is 255)
        }
        let lsb = s.take_byte()?;
        *last_offset = msb as usize * 128 - (lsb as usize >> 1);
        s.backtrack = true; // bit 0 of lsb = first bit of the length gamma
        s.gamma(false)? as usize + 1
    } else {
        s.gamma(false)? as usize
    };
    copy_from(out, *last_offset, n)?;
    Some(false)
}

/// §4.3 entry. Decodes a ZX0 v2 stream starting at `data[0]`; trailing
/// bytes after the end marker are ignored. Returns (unpacked, consumed).
/// None = corrupt/truncated/absurd — never a panic.
pub(crate) fn decrunch(data: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut s = Stream::new(data);
    let mut out = Vec::new();
    let mut last_offset = 1usize;
    loop {
        let n = s.gamma(false)? as usize;
        literals(&mut s, &mut out, n)?;
        let mut new_offset = s.bit()?;
        loop {
            if copy_event(&mut s, &mut out, &mut last_offset, new_offset)? {
                return Some((out, s.pos));
            }
            new_offset = s.bit()?;
            if !new_offset {
                break; // -> COPY_LITERALS
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Whole blobs (P12 §6.2), reference-compressor/decompressor pinned.
    const Z1: &[u8] = &[0xD5, 0x41, 0x55, 0x60];
    const Z2: &[u8] = &[0x87, 0x41, 0x55, 0x55, 0x80];
    const Z3: &[u8] = &[0x7B, 0x41, 0x42, 0x43, 0xFA, 0x55, 0x55, 0x80];

    #[test]
    fn pinned_streams_decode() {
        for (blob, expected) in [(Z1, &b"A"[..]), (Z2, &b"AAAAAA"[..]), (Z3, &b"ABCABC"[..])] {
            let (out, consumed) = decrunch(blob).unwrap();
            assert_eq!(out, expected);
            assert_eq!(consumed, blob.len());
        }
    }

    #[test]
    fn trailing_bytes_are_prefix() {
        let mut with_trailer = Z2.to_vec();
        with_trailer.extend([0x99; 3]);
        let (out, consumed) = decrunch(&with_trailer).unwrap();
        assert_eq!(out, b"AAAAAA");
        assert_eq!(consumed, 5);
    }

    #[test]
    fn all_zero_is_none() {
        assert_eq!(decrunch(&[0u8; 64]), None);
    }

    #[test]
    fn offset_beyond_written_is_none() {
        let mut mutated = Z3.to_vec();
        mutated[4] = 0x00; // LSB byte -> offset 128, only 3 bytes written
        assert_eq!(decrunch(&mutated), None);
    }

    #[test]
    fn truncated_is_none() {
        assert_eq!(decrunch(&[]), None);
        for k in 0..Z3.len() {
            assert_eq!(decrunch(&Z3[..k]), None, "k={k}");
        }
    }

    #[test]
    fn corruption_never_panics() {
        for pos in 0..Z1.len() {
            for byte in 0..=u8::MAX {
                let mut mutated = Z1.to_vec();
                mutated[pos] = byte;
                let _ = decrunch(&mutated);
            }
        }
    }
}
