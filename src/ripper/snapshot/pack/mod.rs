//! SNAP1: the purpose-built zero-run/literal/skip/raw-tail codec for the
//! snapshot RAM payload (P22 §4.3/§4.4). Pure, no session/emulator imports.
//! The packer *proves* in-place decodability at pack time (`verify_inplace`)
//! rather than merely hoping the invariant holds — the load-bearing
//! correctness argument for a full-RAM self-restoring `.PRG`. Stream
//! encode/decode (`build_from_runs`/`verify_inplace`/`unpack`) lives in
//! `codec.rs` (200-line budget split); this file owns run decomposition
//! and the `pack` search.
//!
//! Safety argument (§4.4), restated as the implementation follows it: let
//! `S = i - o` be the decoder's live "slack" (how far the unread input is
//! ahead of the written output). A literal run of `n` costs a 4-byte
//! header the reader consumes without any matching output, so it *adds*
//! exactly 4 to `S`; a zero run of `n` costs the same 4-byte header but
//! writes `n` unread-input-free zero bytes, so it *removes* `n - 4` from
//! `S` — the only run that can drive `S` negative (a literal run's own
//! byte-by-byte copy loop needs only `o ≤ i`, identical to the raw tail).
//! So the one and only failure mode is an *interior* zero run whose length
//! exceeds the slack available where it sits; `pack` demotes one failing
//! zero run per retry (never un-demotes, so the search is bounded by the
//! zero-run count and terminates).
//!
//! ADR-38: the *terminal* zero run (whatever, if anything, is still zero
//! right before `image.len()`) is a different story — P22's first-cut
//! `0x03` ("copy stream bytes until output == region end") gave that run
//! no way to bank the slack it needed, so it failed by exactly 1 byte no
//! matter its size, and demoting it (the only fix available) inflated
//! payload by the *whole* gap — cascading to `Dense` for exactly the
//! half-zero RAM this codec exists to pack. `0x03` is redefined here (and
//! in `stub.s`) as *zero-fill output until it reaches the input cursor,
//! then copy remaining input to region end*: a strict generalization
//! (empty fill when `o == i`) that costs the terminal gap **zero** stream
//! bytes instead of its own header, and is unconditionally safe (the fill
//! only ever touches `[o, i)`, strictly behind unread input). `build_from_runs`
//! absorbs an immediately-preceding `Zero` run into this implicit fill
//! (regardless of `ZERO_MIN` — the fill is free) instead of writing a
//! separate `0x01` run or the run's raw zero bytes into the tail.
//! `Dense` iff the resulting payload still cannot fit `budget`.

mod codec;

use std::ops::Range;

use codec::build_from_runs;
pub use codec::{unpack, verify_inplace};

/// Zero runs shorter than this are folded into literals — the 4-byte
/// zero-run header must pay for itself with margin (§4.3).
pub const ZERO_MIN: usize = 16;

/// Stage-0 code + slack reserve subtracted from the restore budget (§6.2):
/// the payload's own start (`region_end - payload_len`) must clear the
/// hole's end by this much, or its staging backward-copy (which runs
/// *after* stage-0's forward-copy of the hole block) clobbers the just-
/// installed decoder before it runs — a structural requirement distinct
/// from (and not visible to) `verify_inplace`'s per-run simulation.
const STAGE0_RESERVE: usize = 0x400;

/// Bytes `[0, LOW_MEM_RESERVED)` — the 68000 reset SSP/PC — are never
/// written by the decode sweep, in *any* CPU mode: real ST hardware (and
/// Hatari's accurate emulation of it, confirmed live) bus-errors on any
/// write to address `< 8` unconditionally, supervisor mode included
/// (`SysMem_bput`, hatari `src/cpu/memory.c`: `addr < 0x8` is checked
/// before the supervisor-only `addr < 0x800` gate, not gated by it). A
/// frozen program's own `[0,8)` is always just the boot-time reset
/// vectors anyway — restoring them would be a no-op even if it worked.
const LOW_MEM_RESERVED: usize = 8;

/// The RAM image cannot be packed to fit its own restore budget (§4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackError {
    Dense { payload: usize, budget: usize },
}

/// One greedily-decomposed run, keyed by its starting image offset (the
/// key `pack`'s retry loop demotes by, matching `verify_inplace`'s `Err(o)`).
#[derive(Clone, Copy)]
pub(super) enum Run {
    Zero { start: usize, len: usize },
    Literal { start: usize, len: usize },
    Skip { start: usize, len: usize },
}

impl Run {
    fn start(&self) -> usize {
        match *self {
            Run::Zero { start, .. } | Run::Literal { start, .. } | Run::Skip { start, .. } => start,
        }
    }
}

fn zero_run_len(chunk: &[u8]) -> usize {
    chunk.iter().take_while(|&&b| b == 0).count()
}

