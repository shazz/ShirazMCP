//! SNAP1 stream encode/decode: turn a `Run` sequence into bytes
//! (`build_from_runs`), simulate the 68k decoder's in-place safety
//! (`verify_inplace`), and mirror the decode itself (`unpack`, the
//! round-trip test oracle). Split from `pack/mod.rs` for the 200-line
//! budget — `mod.rs` owns run decomposition and the `pack` search; this
//! file owns the wire format. See `mod.rs`'s module doc for the safety
//! argument and ADR-38's `0x03` redefinition this file implements.

use super::Run;

pub(super) const OP_ZERO: u8 = 0x00;
pub(super) const OP_LITERAL: u8 = 0x01;
const OP_SKIP: u8 = 0x02;
pub(super) const OP_TAIL: u8 = 0x03;

pub(super) fn push_run(out: &mut Vec<u8>, op: u8, len: usize, data: Option<&[u8]>) {
    out.push(op);
    out.extend_from_slice(&(len as u32).to_be_bytes()[1..]); // u24 BE
    if let Some(bytes) = data {
        out.extend_from_slice(bytes);
    }
}

/// Index of a `Zero` run immediately preceding the final tail-merged
/// `Literal` — ADR-38: that gap is free (absorbed into the terminal
/// `0x03`'s implicit fill), so it must not also be separately encoded.
/// A terminal `Zero` run (no trailing `Literal` at all) is handled
/// directly in `build_from_runs`, not here.
fn absorbed_gap_index(runs: &[Run]) -> Option<usize> {
    let last_is_literal = matches!(runs.last(), Some(Run::Literal { .. }));
    let idx = runs.len().checked_sub(2)?;
    (last_is_literal && matches!(runs[idx], Run::Zero { .. })).then_some(idx)
}

/// Emit `runs` as a stream. The LAST run always reaches `image.len()` (by
/// construction of `decompose_all`); if it is literal-encoded, it is
/// folded into the `0x03` tail marker itself rather than a separate
/// `0x01` run followed by an empty terminator — `0x03`'s "copy to end"
/// already needs no length field, so paying for both wastes exactly 4
/// bytes (a 1-byte opcode + a 3-byte length field) on every packed
/// stream, for nothing.
pub(super) fn build_from_runs(image: &[u8], runs: &[Run]) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.len() / 2);
    let absorbed = absorbed_gap_index(runs);
    for (i, r) in runs.iter().enumerate() {
        if Some(i) == absorbed {
            continue; // ADR-38: free zero-fill, no explicit run at all
        }
        if i == runs.len() - 1 {
            if let Some(out) = try_finish_with_tail(&mut out, image, r) {
                return out;
            }
            // else: a Skip run (never the last in practice) falls through.
        }
        match *r {
            Run::Skip { len, .. } => push_run(&mut out, OP_SKIP, len, None),
            Run::Zero { len, .. } => push_run(&mut out, OP_ZERO, len, None),
            Run::Literal { start, len } => {
                push_run(&mut out, OP_LITERAL, len, Some(&image[start..start + len]));
            }
        }
    }
    out.push(OP_TAIL); // only reached if `runs` is empty (region == hole)
    out
}

/// If `r` is the terminal run and Zero/Literal, finish `out` with the
/// `0x03` tail marker (ADR-38: a terminal `Zero` gap is an implicit,
/// header-free zero-fill) and return it; `None` for `Skip` (fall through
/// to the normal encoding — never the last run in practice).
fn try_finish_with_tail(out: &mut Vec<u8>, image: &[u8], r: &Run) -> Option<Vec<u8>> {
    match *r {
        Run::Zero { .. } => {
            out.push(OP_TAIL);
            Some(std::mem::take(out))
        }
        Run::Literal { start, len } => {
            out.push(OP_TAIL);
            out.extend_from_slice(&image[start..start + len]);
            Some(std::mem::take(out))
        }
        Run::Skip { .. } => None,
    }
}

fn read_u24(stream: &[u8], at: usize) -> Option<usize> {
    let b = stream.get(at..at + 3)?;
    Some((usize::from(b[0]) << 16) | (usize::from(b[1]) << 8) | usize::from(b[2]))
}

