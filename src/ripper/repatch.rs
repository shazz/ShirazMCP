//! RAM→file back-mapping for `save_prg` (P16): reconstruct the expected
//! in-RAM TEXT+DATA image (file bytes + relocation at the live load
//! address), diff against acquired RAM, split the differences into
//! file-representable patch runs vs relocated-longword skips, apply the
//! runs to a copy of the original file. Pure — unit-tested on synthetic
//! PRGs.

use std::collections::BTreeSet;

use super::prg_file::{PrgFile, HEADER_LEN};
use crate::types::Addr;

/// A maximal contiguous run of patched, file-representable bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchRun {
    /// Offset into the TEXT‖DATA segment image.
    pub seg_off: usize,
    pub len: usize,
}

impl PatchRun {
    pub fn file_offset(&self) -> usize {
        HEADER_LEN + self.seg_off
    }
    pub fn ram_addr(&self, text_base: Addr) -> Addr {
        Addr::new(text_base.raw() + self.seg_off as u32)
    }
}

/// A patched relocated longword — NOT file-representable (writing the RAM
/// value into the file would bake an absolute run-time address into a
/// relocatable image). Reported, never applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelocSkip {
    /// Segment offset of the relocated longword (== a `PrgFile::reloc` entry).
    pub seg_off: usize,
    /// The longword as stored in the file (pre-relocation).
    pub file_value: u32,
    /// file_value + text_base — what unpatched RAM should hold.
    pub expected_ram: u32,
    /// What RAM actually holds.
    pub actual_ram: u32,
}

/// The full diff result: file-representable runs, relocated-longword
/// skips, and the honesty counters behind `save_prg`'s divergence warning.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepatchPlan {
    pub runs: Vec<PatchRun>,
    pub reloc_skips: Vec<RelocSkip>,
    /// Total differing bytes (applied + skipped + filtered).
    pub changed_bytes: usize,
    /// Differing bytes excluded by the caller's region filter (0 = no filter). Reported, never applied.
    pub filtered_bytes: usize,
}

/// The §4.2 load replay: clone the segment image and add `text_base` to
/// the big-endian u32 at every reloc offset (wrapping add — TOS does 32-bit
/// adds too).
pub fn expected_ram(prg: &PrgFile, text_base: Addr) -> Vec<u8> {
    let mut out = prg.seg_image().to_vec();
    for &r in &prg.reloc {
        let cur = u32::from_be_bytes([out[r], out[r + 1], out[r + 2], out[r + 3]]);
        out[r..r + 4].copy_from_slice(&cur.wrapping_add(text_base.raw()).to_be_bytes());
    }
    out
}

enum Class {
    Reloc(usize),
    Filtered,
    Applied,
}

/// Binary search: does byte `i` lie in `[r, r+4)` for some `r` in the
/// sorted `reloc` offsets?
fn in_reloc(i: usize, reloc: &[usize]) -> Option<usize> {
    let idx = reloc.partition_point(|&r| r + 4 <= i);
    reloc.get(idx).filter(|&&r| r <= i && i < r + 4).copied()
}

fn classify(i: usize, reloc: &[usize], regions: Option<&[(usize, usize)]>) -> Class {
    if let Some(r) = in_reloc(i, reloc) {
        return Class::Reloc(r);
    }
    match regions {
        Some(rs) if !rs.iter().any(|&(off, len)| i >= off && i < off + len) => Class::Filtered,
        _ => Class::Applied,
    }
}

/// Accumulates maximal contiguous spans of `Class::Applied` bytes.
#[derive(Default)]
struct RunBuilder {
    runs: Vec<PatchRun>,
    open: Option<(usize, usize)>,
}

impl RunBuilder {
    fn push_applied(&mut self, i: usize) {
        match &mut self.open {
            Some((start, len)) if *start + *len == i => *len += 1,
            _ => {
                self.close();
                self.open = Some((i, 1));
            }
        }
    }
    fn close(&mut self) {
        if let Some((seg_off, len)) = self.open.take() {
            self.runs.push(PatchRun { seg_off, len });
        }
    }
}

/// Borrowed diff inputs + mutable accumulation state, combined so `record` stays under the arg-count lint.
struct Diff<'a> {
    prg: &'a PrgFile,
    expected: &'a [u8],
    actual: &'a [u8],
    regions: Option<&'a [(usize, usize)]>,
    plan: RepatchPlan,
    seen_reloc: BTreeSet<usize>,
    builder: RunBuilder,
}

