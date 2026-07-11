//! Atomik 3.5 (`ATM5`) decruncher — a semantic port of
//! `Atari_ST_Sources/ASM/Electronic Images/routines.s/depack.s/depack35.s`
//! (`MODE=0`), P9 §4.A: backward bit stream with a recycled-sentinel
//! reload (the one way it differs from Pack-Ice's forced-1), per-chunk
//! special-offset/frequency tables, unary-threshold length coding,
//! descending overlap-replicating copies. Dropped (§4.A.7, ADR-19): the
//! picture algorithm (trailing count byte != 0, refused fail-closed), the
//! in-place `MODE=1` variant, and the `ATOM` magic (ambiguous with
//! Thunder datafiles). Fail-closed: any bound violation is `None`, never
//! a panic (Depacker contract, ADR-2/ADR-14). The bit reader and static
//! tables live in `atomik_bits` (200-line budget).

use super::atomik_bits::{build_odd_table, Stream, BASES, XBITS};
use crate::ripper::registry::MAX_UNPACKED;

/// Per-chunk parameters (§4.A.3 steps 1–4).
struct Chunk {
    spec: Option<(u8, [u8; 7])>,
    freq: Option<[u8; 16]>,
    d3: u32,
    size: usize,
}

/// §4.A.3 steps 1–4: trailer read before the chunk body. Does NOT gate
/// `size` against the caller's remaining `write` — that's the caller's job.
fn chunk_header(s: &mut Stream) -> Option<Chunk> {
    let spec = if s.bit()? {
        let spec_byte = s.take_byte()?;
        Some((spec_byte, build_odd_table(spec_byte)))
    } else {
        None
    };
    let freq = if s.bit()? {
        let mut f = [0u8; 16];
        for slot in f.iter_mut() {
            *slot = s.take_byte()?;
        }
        Some(f)
    } else {
        None
    };
    let d3 = u32::from(s.take_byte()?);
    let hi = s.take_byte()?;
    let lo = s.take_byte()?;
    Some(Chunk {
        spec,
        freq,
        d3,
        size: (usize::from(hi) << 8) | usize::from(lo),
    })
}

/// §4.A.4 length code: unary zero-count up to `d3`, else escalate through
/// 4-, 8- and 14-bit rows.
fn take_length(s: &mut Stream, d3: u32) -> Option<u32> {
    let mut z = 0u32;
    while z <= d3 {
        if s.bit()? {
            return Some(z);
        }
        z += 1;
    }
    let v = s.bits(4)?;
    if v != 0 {
        return Some(v + d3);
    }
    let v = s.bits(8)?;
    if v != 0 {
        return Some(v + 15 + d3);
    }
    Some(s.bits(14)? + 270 + d3)
}

/// §4.A.4 special-op byte: frequency-table lookup or signed 4-bit delta
/// from the byte just written.
fn special_byte(s: &mut Stream, chunk: &Chunk, out: &[u8], write: usize) -> Option<u8> {
    let v = s.bits(4)?;
    if let Some(freq) = &chunk.freq {
        return Some(freq[v as usize]);
    }
    if write == out.len() {
        return None; // delta mode needs a byte already written
    }
    let prev = out[write];
    let delta = if v >= 8 { v as i32 - 16 } else { v as i32 } as u8;
    Some(prev.wrapping_sub(delta))
}

/// §4.A.5 match offset: normal (`bits(XBITS[sel]+5)`) or special-offset
/// mode (hi/v3/lo assembled per the odd table or a raw/even fallback).
fn take_offset(s: &mut Stream, sel: usize, spec: Option<&(u8, [u8; 7])>) -> Option<usize> {
    let mut val = if let Some((spec_byte, odd)) = spec {
        let hi = s.bits(XBITS[sel] + 1)?;
        let v3 = s.bits(3)?;
        let lo = if v3 < 7 {
            u32::from(odd[v3 as usize])
        } else if s.bit()? {
            s.bits(3)? * 2
        } else {
            u32::from(*spec_byte)
        };
        ((hi << 4) & 0xFFFF) | lo
    } else {
        s.bits(XBITS[sel] + 5)?
    };
    if sel > 0 {
        val += BASES[sel - 1];
    }
    Some(val as usize)
}

/// §4.A.5 copy: descending so overlaps replicate (val 0 → distance 1 → RLE).
/// `copy_within`/memmove semantics would break RLE runs — keep the loop.
fn copy_match(out: &mut [u8], write: &mut usize, len: usize, val: usize) -> Option<()> {
    if len > *write {
        return None;
    }
    for _ in 0..len {
        *write -= 1;
        let src = write.checked_add(val)?.checked_add(1)?;
        if src >= out.len() {
            return None;
        }
        out[*write] = out[src];
    }
    Some(())
}

/// §4.A.4: length code, then either a special single byte or a match.
fn code_event(s: &mut Stream, chunk: &Chunk, out: &mut [u8], write: &mut usize) -> Option<()> {
    let len_code = take_length(s, chunk.d3)?;
    let (copy_len, sel) = if len_code == 0 {
        if s.bit()? {
            let byte = special_byte(s, chunk, out, *write)?;
            *write -= 1;
            out[*write] = byte;
            return Some(());
        }
        (2usize, s.bits(2)? as usize)
    } else {
        (len_code as usize + 2, s.bits(3)? as usize)
    };
    let val = take_offset(s, sel, chunk.spec.as_ref())?;
    copy_match(out, write, copy_len, val)
}