/// Simulate the 68k decoder: input at `[region_end - stream.len(),
/// region_end)`, output sweeping `0..region_end`. `Err((o, margin))` — the
/// output offset of the first violated rule of §4.4 (a write landing on
/// unread input, or the stream ending short of `region_end`), plus the
/// slack (`i - o`, saturating) that *was* available there — `pack` uses
/// `margin` to split an oversized zero run at exactly the safe boundary.
pub fn verify_inplace(stream: &[u8], region_end: usize) -> Result<(), (usize, usize)> {
    let i0 = region_end
        .checked_sub(stream.len())
        .ok_or((0usize, 0usize))?;
    let mut o = 0usize;
    let mut p = 0usize;
    loop {
        let op = *stream.get(p).ok_or((o, 0usize))?;
        if op == OP_TAIL {
            // ADR-38: the decoder has already read the 0x03 marker itself
            // (a0+=1); it then zero-fills [o,i) — always safe, strictly
            // behind unread input — before lockstep-copying the rest,
            // which lands at region_end by construction (i + remaining
            // stream bytes == region_end always, since i = i0+p+1 and the
            // remaining bytes are exactly stream.len()-(p+1)). So o<=i is
            // the whole condition.
            let i = i0 + p + 1;
            return if o <= i {
                Ok(())
            } else {
                Err((o, i.saturating_sub(o)))
            };
        }
        let len = read_u24(stream, p + 1).ok_or((o, 0usize))?;
        // The decoder has already read the 4-byte opcode+length header (a0
        // advances by 4) before the run's own write loop starts.
        let i = i0 + p + 4;
        (o, p) = advance_run(op, len, o, i, p, stream.len())?;
    }
}

/// Advance `(o, p)` past one zero/literal/skip run, or reject it per §4.4
/// (a write landing on unread input, or a truncated literal).
fn advance_run(
    op: u8,
    len: usize,
    o: usize,
    i: usize,
    p: usize,
    stream_len: usize,
) -> Result<(usize, usize), (usize, usize)> {
    match op {
        OP_ZERO => {
            if o + len > i {
                return Err((o, i.saturating_sub(o)));
            }
            Ok((o + len, p + 4))
        }
        OP_LITERAL => {
            if o > i || p + 4 + len > stream_len {
                return Err((o, i.saturating_sub(o)));
            }
            Ok((o + len, p + 4 + len))
        }
        OP_SKIP => Ok((o + len, p + 4)),
        _ => Err((o, 0)),
    }
}

/// Rust mirror of the 68k decoder (the round-trip test oracle). Hole bytes
/// come back as `0`. `Err` on a malformed stream (never on the packer's own
/// output — that is what `pack_unpack_round_trips` proves).
pub fn unpack(stream: &[u8], region_end: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; region_end];
    let mut o = 0usize;
    let mut p = 0usize;
    while p < stream.len() {
        let op = stream[p];
        p += 1;
        if op == OP_TAIL {
            fill_tail(&mut out, &stream[p..], o)?;
            return Ok(out);
        }
        let len = read_u24(stream, p).ok_or("truncated run length")?;
        p += 3;
        match op {
            OP_ZERO | OP_SKIP => o += len,
            OP_LITERAL => {
                let src = stream.get(p..p + len).ok_or("truncated literal")?;
                out[o..o + len].copy_from_slice(src);
                p += len;
                o += len;
            }
            other => return Err(format!("bad opcode {other:#04x}")),
        }
    }
    Err("stream ended without a 0x03 terminator".to_string())
}

/// ADR-38: zero-fill `[o, fill_end)` — a no-op here since `out` is already
/// zero-initialized — then place the raw `tail` data at `[fill_end,
/// out.len())`.
fn fill_tail(out: &mut [u8], tail: &[u8], o: usize) -> Result<(), String> {
    let fill_end = out
        .len()
        .checked_sub(tail.len())
        .ok_or("tail longer than region")?;
    if fill_end < o {
        return Err("tail fill would overwrite already-written output".to_string());
    }
    out[fill_end..].copy_from_slice(tail);
    Ok(())
}
