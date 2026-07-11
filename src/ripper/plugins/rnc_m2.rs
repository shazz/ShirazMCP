//! RNC ProPack method 2 decoder — a semantic port of
//! `Atari_ST_Sources/C/Mike Watson (Xerud)/NEW_DPAK/DEPACKER.S/RNC_2.S`,
//! P9 §4.C.5. Forward MSB-first recycled-sentinel bit reader (Atomik's
//! reader mirrored forward): init discards the first byte's 2 top bits
//! and manufactures a sentinel at bit 1 (`reg = (first_byte << 2) | 2`),
//! so the first byte delivers 6 data bits and every reloaded byte
//! delivers 8. The XOR key is zero in this build and dropped entirely
//! (§4.C.5, ADR-19). Fail-closed: any bound violation is `None`, never a
//! panic (Depacker contract, ADR-2/ADR-14).

use crate::ripper::registry::MAX_UNPACKED;

/// Forward reader; recycled-sentinel reload (Atomik's §4.A.2 scheme,
/// forward).
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    reg: u8,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        let mut s = Self {
            data,
            pos: 0,
            reg: 0,
        };
        let first = s.take_byte()?;
        s.reg = (first << 2) | 2;
        Some(s)
    }

    fn take_byte(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn bit(&mut self) -> Option<bool> {
        let carry = self.reg & 0x80 != 0;
        self.reg <<= 1;
        if self.reg == 0 {
            let next = self.take_byte()?;
            let bit = next & 0x80 != 0;
            self.reg = (next << 1) | u8::from(carry);
            return Some(bit);
        }
        Some(carry)
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut acc = 0u32;
        for _ in 0..n {
            acc = (acc << 1) | u32::from(self.bit()?);
        }
        Some(acc)
    }
}

enum Code {
    Len(usize),
    RawBlock,
    End,
}

/// §4.C.5 length prefix code. The `bool` is "offset is low-byte-only"
/// (`len == 2`). A stream-escape-continue (`111`, byte `0`, bit `1`) is a
/// no-op within the prefix — loop back and keep decoding.
fn length_code(s: &mut Bits) -> Option<(Code, bool)> {
    loop {
        if s.bit()? {
            if !s.bit()? {
                return Some((Code::Len(2), true)); // "1, 0"
            }
            if !s.bit()? {
                return Some((Code::Len(3), false)); // "1, 1, 0"
            }
            let b = s.take_byte()?; // "1, 1, 1" + raw byte
            if b != 0 {
                return Some((Code::Len(b as usize + 8), false));
            }
            if s.bit()? {
                continue; // stream escape: continue the main loop
            }
            return Some((Code::End, false));
        }
        let x = u32::from(s.bit()?);
        if !s.bit()? {
            return Some((Code::Len((4 + x) as usize), false)); // "0, x, 0"
        }
        let z = u32::from(s.bit()?);
        let v = (3 + x) * 2 + z; // "0, x, 1, z"
        return Some(if v == 9 {
            (Code::RawBlock, false)
        } else {
            (Code::Len(v as usize), false)
        });
    }
}

/// §4.C.5 offset prefix code (`len != 2`): the `hi` byte.
fn offset_high(s: &mut Bits) -> Option<u32> {
    if !s.bit()? {
        return Some(0); // "0"
    }
    let b = u32::from(s.bit()?);
    if !s.bit()? {
        return Some(if b == 1 { 1 } else { 2 + u32::from(s.bit()?) }); // "1,b,0"
    }
    let d = u32::from(s.bit()?);
    let base = (2 * b + d) | 4; // "1,b,1,d,..."
    if s.bit()? {
        Some(base) // "1,b,1,d,1"
    } else {
        Some(base * 2 + u32::from(s.bit()?)) // "1,b,1,d,0,f"
    }
}

/// `dist = ((hi << 8) | lo) + 1`; for `len == 2` there is no `hi` prefix.
fn offset(s: &mut Bits, low_only: bool) -> Option<usize> {
    let hi = if low_only { 0 } else { offset_high(s)? };
    let lo = s.take_byte()?;
    Some((((hi << 8) | u32::from(lo)) + 1) as usize)
}

/// `.rnc20E`: a raw block of `4*(v+3)` bytes (12..72), copied straight
/// from the byte cursor, no offset.
fn raw_block(s: &mut Bits, out: &mut Vec<u8>, unpacked: usize) -> Option<()> {
    let v = s.bits(4)?;
    let n = 4 * (v as usize + 3);
    if out.len() + n > unpacked {
        return None;
    }
    for _ in 0..n {
        out.push(s.take_byte()?);
    }
    Some(())
}

/// A match: `len` forward bytes copied from `dist` back (overlap
/// replicates; `dist == 1` is RLE — the asm's `.rnc21F` special case).
fn match_copy(
    s: &mut Bits,
    out: &mut Vec<u8>,
    unpacked: usize,
    len: usize,
    low_only: bool,
) -> Option<()> {
    let dist = offset(s, low_only)?;
    if dist > out.len() || out.len() + len > unpacked {
        return None;
    }
    for _ in 0..len {
        out.push(out[out.len() - dist]);
    }
    Some(())
}

