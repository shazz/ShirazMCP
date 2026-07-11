//! Pack-Ice LZ decruncher — a semantic port of
//! Atari_ST_Sources/C/Toyoyo/stvn/deps/pack-ice/ice_decrunch.c (P5 §4).
//! Backward bit stream, backward output, descending overlap-replicating
//! match copy. We drop the reference's `ice_decrunch_2` in-place variant
//! and its disabled (`#if 0`) picture-mode tail. Fail-closed: any bound
//! violation is `None`, never a panic (Depacker contract, ADR-2/ADR-14).

use crate::ripper::registry::MAX_UNPACKED;

/// Backward reader over the packed stream: one descending byte cursor
/// shared by the bit register and the literal copier (§4.2).
struct Stream<'a> {
    data: &'a [u8],
    pos: usize, // next unread byte is data[pos - 1]
    bits: u8,
}

impl<'a> Stream<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        let mut s = Self {
            data,
            pos: data.len(),
            bits: 0,
        };
        s.bits = s.take_byte()?; // raw init load — NO sentinel (§4.2)
        Some(s)
    }

    fn take_byte(&mut self) -> Option<u8> {
        self.pos = self.pos.checked_sub(1)?;
        Some(self.data[self.pos])
    }

    /// §4.2 verbatim, including the recomputed-bit sentinel reload.
    fn bit(&mut self) -> Option<bool> {
        let mut bit = self.bits & 0x80 != 0;
        self.bits <<= 1;
        if self.bits == 0 {
            let next = self.take_byte()?;
            bit = next & 0x80 != 0;
            self.bits = (next << 1) | 1;
        }
        Some(bit)
    }

    /// MSB-first fold; `take_bits(0) == Some(0)` reading nothing.
    fn take_bits(&mut self, n: u32) -> Option<u32> {
        let mut acc = 0u32;
        for _ in 0..n {
            acc = (acc << 1) | u32::from(self.bit()?);
        }
        Some(acc)
    }

    /// A literal block: `len` raw bytes below the cursor, FORWARD order.
    fn take_block(&mut self, len: usize) -> Option<&'a [u8]> {
        self.pos = self.pos.checked_sub(len)?;
        Some(&self.data[self.pos..self.pos + len])
    }
}

const DL_BITS: [u32; 6] = [1, 2, 2, 3, 8, 15];
const DL_ONES: [u32; 6] = [1, 3, 3, 7, 0xFF, 0x7FFF];
const DL_ADD: [usize; 7] = [1, 2, 5, 8, 15, 270, 270];

const PL_BITS: [u32; 5] = [0, 0, 1, 2, 10];
const PL_ADD: [usize; 5] = [2, 3, 4, 6, 10];

const PO_BITS: [u32; 3] = [8, 5, 12];
const PO_ADD: [i32; 3] = [31, -1, 287];

fn direct_length(s: &mut Stream) -> Option<usize> {
    let mut i = 0;
    let mut n = 0;
    while i < 6 {
        n = s.take_bits(DL_BITS[i])?;
        if n != DL_ONES[i] {
            break;
        }
        i += 1;
    }
    Some(n as usize + DL_ADD[i]) // i == 6 falls through with n = 0x7FFF (§4.4)
}

fn depack_length(s: &mut Stream) -> Option<usize> {
    let mut i = 0;
    while i < 4 && s.bit()? {
        i += 1;
    }
    Some(s.take_bits(PL_BITS[i])? as usize + PL_ADD[i])
}

fn depack_offset(s: &mut Stream, len: usize) -> Option<i32> {
    if len == 2 {
        return Some(if s.bit()? {
            s.take_bits(9)? as i32 + 0x3F
        } else {
            s.take_bits(6)? as i32 - 1
        });
    }
    let mut i = 0;
    while i < 2 && s.bit()? {
        i += 1;
    }
    let mut off = s.take_bits(PO_BITS[i])? as i32 + PO_ADD[i];
    if off < 0 {
        off -= len as i32 - 2;
    }
    Some(off)
}