impl<'a> Diff<'a> {
    fn record(&mut self, i: usize) {
        match classify(i, &self.prg.reloc, self.regions) {
            Class::Reloc(r) => {
                self.builder.close();
                if self.seen_reloc.insert(r) {
                    self.plan
                        .reloc_skips
                        .push(reloc_skip(self.prg, self.expected, self.actual, r));
                }
            }
            Class::Filtered => {
                self.builder.close();
                self.plan.filtered_bytes += 1;
            }
            Class::Applied => self.builder.push_applied(i),
        }
    }
}

fn reloc_skip(prg: &PrgFile, expected: &[u8], actual: &[u8], r: usize) -> RelocSkip {
    let word_at = |b: &[u8]| u32::from_be_bytes([b[r], b[r + 1], b[r + 2], b[r + 3]]);
    RelocSkip {
        seg_off: r,
        file_value: word_at(prg.seg_image()),
        expected_ram: word_at(expected),
        actual_ram: word_at(actual),
    }
}

/// Diff `actual` (the acquired RAM window, exactly seg_len bytes) against
/// `expected`; split per §6.2.1. `regions`: allowed segment-offset ranges
/// (None = everything). `text_base` is unused (kept for signature symmetry
/// with `expected_ram` — RAM addresses come from `PatchRun::ram_addr`).
pub fn diff(
    prg: &PrgFile,
    expected: &[u8],
    actual: &[u8],
    _text_base: Addr,
    regions: Option<&[(usize, usize)]>,
) -> RepatchPlan {
    let mut d = Diff {
        prg,
        expected,
        actual,
        regions,
        plan: RepatchPlan::default(),
        seen_reloc: BTreeSet::new(),
        builder: RunBuilder::default(),
    };
    for i in 0..expected.len().min(actual.len()) {
        if expected[i] == actual[i] {
            d.builder.close();
            continue;
        }
        d.plan.changed_bytes += 1;
        d.record(i);
    }
    d.builder.close();
    d.plan.runs = d.builder.runs;
    d.plan
}