/// §4.C.5 entry. `stream` = `blob[18 .. 18+packed]`. None = corrupt,
/// truncated, or absurd — never a panic. The caller re-checks the output
/// CRC (§4.C.1); this function only enforces length bookkeeping and the
/// end-of-stream length match.
pub(crate) fn unpack(stream: &[u8], unpacked: usize) -> Option<Vec<u8>> {
    if unpacked == 0 || unpacked > MAX_UNPACKED {
        return None;
    }
    let mut s = Bits::new(stream)?;
    let mut out = Vec::with_capacity(unpacked);
    loop {
        if !s.bit()? {
            if out.len() >= unpacked {
                return None;
            }
            out.push(s.take_byte()?);
            continue;
        }
        match length_code(&mut s)? {
            (Code::End, _) => return (out.len() == unpacked).then_some(out),
            (Code::RawBlock, _) => raw_block(&mut s, &mut out, unpacked)?,
            (Code::Len(len), low_only) => match_copy(&mut s, &mut out, unpacked, len, low_only)?,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M2: &[u8] = &[0x07, 0x41, 0x42, 0x43, 0x3C, 0x02, 0x00];

    #[test]
    fn pinned_stream_decodes() {
        assert_eq!(unpack(M2, 6), Some(b"ABCABC".to_vec()));
    }

    #[test]
    fn eof_requires_exact_len() {
        assert_eq!(unpack(M2, 5), None);
        assert_eq!(unpack(M2, 7), None);
    }

    #[test]
    fn truncated_stream_is_none() {
        assert_eq!(unpack(&[], 6), None);
    }

    #[test]
    fn absurd_lengths_are_none() {
        assert_eq!(unpack(M2, 0), None);
        assert_eq!(unpack(M2, MAX_UNPACKED + 1), None);
    }

    #[test]
    fn corruption_never_panics() {
        for pos in 0..M2.len() {
            for byte in 0..=u8::MAX {
                let mut mutated = M2.to_vec();
                mutated[pos] = byte;
                let _ = unpack(&mutated, 6);
            }
        }
    }

    /// Test-only forward bit packer (§6.3.1 serialization, mirrored): the
    /// first byte carries up to 6 data bits (top-justified in its low 6
    /// bits — its top 2 bits are the discarded pad); each following byte
    /// carries 8, zero-padded low if incomplete.
    fn pack_first(bits: &[u8]) -> u8 {
        let mut v = 0u8;
        for &b in bits {
            v = (v << 1) | b;
        }
        v << (6 - bits.len() as u32)
    }

    fn pack_plain(bits: &[u8]) -> u8 {
        let mut v = 0u8;
        for &b in bits {
            v = (v << 1) | b;
        }
        v << (8 - bits.len() as u32)
    }

    fn stream_from_bits(bits: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let first_k = bits.len().min(6);
        out.push(pack_first(&bits[..first_k]));
        let mut i = first_k;
        while i < bits.len() {
            let end = (i + 8).min(bits.len());
            out.push(pack_plain(&bits[i..end]));
            i = end;
        }
        out
    }

    fn code_of(bits: &[u8]) -> (Code, bool) {
        let data = stream_from_bits(bits);
        length_code(&mut Bits::new(&data).unwrap()).unwrap()
    }

    #[test]
    fn length_code_rows() {
        assert!(matches!(code_of(&[0, 0, 0]), (Code::Len(4), false)));
        assert!(matches!(code_of(&[0, 1, 0]), (Code::Len(5), false)));
        assert!(matches!(code_of(&[0, 0, 1, 0]), (Code::Len(6), false)));
        assert!(matches!(code_of(&[0, 1, 1, 0]), (Code::Len(8), false)));
        assert!(matches!(code_of(&[0, 1, 1, 1]), (Code::RawBlock, false)));
        assert!(matches!(code_of(&[1, 0]), (Code::Len(2), true)));
        assert!(matches!(code_of(&[1, 1, 0]), (Code::Len(3), false)));

        let mut data = stream_from_bits(&[1, 1, 1]);
        data.push(5);
        assert!(matches!(
            length_code(&mut Bits::new(&data).unwrap()),
            Some((Code::Len(13), false))
        ));

        // "111" + byte 0 -> escape; the continuation bit is read from the
        // still-loaded init byte's leftover (zero-padded) bits -> End.
        let mut data = stream_from_bits(&[1, 1, 1]);
        data.push(0);
        assert!(matches!(
            length_code(&mut Bits::new(&data).unwrap()),
            Some((Code::End, false))
        ));
    }

    #[test]
    fn offset_rows() {
        assert_eq!(
            offset_high(&mut Bits::new(&stream_from_bits(&[0])).unwrap()),
            Some(0)
        );
        assert_eq!(
            offset_high(&mut Bits::new(&stream_from_bits(&[1, 1, 0])).unwrap()),
            Some(1)
        );
        assert_eq!(
            offset_high(&mut Bits::new(&stream_from_bits(&[1, 0, 0, 1])).unwrap()),
            Some(3)
        );
        assert_eq!(
            offset_high(&mut Bits::new(&stream_from_bits(&[1, 0, 1, 1, 1])).unwrap()),
            Some(5)
        );
        assert_eq!(
            offset_high(&mut Bits::new(&stream_from_bits(&[1, 1, 1, 0, 0, 1])).unwrap()),
            Some(13)
        );
    }
}