/// A literal run extends until the next zero run of `>= ZERO_MIN` bytes
/// (or the chunk ends).
fn literal_run_len(chunk: &[u8]) -> usize {
    let mut i = 1;
    while i < chunk.len() {
        if chunk[i] == 0 && zero_run_len(&chunk[i..]) >= ZERO_MIN {
            break;
        }
        i += 1;
    }
    i
}

/// Greedy zero/literal decomposition of `chunk` (image offsets starting at
/// `base`), folding zero runs `< ZERO_MIN` into the surrounding literal.
fn decompose_greedy(chunk: &[u8], base: usize) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < chunk.len() {
        let run = zero_run_len(&chunk[i..]);
        if run >= ZERO_MIN {
            runs.push(Run::Zero {
                start: base + i,
                len: run,
            });
            i += run;
        } else {
            let len = literal_run_len(&chunk[i..]);
            runs.push(Run::Literal {
                start: base + i,
                len,
            });
            i += len;
        }
    }
    runs
}

/// A fixed `Skip` over `[0, LOW_MEM_RESERVED)` + `[LOW_MEM_RESERVED,
/// hole.start)` greedy + one `Skip` over `hole` (if nonempty) +
/// `[hole.end, image.len())` greedy — the full run sequence `pack` demotes
/// runs within.
fn decompose_all(image: &[u8], hole: &Range<usize>) -> Vec<Run> {
    let mut runs = vec![Run::Skip {
        start: 0,
        len: LOW_MEM_RESERVED,
    }];
    runs.extend(decompose_greedy(
        &image[LOW_MEM_RESERVED..hole.start],
        LOW_MEM_RESERVED,
    ));
    if !hole.is_empty() {
        runs.push(Run::Skip {
            start: hole.start,
            len: hole.len(),
        });
    }
    runs.extend(decompose_greedy(&image[hole.end..], hole.end));
    runs
}

/// Greedy decomposition, then demote violating zero runs to literals one
/// at a time (§ module doc — partial splitting cannot help: the slack
/// available to a zero run at any given split point is a constant
/// independent of where the split lands, so the fix must drop the run's
/// `o + len ≤ i` requirement entirely by re-encoding it as a literal,
/// which only needs `o ≤ i`) until `verify_inplace` accepts the stream
/// and it fits `budget`, or a failing run isn't a demotable `Zero` and
/// there's nothing left to try (`Dense`).
pub fn pack(image: &[u8], hole: Range<usize>) -> Result<Vec<u8>, PackError> {
    let region_end = image.len();
    let budget = region_end
        .saturating_sub(hole.end)
        .saturating_sub(STAGE0_RESERVE);
    let mut runs = decompose_all(image, &hole);
    loop {
        let stream = build_from_runs(image, &runs);
        if stream.len() > budget {
            return Err(PackError::Dense {
                payload: stream.len(),
                budget,
            });
        }
        match verify_inplace(&stream, region_end) {
            Ok(()) => return Ok(stream),
            Err((o_fail, _margin)) => {
                if !demote(&mut runs, o_fail) {
                    return Err(PackError::Dense {
                        payload: stream.len(),
                        budget,
                    });
                }
            }
        }
    }
}