/// Copy of the original file with every run's bytes replaced from `actual`
/// at `file_offset()`. Header/symbols/reloc table untouched.
pub fn apply(prg: &PrgFile, plan: &RepatchPlan, actual: &[u8]) -> Vec<u8> {
    let mut out = prg.bytes.clone();
    for run in &plan.runs {
        let fo = run.file_offset();
        out[fo..fo + run.len].copy_from_slice(&actual[run.seg_off..run.seg_off + run.len]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte-identical to `prg_file`'s §9.1 fixture (kept local — the P7
    /// convention of duplicating shared fixtures across test modules).
    fn fixture_prg() -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&0x601Au16.to_be_bytes());
        f.extend_from_slice(&32u32.to_be_bytes());
        f.extend_from_slice(&16u32.to_be_bytes());
        f.extend_from_slice(&8u32.to_be_bytes());
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&[0u8; 10]);
        f.extend((0u8..32).map(|i| i ^ 0xA5));
        f[28 + 8..28 + 12].copy_from_slice(&0x0000_0010u32.to_be_bytes());
        f.extend((0u8..16).map(|i| 0x40 + i));
        f.extend_from_slice(&8u32.to_be_bytes());
        f.push(0);
        f
    }

    fn prg() -> PrgFile {
        super::super::prg_file::parse(fixture_prg()).unwrap()
    }

    const TBASE: Addr = Addr::new(0x1_0000);

    #[test]
    fn expected_ram_applies_reloc() {
        let prg = prg();
        let exp = expected_ram(&prg, TBASE);
        assert_eq!(exp.len(), 48);
        assert_eq!(&exp[..8], &prg.seg_image()[..8]);
        assert_eq!(&exp[8..12], &(0x10 + 0x10000u32).to_be_bytes());
        assert_eq!(&exp[12..], &prg.seg_image()[12..]);
    }

    #[test]
    fn no_diff_no_runs() {
        let prg = prg();
        let exp = expected_ram(&prg, TBASE);
        let plan = diff(&prg, &exp, &exp.clone(), TBASE, None);
        assert_eq!(plan, RepatchPlan::default());
    }

    #[test]
    fn text_patch_maps_to_file_offset() {
        let prg = prg();
        let exp = expected_ram(&prg, TBASE);
        let mut actual = exp.clone();
        actual[20..26].copy_from_slice(&[0xAA; 6]);
        let plan = diff(&prg, &exp, &actual, TBASE, None);
        assert_eq!(
            plan.runs,
            vec![PatchRun {
                seg_off: 20,
                len: 6
            }]
        );
        assert_eq!(plan.runs[0].file_offset(), 48);
        assert_eq!(plan.runs[0].ram_addr(TBASE), Addr::new(0x10014));
        assert_eq!(plan.changed_bytes, 6);
    }

    #[test]
    fn data_patch_maps_past_text() {
        let prg = prg();
        let exp = expected_ram(&prg, TBASE);
        let mut actual = exp.clone();
        actual[36..40].copy_from_slice(&[0xBB; 4]);
        let plan = diff(&prg, &exp, &actual, TBASE, None);
        assert_eq!(
            plan.runs,
            vec![PatchRun {
                seg_off: 36,
                len: 4
            }]
        );
        assert_eq!(plan.runs[0].file_offset(), 64);
    }

    #[test]
    fn reloc_patch_skipped_not_applied() {
        let prg = prg();
        let exp = expected_ram(&prg, TBASE);
        let mut actual = exp.clone();
        actual[8..12].copy_from_slice(&0xAABB_CCDDu32.to_be_bytes());
        let plan = diff(&prg, &exp, &actual, TBASE, None);
        assert!(plan.runs.is_empty());
        assert_eq!(plan.reloc_skips.len(), 1);
        let skip = plan.reloc_skips[0];
        assert_eq!(skip.seg_off, 8);
        assert_eq!(skip.file_value, 0x10);
        assert_eq!(skip.expected_ram, 0x10010);
        assert_eq!(skip.actual_ram, 0xAABB_CCDD);
        assert_eq!(plan.changed_bytes, 4);
    }

    #[test]
    fn straddling_patch_splits() {
        let prg = prg();
        let exp = expected_ram(&prg, TBASE);
        let mut actual = exp.clone();
        actual[6..14].copy_from_slice(&[0xCC; 8]);
        let plan = diff(&prg, &exp, &actual, TBASE, None);
        assert_eq!(
            plan.runs,
            vec![
                PatchRun { seg_off: 6, len: 2 },
                PatchRun {
                    seg_off: 12,
                    len: 2
                },
            ]
        );
        assert_eq!(plan.reloc_skips.len(), 1);
        assert_eq!(plan.reloc_skips[0].seg_off, 8);
        assert_eq!(plan.changed_bytes, 8);
    }

    #[test]
    fn regions_filter_excludes() {
        let prg = prg();
        let exp = expected_ram(&prg, TBASE);
        let mut actual = exp.clone();
        actual[20..26].copy_from_slice(&[0xAA; 6]);
        actual[36..40].copy_from_slice(&[0xBB; 4]);
        let plan = diff(&prg, &exp, &actual, TBASE, Some(&[(20, 6)]));
        assert_eq!(
            plan.runs,
            vec![PatchRun {
                seg_off: 20,
                len: 6
            }]
        );
        assert_eq!(plan.filtered_bytes, 4);
    }

    #[test]
    fn apply_writes_runs_only() {
        let prg = prg();
        let exp = expected_ram(&prg, TBASE);
        let mut actual = exp.clone();
        actual[20..26].copy_from_slice(&[0xAA; 6]);
        actual[8..12].copy_from_slice(&0x0099_0000u32.to_be_bytes()); // reloc-patch present too
        let plan = diff(&prg, &exp, &actual, TBASE, None);
        let out = apply(&prg, &plan, &actual);
        let mut expected_file = fixture_prg();
        expected_file[48..54].copy_from_slice(&[0xAA; 6]);
        assert_eq!(out, expected_file);
        assert_eq!(out.len(), fixture_prg().len());
    }

    #[test]
    fn two_adjacent_diff_bytes_one_run() {
        let prg = prg();
        let exp = expected_ram(&prg, TBASE);
        let mut actual = exp.clone();
        actual[20] = 0xEE;
        actual[21] = 0xEF;
        let plan = diff(&prg, &exp, &actual, TBASE, None);
        assert_eq!(
            plan.runs,
            vec![PatchRun {
                seg_off: 20,
                len: 2
            }]
        );
    }
}
