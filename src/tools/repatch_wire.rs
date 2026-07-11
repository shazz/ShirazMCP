//! mode="repatch" output shape + region plumbing for `save_prg` (§7.2),
//! split from `save_prg_wire.rs` for the 200-line budget — pure, unit-
//! tested without a session.

use rmcp::ErrorData as McpError;
use std::path::Path;

use crate::debug::basepage::Basepage;
use crate::ripper::prg_file::HEADER_LEN;
use crate::ripper::repatch::RepatchPlan;
use crate::types::Addr;

use super::mem::scan::parse_addr;
use super::save_prg_wire::RegionArg;

pub(crate) const APPLIED_RUNS_MAX: usize = 32;
pub(crate) const RELOC_SKIPS_MAX: usize = 16;

fn inv(msg: impl Into<String>) -> McpError {
    McpError::invalid_params(msg.into(), None)
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct BasepageOut {
    pub basepage: String,
    pub text_base: String,
    pub text_len: u32,
    pub data_base: String,
    pub data_len: u32,
    pub bss_base: String,
    pub bss_len: u32,
}

/// One applied patch run: RAM address "$…", byte offset in the output file, length.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct AppliedRunOut {
    pub addr: String,
    pub file_offset: usize,
    pub len: usize,
}
/// A patched relocated longword — reported, NOT persisted: address, file offset, file value, expected vs actual RAM.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct RelocSkipOut {
    pub addr: String,
    pub file_offset: usize,
    pub file_value: String,
    pub expected_ram: String,
    pub actual_ram: String,
}

/// mode="repatch" output (§7.2). Renamed from the pre-P22 `SavePrgOutput`
/// now that the tool has a second flavour (§8.1).
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct RepatchOutput {
    pub out_path: String,
    /// Output file size — identical to the source size (only bytes replace bytes, never insert).
    pub size: u64,
    pub basepage: BasepageOut,
    /// Total bytes differing between the file image and live RAM.
    pub changed_bytes: usize,
    /// Applied patch runs (first 32; see applied_truncated).
    pub applied: Vec<AppliedRunOut>,
    pub applied_truncated: bool,
    /// Patched relocated longwords — reported, NOT persisted (first 16).
    pub skipped_reloc: Vec<RelocSkipOut>,
    pub skipped_reloc_truncated: bool,
    /// Changed bytes excluded by the regions filter (0 when no filter).
    pub filtered_bytes: usize,
    /// Static fact: BSS/runtime state is never captured by mode=repatch.
    pub bss_note: String,
    pub warnings: Vec<String>,
}