/// §4.A entry. `stream` = blob[12 .. 12+stream_len]. None = corrupt,
/// truncated, absurd, or picture-mode (pic byte != 0) — never a panic.
pub(crate) fn decrunch(stream: &[u8], unpacked_len: usize) -> Option<Vec<u8>> {
    if unpacked_len == 0 || unpacked_len > MAX_UNPACKED {
        return None;
    }
    let mut s = Stream::new(stream);
    if s.take_byte()? != 0 {
        return None; // picture mode: out of scope (§4.A.7)
    }
    s.reg = s.take_byte()?; // raw init, no sentinel
    let mut out = vec![0u8; unpacked_len];
    let mut write = unpacked_len;
    loop {
        let chunk = chunk_header(&mut s)?;
        if chunk.size == 0 || chunk.size > write {
            return None;
        }
        let stop = write - chunk.size;
        while write > stop {
            if s.bit()? {
                code_event(&mut s, &chunk, &mut out, &mut write)?;
            } else {
                write -= 1;
                out[write] = s.take_byte()?;
            }
        }
        if write != stop {
            return None; // match overshot the chunk boundary
        }
        if s.pos == 0 {
            break;
        }
    }
    if write != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Stream-only vectors (P9 §6.1.2 table, minus the 12-byte header).
    const A1: (&[u8], usize, &[u8]) = (&[0x41, 0x42, 0x02, 0x00, 0x00, 0x08, 0x00], 2, b"AB");
    const A2: (&[u8], usize, &[u8]) = (
        &[0x00, 0x80, 0x41, 0x06, 0x00, 0x03, 0x11, 0x00],
        6,
        b"AAAAAA",
    );
    const A3: (&[u8], usize, &[u8]) = (&[0x20, 0x42, 0x02, 0x00, 0x00, 0x1D, 0x00], 2, b"AB");

    #[test]
    fn pinned_streams_decode() {
        for (stream, len, expected) in [A1, A2, A3] {
            assert_eq!(decrunch(stream, len), Some(expected.to_vec()));
        }
    }

    #[test]
    fn pic_mode_is_none() {
        let mut mutated = A1.0.to_vec();
        *mutated.last_mut().unwrap() = 0x01;
        assert_eq!(decrunch(&mutated, 2), None);
    }

    #[test]
    fn truncated_stream_is_none() {
        assert_eq!(decrunch(&[], 6), None);
        assert_eq!(decrunch(&[0x00], 6), None);
        assert_eq!(decrunch(&A2.0[1..], 6), None);
    }

    #[test]
    fn absurd_lengths_are_none() {
        assert_eq!(decrunch(A1.0, 0), None);
        assert_eq!(decrunch(A1.0, MAX_UNPACKED + 1), None);
    }

    #[test]
    fn overshoot_chunk_is_none() {
        // A2 with the chunk-size field shrunk from 6 to 5: the match's
        // copy_len (5, unchanged) overshoots the (now smaller) chunk.
        let mut mutated = A2.0.to_vec();
        mutated[3] = 0x05;
        assert_eq!(decrunch(&mutated, 6), None);
    }

    #[test]
    fn corruption_never_panics() {
        for pos in 0..A2.0.len() {
            for byte in 0..=u8::MAX {
                let mut mutated = A2.0.to_vec();
                mutated[pos] = byte;
                let _ = decrunch(&mutated, 6);
            }
        }
    }

    /// Test-only mirror of §6.1.1's serialization rule (zero-fill, not
    /// ICE's one-fill): the init byte carries up to 7 data bits at the top
    /// then a `1` sentinel then zero-fill; each reloaded byte carries 8.
    fn pack_first(bits: &[u8]) -> u8 {
        let mut v: u16 = 0;
        for &b in bits {
            v = (v << 1) | u16::from(b);
        }
        let pad = 8 - bits.len() as u32;
        ((v << pad) | (1u16 << (pad - 1))) as u8
    }

    fn pack_plain(bits: &[u8]) -> u8 {
        let mut v: u16 = 0;
        for &b in bits {
            v = (v << 1) | u16::from(b);
        }
        (v << (8 - bits.len() as u32)) as u8
    }

    fn stream_from_bits(bits: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let first_k = bits.len().min(7);
        out.push(pack_first(&bits[..first_k]));
        let mut i = first_k;
        while i < bits.len() {
            let end = (i + 8).min(bits.len());
            out.push(pack_plain(&bits[i..end]));
            i = end;
        }
        out.reverse();
        out
    }

    /// Build a stream from a bit list and load its raw init byte, mirroring
    /// the `decrunch` entry (minus the pic byte, irrelevant to these rows).
    fn init(data: &[u8]) -> Stream<'_> {
        let mut s = Stream::new(data);
        s.reg = s.take_byte().unwrap();
        s
    }

    #[test]
    fn take_length_rows() {
        let data = stream_from_bits(&[0, 0, 0, 1]);
        assert_eq!(take_length(&mut init(&data), 3), Some(3));

        let data = stream_from_bits(&[0, 0, 0, 0, 1]);
        assert_eq!(take_length(&mut init(&data), 0), Some(1));

        let data = stream_from_bits(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(take_length(&mut init(&data), 0), Some(16));
    }

    #[test]
    fn take_offset_normal_rows() {
        let data = stream_from_bits(&[0, 0, 0, 0, 0]);
        assert_eq!(take_offset(&mut init(&data), 0, None), Some(0));

        let data = stream_from_bits(&[0, 0, 0, 0, 0, 0]);
        assert_eq!(take_offset(&mut init(&data), 1, None), Some(32));

        let data = stream_from_bits(&[0; 13]);
        assert_eq!(take_offset(&mut init(&data), 7, None), Some(8032));
    }
}