/// Find the `Zero` run starting at `o_fail` and re-encode it as a
/// `Literal` (§ module doc). `false` iff no such run exists (`Dense`).
fn demote(runs: &mut [Run], o_fail: usize) -> bool {
    let Some(idx) = runs
        .iter()
        .position(|r| r.start() == o_fail && matches!(r, Run::Zero { .. }))
    else {
        return false;
    };
    let Run::Zero { start, len } = runs[idx] else {
        unreachable!("position() just matched Run::Zero");
    };
    runs[idx] = Run::Literal { start, len };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::{push_run, OP_TAIL, OP_ZERO};

    fn lcg_image(len: usize, seed: u32) -> Vec<u8> {
        let mut x = seed;
        (0..len)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect()
    }

    // A realistic Atari ST RAM size (Hatari's own default,
    // `HatariConfig::default().memsize_mb == 1`).
    const RAM_LEN: usize = 1 << 20;
    // Half the region as incompressible "used" content anchored at the top
    // (so nothing needs to reach exactly `region_end` as an unencodable
    // trailing zero run — see the module doc's `-1`-slack finding) leaves
    // a huge margin between the packer's near-minimal payload and
    // `budget`: comfortable headroom for these fixtures, not a probe of
    // the exact packable boundary (that isn't this phase's concern).
    const USED_LEN: usize = RAM_LEN / 2;

    fn half_zero_image(seed: u32) -> Vec<u8> {
        let mut image = vec![0u8; RAM_LEN];
        image[RAM_LEN - USED_LEN..].copy_from_slice(&lcg_image(USED_LEN, seed));
        image
    }

    #[test]
    fn pack_unpack_round_trips() {
        let mut image = half_zero_image(7);
        image[0x2000..0x2100].copy_from_slice(&lcg_image(0x100, 7));
        let hole = 0x1C0..0x400;
        let stream = pack(&image, hole.clone()).unwrap();
        let decoded = unpack(&stream, image.len()).unwrap();
        let mut expected = image.clone();
        expected[hole].fill(0);
        assert_eq!(decoded, expected);
    }

    /// Live-verified (P22 resume debugging): a write to any address `< 8`
    /// bus-errors on real ST hardware and in Hatari's accurate emulation
    /// of it, unconditionally — even from supervisor mode. `[0,8)` must
    /// never appear in the decode sweep's output, regardless of what the
    /// frozen image held there.
    #[test]
    fn low_mem_reset_vectors_never_restored() {
        let mut image = half_zero_image(3);
        image[0..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x11, 0x22, 0x33, 0x44]);
        let hole = 0x1C0..0x400;
        let stream = pack(&image, hole).unwrap();
        assert!(verify_inplace(&stream, image.len()).is_ok());
        let decoded = unpack(&stream, image.len()).unwrap();
        assert_eq!(
            &decoded[0..8],
            &[0u8; 8],
            "[0,8) must decode as untouched/zero, never the frozen bytes"
        );
    }

    #[test]
    fn packed_stream_is_inplace_safe() {
        for seed in [1u32, 2, 3] {
            // Half-zero RAM (§4.3's stated typical case): a zero gap
            // followed by incompressible "used" content, not a wholly
            // zero-free buffer (that scenario is `dense_image_errors`).
            let image = half_zero_image(seed);
            let hole = 0x1C0..0x400;
            let stream = pack(&image, hole).unwrap();
            assert!(verify_inplace(&stream, image.len()).is_ok());
        }
    }

    #[test]
    fn trailing_zeros_force_tail_extension() {
        // A large zero run positioned so it can't reach `region_end`
        // (there's real "used" content after it) still needs the packer
        // to notice it's too big for the zero-run opcode's own safety
        // check and split off the overflow (module doc) — this is the
        // load-bearing safety test, not the exact-boundary one.
        let mut image = half_zero_image(11);
        image[0x1C0..RAM_LEN - USED_LEN].fill(0); // ensure the whole gap is zero
        let stream = pack(&image, 0x1C0..0x400).unwrap();
        assert!(verify_inplace(&stream, image.len()).is_ok());
        let decoded = unpack(&stream, image.len()).unwrap();
        let mut expected = image.clone();
        expected[0x1C0..0x400].fill(0);
        assert_eq!(decoded, expected);
    }

    #[test]
    fn dense_image_errors() {
        // Region == hole.end + STAGE0_RESERVE exactly -> budget == 0: any
        // nonzero payload (unavoidable — skip + tail overhead alone is
        // nonzero) exceeds it, regardless of content.
        let image = lcg_image(0x400 + STAGE0_RESERVE, 42);
        let hole = 0x1C0..0x400;
        let err = pack(&image, hole).unwrap_err();
        assert!(matches!(err, PackError::Dense { budget: 0, .. }));
    }

    #[test]
    fn zero_min_folding() {
        let mut chunk = vec![0xFFu8; 4];
        chunk.extend(std::iter::repeat_n(0u8, ZERO_MIN - 1)); // below threshold
        chunk.extend_from_slice(&[0xFF, 0xFF]);
        let runs = decompose_greedy(&chunk, 0);
        // A single literal run covers the whole chunk (short zero run folded in).
        assert_eq!(runs.len(), 1);
        assert!(matches!(runs[0], Run::Literal { len, .. } if len == chunk.len()));
    }

    #[test]
    fn skip_emitted_once_for_hole() {
        let mut image = half_zero_image(5);
        let hole = 0x1C0..0x400;
        image[hole.clone()].fill(0xEE); // hole content must be ignored on decode
        let stream = pack(&image, hole.clone()).unwrap();
        let decoded = unpack(&stream, image.len()).unwrap();
        assert_eq!(&decoded[hole.clone()], &vec![0u8; hole.len()][..]);
        let mut expected = image.clone();
        expected[hole].fill(0);
        assert_eq!(decoded, expected);
    }

    #[test]
    fn verify_inplace_rejects_bad_stream() {
        // A zero run of 200 bytes where only 195 bytes of slack exist:
        // o + len (200) > i (195) — a write landing on unread input.
        let mut stream = Vec::new();
        push_run(&mut stream, OP_ZERO, 200, None);
        stream.push(OP_TAIL);
        stream.extend(vec![0xFFu8; 56]);
        let region_end = 256;
        assert_eq!(region_end - stream.len(), 195);
        let (off, margin) = verify_inplace(&stream, region_end).unwrap_err();
        assert_eq!(off, 0);
        assert_eq!(margin, 199); // i0(195) + the run's own 4-byte header
    }
}