/// `RegionArg`s -> validated `(seg_off, len)` pairs within `[text_base,
/// text_base + seg_len)`. Moved from `save_prg.rs` (line-budget rebalance,
/// P22) — pure arg plumbing, no session access.
pub(crate) fn convert_regions(
    regions: Option<&[RegionArg]>,
    bp: &Basepage,
) -> Result<Option<Vec<(usize, usize)>>, McpError> {
    let Some(regions) = regions else {
        return Ok(None);
    };
    let seg_len = (bp.text_len + bp.data_len) as usize;
    regions
        .iter()
        .map(|r| region_to_seg_off(r, bp.text_base, seg_len))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn region_to_seg_off(
    r: &RegionArg,
    text_base: Addr,
    seg_len: usize,
) -> Result<(usize, usize), McpError> {
    let addr = parse_addr(&r.addr)?;
    if r.len < 1 {
        return Err(inv("region len must be >= 1"));
    }
    let top = Addr::new(text_base.raw() + seg_len as u32);
    let start = addr
        .raw()
        .checked_sub(text_base.raw())
        .filter(|&s| s as usize + r.len <= seg_len)
        .ok_or_else(|| {
            inv(format!(
                "region {addr} is outside the loaded TEXT+DATA image ({text_base} .. {top})"
            ))
        })?;
    Ok((start as usize, r.len))
}

fn cap<T, U>(items: &[T], max: usize, f: impl Fn(&T) -> U) -> (Vec<U>, bool) {
    (items.iter().take(max).map(f).collect(), items.len() > max)
}

/// Pure summary builder for mode=repatch: caps applied/skipped lists,
/// formats addresses via `Addr` display / `"${:08X}"` longwords, fixed
/// `bss_note` text.
pub(crate) fn summarize(
    out_path: &Path,
    size: u64,
    bp: &Basepage,
    plan: &RepatchPlan,
    warnings: Vec<String>,
) -> RepatchOutput {
    let (applied, applied_truncated) = capped_applied(plan, bp);
    let (skipped_reloc, skipped_reloc_truncated) = capped_reloc_skips(plan, bp);
    RepatchOutput {
        out_path: out_path.display().to_string(),
        size,
        basepage: basepage_out(bp),
        changed_bytes: plan.changed_bytes,
        applied,
        applied_truncated,
        skipped_reloc,
        skipped_reloc_truncated,
        filtered_bytes: plan.filtered_bytes,
        bss_note: bss_note(bp),
        warnings,
    }
}

fn capped_applied(plan: &RepatchPlan, bp: &Basepage) -> (Vec<AppliedRunOut>, bool) {
    cap(&plan.runs, APPLIED_RUNS_MAX, |r| AppliedRunOut {
        addr: r.ram_addr(bp.text_base).to_string(),
        file_offset: r.file_offset(),
        len: r.len,
    })
}

fn capped_reloc_skips(plan: &RepatchPlan, bp: &Basepage) -> (Vec<RelocSkipOut>, bool) {
    cap(&plan.reloc_skips, RELOC_SKIPS_MAX, |r| RelocSkipOut {
        addr: Addr::new(bp.text_base.raw() + r.seg_off as u32).to_string(),
        file_offset: HEADER_LEN + r.seg_off,
        file_value: format!("${:08X}", r.file_value),
        expected_ram: format!("${:08X}", r.expected_ram),
        actual_ram: format!("${:08X}", r.actual_ram),
    })
}

fn basepage_out(bp: &Basepage) -> BasepageOut {
    BasepageOut {
        basepage: bp.addr.to_string(),
        text_base: bp.text_base.to_string(),
        text_len: bp.text_len,
        data_base: bp.data_base.to_string(),
        data_len: bp.data_len,
        bss_base: bp.bss_base.to_string(),
        bss_len: bp.bss_len,
    }
}

fn bss_note(bp: &Basepage) -> String {
    let bss_end = Addr::new(bp.bss_base.raw() + bp.bss_len);
    format!(
        "BSS ({} .. {bss_end}, {} bytes) and runtime state are not part of the .PRG file \
         and were not captured (mode=repatch)",
        bp.bss_base, bp.bss_len
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ripper::repatch::RelocSkip;

    fn basepage() -> Basepage {
        Basepage {
            addr: Addr::new(0x20000),
            text_base: Addr::new(0x20100),
            text_len: 0x1000,
            data_base: Addr::new(0x21100),
            data_len: 0x200,
            bss_base: Addr::new(0x21300),
            bss_len: 0x80,
        }
    }

    #[test]
    fn region_in_window_ok() {
        let bp = basepage();
        let r = RegionArg {
            addr: "$20114".to_string(),
            len: 6,
        };
        let (off, len) =
            region_to_seg_off(&r, bp.text_base, (bp.text_len + bp.data_len) as usize).unwrap();
        assert_eq!(off, 0x14);
        assert_eq!(len, 6);
    }

    #[test]
    fn region_out_of_window_rejected() {
        let bp = basepage();
        let r = RegionArg {
            addr: "$1000".to_string(),
            len: 4,
        };
        let err =
            region_to_seg_off(&r, bp.text_base, (bp.text_len + bp.data_len) as usize).unwrap_err();
        assert!(err.message.contains("outside the loaded TEXT+DATA image"));
    }

    #[test]
    fn region_zero_len_rejected() {
        let bp = basepage();
        let r = RegionArg {
            addr: "$20100".to_string(),
            len: 0,
        };
        assert!(region_to_seg_off(&r, bp.text_base, (bp.text_len + bp.data_len) as usize).is_err());
    }

    #[test]
    fn summarize_caps_lists() {
        let runs: Vec<_> = (0..40)
            .map(|i| crate::ripper::repatch::PatchRun { seg_off: i, len: 1 })
            .collect();
        let skips: Vec<_> = (0..20)
            .map(|i| RelocSkip {
                seg_off: i * 4,
                file_value: 0,
                expected_ram: 0,
                actual_ram: 0,
            })
            .collect();
        let plan = RepatchPlan {
            runs,
            reloc_skips: skips,
            changed_bytes: 0,
            filtered_bytes: 0,
        };
        let out = summarize(Path::new("/tmp/out.prg"), 100, &basepage(), &plan, vec![]);
        assert_eq!(out.applied.len(), 32);
        assert!(out.applied_truncated);
        assert_eq!(out.skipped_reloc.len(), 16);
        assert!(out.skipped_reloc_truncated);
    }

    #[test]
    fn summarize_formats() {
        let plan = RepatchPlan {
            runs: vec![crate::ripper::repatch::PatchRun { seg_off: 0, len: 4 }],
            reloc_skips: vec![RelocSkip {
                seg_off: 8,
                file_value: 0x10,
                expected_ram: 0x10010,
                actual_ram: 0x20020,
            }],
            changed_bytes: 8,
            filtered_bytes: 0,
        };
        let out = summarize(Path::new("/tmp/out.prg"), 100, &basepage(), &plan, vec![]);
        assert!(out.applied[0].addr.starts_with('$'));
        assert_eq!(out.skipped_reloc[0].file_value, "$00000010");
        assert!(out.bss_note.contains("not captured"));
    }
}
