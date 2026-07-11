//! Degas Elite RLE codec + linear↔interleaved mapping, split from degas.rs
//! (200-line budget, P21); semantics unchanged — see degas.rs header for the
//! authority citation.

use crate::ripper::image::{ScreenRes, SCREEN_BYTES};

/// PackBits-style Degas Elite RLE. Some((linear, consumed)) on success.
/// Control byte `c`: 0x00..=0x7F literal (copy `c+1` bytes); 0x81..=0xFF run
/// (repeat next byte `257-c` times); 0x80 no-op (consume, emit nothing).
pub(crate) fn rle_decode(input: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::with_capacity(SCREEN_BYTES);
    let mut i = 0usize;
    while out.len() < SCREEN_BYTES {
        let c = *input.get(i)?;
        i += 1;
        if c <= 0x7F {
            let n = c as usize + 1;
            let lit = input.get(i..i + n)?;
            if out.len() + n > SCREEN_BYTES {
                return None;
            }
            out.extend_from_slice(lit);
            i += n;
        } else if c >= 0x81 {
            let n = 257 - c as usize;
            let b = *input.get(i)?;
            i += 1;
            if out.len() + n > SCREEN_BYTES {
                return None;
            }
            out.resize(out.len() + n, b);
        }
    }
    Some((out, i))
}

/// Linear (per-row, plane-by-plane) → standard word-interleaved screen.
/// linear MUST be exactly SCREEN_BYTES (guaranteed by rle_decode). Degas
/// Elite stores decompressed bytes per scanline, one whole plane-line at a
/// time; `DegasPutByte` writes decompressed byte `k` of plane `p`, row `r`
/// to interleaved offset `r*bpl + (k/2)*2*P + p*2 + (k&1)` — this is that
/// mapping applied forward.
pub(crate) fn reinterleave(linear: &[u8], res: ScreenRes) -> Vec<u8> {
    let (planes, bpl) = (res.planes(), res.bytes_per_line());
    let plane_line = bpl / planes; // bytes of one plane's row: 40 | 80 | 80
    let mut out = vec![0u8; SCREEN_BYTES];
    for r in 0..SCREEN_BYTES / bpl {
        for p in 0..planes {
            for k in 0..plane_line {
                let dst = r * bpl + (k / 2) * 2 * planes + p * 2 + (k & 1);
                out[dst] = linear[r * bpl + p * plane_line + k];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emit run-length control/value pairs (blocks of <= 128) totaling
    /// exactly `n` zero bytes of decoded output.
    fn run_fill(n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut remaining = n;
        while remaining > 0 {
            let block = remaining.min(128);
            out.push((257 - block) as u8);
            out.push(0);
            remaining -= block;
        }
        out
    }

    #[test]
    fn rle_literal() {
        // literal control (n=3, "ABC") contributes 3 output bytes — the
        // fill only needs to cover the remaining SCREEN_BYTES - 3.
        let mut input = vec![0x02, b'A', b'B', b'C'];
        input.extend(run_fill(SCREEN_BYTES - 3));
        let (out, consumed) = rle_decode(&input).unwrap();
        assert_eq!(&out[..3], b"ABC");
        assert_eq!(consumed, input.len());
    }

    #[test]
    fn rle_run() {
        let mut a = vec![0xFFu8, 0x42]; // run of 2 (257 - 255)
        a.extend(run_fill(SCREEN_BYTES - 2));
        let (out, _) = rle_decode(&a).unwrap();
        assert_eq!(&out[..2], &[0x42, 0x42]);

        let mut b = vec![0x81u8, 0x42]; // run of 128 (257 - 129)
        b.extend(run_fill(SCREEN_BYTES - 128));
        let (out, _) = rle_decode(&b).unwrap();
        assert_eq!(&out[..128], [0x42u8; 128].as_slice());
    }

    #[test]
    fn rle_noop_0x80() {
        let mut input = vec![0x80u8]; // no-op contributes 0 output bytes
        input.extend(run_fill(SCREEN_BYTES));
        let (_, consumed) = rle_decode(&input).unwrap();
        assert_eq!(consumed, input.len());
    }

    #[test]
    fn rle_truncated_is_none() {
        assert_eq!(rle_decode(&[0x02, b'A', b'B']), None);
        assert_eq!(rle_decode(&[0x81]), None);
    }

    #[test]
    fn rle_overrun_is_none() {
        // Fill SCREEN_BYTES - 3 with run blocks, then a 4-byte literal that
        // would push the total to SCREEN_BYTES + 1.
        let mut over = run_fill(SCREEN_BYTES - 3);
        over.extend([0x03u8, 1, 2, 3, 4]); // literal control: n = 4
        assert_eq!(rle_decode(&over), None);
    }

    #[test]
    fn reinterleave_low_anchors() {
        let mut linear = vec![0u8; SCREEN_BYTES];
        // (r0,p0,k0..3) -> dst 0,1,8,9
        linear[0] = 1;
        linear[1] = 2;
        linear[2] = 3;
        linear[3] = 4;
        // (r0,p1,k0..1) -> dst 2,3
        linear[40] = 5;
        linear[41] = 6;
        // (r0,p3,k38..39) -> dst 158,159
        linear[3 * 40 + 38] = 7;
        linear[3 * 40 + 39] = 8;
        // (r1,p0,k0) -> dst 160
        linear[160] = 9;
        let out = reinterleave(&linear, ScreenRes::Low);
        assert_eq!(out[0], 1);
        assert_eq!(out[1], 2);
        assert_eq!(out[8], 3);
        assert_eq!(out[9], 4);
        assert_eq!(out[2], 5);
        assert_eq!(out[3], 6);
        assert_eq!(out[158], 7);
        assert_eq!(out[159], 8);
        assert_eq!(out[160], 9);
    }

    #[test]
    fn reinterleave_high_is_identity() {
        let linear: Vec<u8> = (0..SCREEN_BYTES).map(|i| (i % 251) as u8).collect();
        let out = reinterleave(&linear, ScreenRes::High);
        assert_eq!(out, linear);
    }
}