/// C `memcpybwd`: descending k so an overlapping copy replicates the byte
/// just written (offset −1 raw ⇒ src = write+1 ⇒ single-byte RLE run).
/// `copy_within`/memmove semantics would BREAK runs — keep the loop.
fn copy_match(out: &mut [u8], write: usize, len: usize, off: i32) -> Option<()> {
    let src = (write + len).checked_add_signed(off as isize)?;
    if src.checked_add(len)? > out.len() {
        return None;
    }
    for k in (0..len).rev() {
        out[write + k] = out[src + k];
    }
    Some(())
}

/// Decode a Pack-Ice bit/byte stream (the blob WITHOUT its 12-byte header,
/// i.e. `blob[12..crunched]`) into exactly `unpacked_len` bytes.
/// None = corrupt/truncated/absurd — never an error, never a panic.
pub(crate) fn decrunch(stream: &[u8], unpacked_len: usize) -> Option<Vec<u8>> {
    if unpacked_len == 0 || unpacked_len > MAX_UNPACKED {
        return None;
    }
    let mut s = Stream::new(stream)?;
    let mut out = vec![0u8; unpacked_len];
    let mut write = unpacked_len;
    loop {
        if s.bit()? {
            let len = direct_length(&mut s)?;
            let block = s.take_block(len)?;
            write = write.checked_sub(len)?;
            out[write..write + len].copy_from_slice(block);
        }
        if write == 0 {
            return Some(out); // §4.3: the only exit
        }
        let len = depack_length(&mut s)?;
        let off = depack_offset(&mut s, len)?;
        write = write.checked_sub(len)?;
        copy_match(&mut out, write, len, off)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pinned stream-only vectors (P5 §6.2 table, "Stream bytes" column).
    const V1: (&[u8], usize, &[u8]) = (&[0x01, 0x41, 0xB7], 6, b"AAAAAA");
    const V2: (&[u8], usize, &[u8]) = (&[0x05, 0x41, 0x42, 0x43, 0xDB], 6, b"ABCABC");
    const V3: (&[u8], usize, &[u8]) = (&[0x0B, 0x41, 0x42, 0xC1], 4, b"ABAB");
    const V4: (&[u8], usize, &[u8]) = (&[0x51, 0xBF], 1, b"Q");
    const V5: (&[u8], usize, &[u8]) = (&[0x57, 0x58, 0x59, 0x5A, 0xEF], 4, b"WXYZ");

    #[test]
    fn pinned_vectors_decode() {
        for (stream, len, expected) in [V1, V2, V3, V4, V5] {
            assert_eq!(decrunch(stream, len), Some(expected.to_vec()));
        }
    }

    #[test]
    fn truncated_stream_is_none() {
        assert_eq!(decrunch(&[], 6), None);
        assert_eq!(decrunch(&[0xB7], 6), None);
        assert_eq!(decrunch(&V2.0[1..], 6), None);
    }

    #[test]
    fn output_underflow_is_none() {
        assert_eq!(decrunch(V1.0, 4), None);
    }

    #[test]
    fn absurd_lengths_are_none() {
        assert_eq!(decrunch(V1.0, 0), None);
        assert_eq!(decrunch(V1.0, MAX_UNPACKED + 1), None);
    }

    #[test]
    fn corruption_never_panics() {
        for pos in 0..V1.0.len() {
            for byte in 0..=u8::MAX {
                let mut mutated = V1.0.to_vec();
                mutated[pos] = byte;
                let _ = decrunch(&mutated, 6);
            }
        }
    }

    /// Test-only mirror of §6.3 rule 2 (bit-byte packing): pack `bits`
    /// (consumption order) into a stream `Stream::new` reads back in that
    /// same order. The sentinel bit and the 1-fill are numerically the same
    /// value (1), so "top k data bits, then sentinel, then 1-fill" collapses
    /// to "top k data bits, low bits all 1" — that's what `pack_one` does.
    fn pack_one(bits: &[u8]) -> u8 {
        let mut v: u16 = 0;
        for &b in bits {
            v = (v << 1) | u16::from(b);
        }
        let pad = 8 - bits.len() as u32;
        ((v << pad) | ((1u16 << pad) - 1)) as u8
    }

    fn bits_stream(bits: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let first_k = bits.len().min(7);
        out.push(pack_one(&bits[..first_k]));
        let mut i = first_k;
        while i < bits.len() {
            let end = (i + 8).min(bits.len());
            out.push(pack_one(&bits[i..end]));
            i = end;
        }
        out.reverse();
        out
    }

    /// §4.4 rows, pinned bit-for-bit. The i=3 row is corrected from P5.md
    /// §7.1 item 6's literal transcription (`[1,1,1,1,1,1,1,1,0,1]`, 10
    /// bits): reaching i=3 requires rows 0..2 to escalate at their pinned
    /// bit widths (1+2+2 = 5 bits, all-ones per §4.4's table) and row 3 to
    /// read a NON-escalating 3-bit value of 2 (binary `010`, matching the
    /// row's own stated "i=3, value 2, bits 010") — that is 8 bits total,
    /// not 10; `[1,1,1,1,1,1,1,1,0,1]` actually reads past row 3 into row 4
    /// (which needs 8 more bits the list doesn't have) and decodes to
    /// `None`. Verified against the other three rows in this table, whose
    /// list lengths already equal their exact bit consumption.
    fn check_direct_length_rows() {
        for (bits, expected) in [
            (&[0][..], 1usize),
            (&[1, 0, 0][..], 2),
            (&[1, 1, 1, 0, 1][..], 6),
            (&[1, 1, 1, 1, 1, 0, 1, 0][..], 10),
        ] {
            let data = bits_stream(bits);
            let mut s = Stream::new(&data).unwrap();
            assert_eq!(direct_length(&mut s), Some(expected));
        }
    }

    /// §4.5 rows, pinned bit-for-bit.
    fn check_depack_length_rows() {
        let mut ten = vec![1, 1, 1, 1];
        ten.extend(std::iter::repeat_n(0, 10));
        let mut fifteen = vec![1, 1, 1, 1];
        fifteen.extend([0, 0, 0, 0, 0, 0, 0, 1, 0, 1]);
        for (bits, expected) in [
            (&[0][..], 2usize),
            (&[1, 0][..], 3),
            (&[1, 1, 0, 1][..], 5),
            (&[1, 1, 1, 0, 1, 1][..], 9),
            (ten.as_slice(), 10),
            (fifteen.as_slice(), 15),
        ] {
            let data = bits_stream(bits);
            let mut s = Stream::new(&data).unwrap();
            assert_eq!(depack_length(&mut s), Some(expected));
        }
    }

    /// §4.6 rows (both the len==2 and len>2 branches), pinned bit-for-bit.
    fn check_depack_offset_rows() {
        let mut b63 = vec![1];
        b63.extend([0; 9]);
        let mut b4 = vec![0];
        b4.extend([0, 0, 0, 1, 0, 1]);
        let mut b31 = vec![0];
        b31.extend([0; 8]);
        let mut bneg4 = vec![1, 0];
        bneg4.extend([0; 5]);
        let mut b0 = vec![1, 0];
        b0.extend([0, 0, 0, 0, 1]);
        let mut b287 = vec![1, 1];
        b287.extend([0; 12]);
        for (bits, len, expected) in [
            (b63.as_slice(), 2usize, 63i32),
            (b4.as_slice(), 2, 4),
            (b31.as_slice(), 5, 31),
            (bneg4.as_slice(), 5, -4),
            (b0.as_slice(), 5, 0),
            (b287.as_slice(), 5, 287),
        ] {
            let data = bits_stream(bits);
            let mut s = Stream::new(&data).unwrap();
            assert_eq!(depack_offset(&mut s, len), Some(expected));
        }
    }

    #[test]
    fn helper_tables_row_by_row() {
        check_direct_length_rows();
        check_depack_length_rows();
        check_depack_offset_rows();
    }
}
