//! PowerPacker (`PP20`) decruncher — a semantic port of
//! `Atari_ST_Sources/C/Mike Watson (Xerud)/NEW_DPAK/DEPACKER.S/PP2.S`, P9
//! §4.B. LSB-first bits over big-endian u32 longs, consumed from the END
//! of the data region toward its start; output is written end → start.
//! `PX20` (XOR-encrypted) and `PPMT` are deliberately not recognized —
//! see `powerpacker`'s module doc and ADR-19. Fail-closed: any bound
//! violation is `None`, never a panic (Depacker contract, ADR-2/ADR-14).

use crate::ripper::registry::MAX_UNPACKED;

/// LSB-first bits over big-endian u32 longs consumed from the END of
/// `data` toward its start (§4.B.2). `data.len() % 4 == 0` is the
/// caller's contract.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    n: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: data.len(),
            buf: 0,
            n: 0,
        }
    }

    fn bit(&mut self) -> Option<bool> {
        if self.n == 0 {
            self.pos = self.pos.checked_sub(4)?;
            self.buf = u32::from_be_bytes(self.data[self.pos..self.pos + 4].try_into().ok()?);
            self.n = 32;
        }
        let b = self.buf & 1 != 0;
        self.buf >>= 1;
        self.n -= 1;
        Some(b)
    }

    /// MSB-first fold (`roxl.l` semantics): the first bit read is the
    /// value's high bit.
    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut acc = 0u32;
        for _ in 0..n {
            acc = (acc << 1) | u32::from(self.bit()?);
        }
        Some(acc)
    }
}

/// §4.B.3: flag `0` → a literal run (chunked 2-bit count, `c==3` escalates
/// again); a match always follows unless the run exhausted the output.
fn literal_run(s: &mut Bits, out: &mut [u8], write: &mut usize) -> Option<()> {
    let mut run = 0u32;
    loop {
        let c = s.bits(2)?;
        run += c;
        if c != 3 {
            break;
        }
    }
    for _ in 0..=run {
        *write = write.checked_sub(1)?;
        out[*write] = s.bits(8)? as u8;
    }
    Some(())
}

/// §4.B.3: `code` 0..2 → short match with an `eff[code]`-bit offset;
/// `code == 3` → long match (`eff[3]` or 7-bit offset, chunked length).
fn match_copy(s: &mut Bits, eff: &[u8; 4], out: &mut [u8], write: &mut usize) -> Option<()> {
    let code = s.bits(2)?;
    let (off, len) = if code == 3 {
        let nbits = if s.bit()? { eff[3] } else { 7 };
        let off = s.bits(u32::from(nbits))?;
        let mut len = 5u32;
        loop {
            let c = s.bits(3)?;
            len += c;
            if c != 7 {
                break;
            }
        }
        (off, len as usize)
    } else {
        (s.bits(u32::from(eff[code as usize]))?, code as usize + 2)
    };
    if len > *write {
        return None;
    }
    for _ in 0..len {
        *write -= 1;
        let src = *write + off as usize + 1;
        if src >= out.len() {
            return None;
        }
        out[*write] = out[src];
    }
    Some(())
}

/// §4.B.3 entry. `body` = the data longs only (blob minus 8-byte header
/// minus 4-byte trailer); `skip`/`out_len` from the validated trailer.
pub(crate) fn decrunch(body: &[u8], eff: &[u8; 4], skip: u32, out_len: usize) -> Option<Vec<u8>> {
    if out_len == 0 || out_len > MAX_UNPACKED {
        return None;
    }
    let mut s = Bits::new(body);
    s.bits(skip)?;
    let mut out = vec![0u8; out_len];
    let mut write = out_len;
    while write > 0 {
        if !s.bit()? {
            literal_run(&mut s, &mut out, &mut write)?;
            if write == 0 {
                break; // done — no trailing match
            }
        }
        match_copy(&mut s, eff, &mut out, &mut write)?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EFF: [u8; 4] = [9, 10, 12, 13];

    #[test]
    fn pinned_bodies_decode() {
        assert_eq!(
            decrunch(&[0x00, 0x1C, 0x10, 0x00], &EFF, 8, 6),
            Some(b"AAAAAA".to_vec())
        );
        assert_eq!(
            decrunch(
                &[0xEA, 0x1A, 0x9A, 0x5A, 0x30, 0x00, 0x00, 0x00],
                &EFF,
                27,
                4
            ),
            Some(b"WXYZ".to_vec())
        );
    }

    #[test]
    fn truncated_body_is_none() {
        assert_eq!(decrunch(&[], &EFF, 0, 6), None);
        // One word of body cannot supply 1000 output bytes — the reader
        // underflows past the start of `body`.
        assert_eq!(decrunch(&[0x00, 0x1C, 0x10, 0x00], &EFF, 8, 1000), None);
    }

    #[test]
    fn absurd_lengths_are_none() {
        assert_eq!(decrunch(&[0x00, 0x1C, 0x10, 0x00], &EFF, 8, 0), None);
        assert_eq!(
            decrunch(&[0x00, 0x1C, 0x10, 0x00], &EFF, 8, MAX_UNPACKED + 1),
            None
        );
    }

    #[test]
    fn corruption_never_panics() {
        let body = [0x00u8, 0x1C, 0x10, 0x00];
        for pos in 0..body.len() {
            for byte in 0..=u8::MAX {
                let mut mutated = body;
                mutated[pos] = byte;
                let _ = decrunch(&mutated, &EFF, 8, 6);
            }
        }
    }

    /// Test-only mirror of `Bits`: LSB-first within a consumption-order
    /// word list, words then reversed into file order (the last consumed
    /// word is the LAST word in the body — §4.B.2).
    struct BitWriter {
        words: Vec<u32>,
        cur: u32,
        n: u32,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                words: Vec::new(),
                cur: 0,
                n: 0,
            }
        }
        fn push_bit(&mut self, bit: bool) {
            self.cur |= u32::from(bit) << self.n;
            self.n += 1;
            if self.n == 32 {
                self.words.push(self.cur);
                self.cur = 0;
                self.n = 0;
            }
        }
        fn push_bits_msb(&mut self, value: u32, width: u32) {
            for i in (0..width).rev() {
                self.push_bit((value >> i) & 1 != 0);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            if self.n > 0 {
                self.words.push(self.cur);
            }
            self.words
                .iter()
                .rev()
                .flat_map(|w| w.to_be_bytes())
                .collect()
        }
    }

    #[test]
    fn code_rows() {
        for (code, len) in [(0u32, 2usize), (1, 3), (2, 4)] {
            let mut w = BitWriter::new();
            w.push_bit(false);
            w.push_bits_msb(0, 2);
            w.push_bits_msb(0x58, 8);
            w.push_bits_msb(code, 2);
            w.push_bits_msb(0, u32::from(EFF[code as usize]));
            let body = w.finish();
            let out_len = 1 + len;
            assert_eq!(
                decrunch(&body, &EFF, 0, out_len),
                Some(vec![0x58u8; out_len])
            );
        }

        // code 3, offset-size bit 1 (eff[3]-bit offset), length chunk 7,0 -> len 12.
        let mut w = BitWriter::new();
        w.push_bit(false);
        w.push_bits_msb(0, 2);
        w.push_bits_msb(0x58, 8);
        w.push_bits_msb(3, 2);
        w.push_bit(true);
        w.push_bits_msb(0, u32::from(EFF[3]));
        w.push_bits_msb(7, 3);
        w.push_bits_msb(0, 3);
        let body = w.finish();
        assert_eq!(decrunch(&body, &EFF, 0, 13), Some(vec![0x58u8; 13]));
    }
}
