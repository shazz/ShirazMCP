//! Wire schema + pure aggregation for `profile_raw` (ADR-47). Split from
//! `profile_raw.rs` (choreography) — the `profile_frame_wire.rs` precedent.
//! Everything here works on the parsed `profile save` dump, so the region
//! split / named ranges / top-N math is unit-tested without Hatari.

use std::path::Path;

use rmcp::ErrorData as McpError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::analyze::cycles;
use crate::debug::profile::ProfiledAddr;
use crate::types::Addr;

pub const PROFILE_RAW_MAX_FRAMES: u32 = 1_000;
pub const TOP_DEFAULT: usize = 40;
pub const TOP_MAX: usize = 1_000;
pub const RANGES_MAX: usize = 32;

/// Address regions of the split (ST memory map): [start, end) + name.
/// TOS 1.00-1.04 lives at $FC0000 and therefore reports as "other".
const REGIONS: [(&str, u32, u32); 3] = [
    ("st_ram", 0, 0xE0_0000),
    ("rom_tos", 0xE0_0000, 0xE4_0000),
    ("cartridge", 0xFA_0000, 0xFC_0000),
];

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProfileRawArgs {
    /// VBL frames to run in ONE profiler window (1..=1000).
    pub frames: u32,
    /// Named address ranges to total, e.g. [{name:"blitter",
    /// from:"$1a000", to:"$1a400"}] (≤ 32). from/to take $hex, 0xhex,
    /// decimal or a symbol name; `to` is exclusive.
    pub ranges: Option<Vec<RangeArg>>,
    /// Hottest addresses to list by cycles (default 40, max 1000).
    pub top: Option<usize>,
    /// Wait budget for the N-frame window, ms. Default
    /// `frames::default_timeout(frames)`.
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RangeArg {
    pub name: String,
    pub from: String,
    pub to: String,
}

/// A range after address resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NamedRange {
    pub name: String,
    pub from: Addr,
    pub to: Addr,
}

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
pub struct ProfileRawOutput {
    pub frames: u32,
    /// Measured cycles over every executed address in the window.
    pub total_cycles: u64,
    pub total_instructions: u64,
    /// frames × 160256 (8 MHz PAL frame).
    pub expected_cycles: u64,
    /// total_cycles / expected_cycles — ~1.0 for a healthy window; well
    /// below means the profiler missed time (e.g. STOP / halted CPU).
    pub coverage: f64,
    /// st_ram (< $E00000) / rom_tos ($E00000-$E40000) / cartridge
    /// ($FA0000-$FC0000) / other.
    pub regions: Vec<RegionOut>,
    /// One entry per requested range, in request order.
    pub ranges: Vec<RangeOut>,
    /// Hottest addresses by cycles (ties: lower address first).
    pub top: Vec<HotAddr>,
    /// The raw `profile save` dump (console fallback applied).
    pub dump_path: String,
    /// Always "paused" on success — frozen at the last frame boundary.
    pub state: String,
}

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
pub struct RegionOut {
    pub name: String,
    pub cycles: u64,
    /// Share of total_cycles, 0..100.
    pub pct: f64,
    pub instructions: u64,
}

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
pub struct RangeOut {
    pub name: String,
    pub from: String,
    pub to: String,
    pub cycles: u64,
    pub pct: f64,
    pub instructions: u64,
    /// Executions of the instruction AT `from` — calls of the routine
    /// when `from` is its entry point.
    pub entry_hits: u64,
}

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
pub struct HotAddr {
    pub addr: String,
    pub cycles: u64,
    pub pct: f64,
    /// Executions of this instruction.
    pub count: u64,
}

pub(crate) fn validate(a: &ProfileRawArgs) -> Result<usize, McpError> {
    if !(1..=PROFILE_RAW_MAX_FRAMES).contains(&a.frames) {
        return Err(McpError::invalid_params(
            format!("frames must be 1..={PROFILE_RAW_MAX_FRAMES}"),
            None,
        ));
    }
    let top = a.top.unwrap_or(TOP_DEFAULT);
    if top > TOP_MAX {
        return Err(McpError::invalid_params(
            format!("top must be <= {TOP_MAX}"),
            None,
        ));
    }
    let ranges = a.ranges.as_deref().unwrap_or_default();
    if ranges.len() > RANGES_MAX {
        return Err(McpError::invalid_params(
            format!("at most {RANGES_MAX} ranges"),
            None,
        ));
    }
    if ranges.iter().any(|r| r.name.trim().is_empty()) {
        return Err(McpError::invalid_params(
            "range name must be non-empty",
            None,
        ));
    }
    Ok(top)
}

pub(crate) fn check_range(r: &NamedRange) -> Result<(), McpError> {
    if r.to.raw() <= r.from.raw() {
        return Err(McpError::invalid_params(
            format!("range {}: to must be greater than from", r.name),
            None,
        ));
    }
    Ok(())
}

fn pct(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 / total as f64 * 100.0
    }
}

fn sum_where(entries: &[ProfiledAddr], keep: impl Fn(u32) -> bool) -> (u64, u64) {
    entries
        .iter()
        .filter(|e| keep(e.addr.raw()))
        .fold((0, 0), |(i, c), e| (i + e.count, c + e.cycles))
}

fn in_region(addr: u32) -> Option<&'static str> {
    REGIONS
        .iter()
        .find(|(_, lo, hi)| (*lo..*hi).contains(&addr))
        .map(|(name, _, _)| *name)
}

fn regions(entries: &[ProfiledAddr], total: u64) -> Vec<RegionOut> {
    let names = REGIONS.iter().map(|(n, _, _)| *n).chain(["other"]);
    names
        .map(|name| {
            let (instructions, cycles) =
                sum_where(entries, |a| in_region(a).unwrap_or("other") == name);
            RegionOut {
                name: name.to_string(),
                cycles,
                pct: pct(cycles, total),
                instructions,
            }
        })
        .collect()
}

fn range_out(entries: &[ProfiledAddr], r: &NamedRange, total: u64) -> RangeOut {
    let (from, to) = (r.from.raw(), r.to.raw());
    let (instructions, cycles) = sum_where(entries, |a| a >= from && a < to);
    let entry_hits = entries
        .iter()
        .filter(|e| e.addr.raw() == from)
        .map(|e| e.count)
        .sum();
    RangeOut {
        name: r.name.clone(),
        from: r.from.to_string(),
        to: r.to.to_string(),
        cycles,
        pct: pct(cycles, total),
        instructions,
        entry_hits,
    }
}

fn top_n(entries: &[ProfiledAddr], n: usize, total: u64) -> Vec<HotAddr> {
    let mut sorted: Vec<&ProfiledAddr> = entries.iter().collect();
    sorted.sort_by(|x, y| {
        y.cycles
            .cmp(&x.cycles)
            .then(x.addr.raw().cmp(&y.addr.raw()))
    });
    sorted
        .into_iter()
        .take(n)
        .map(|e| HotAddr {
            addr: e.addr.to_string(),
            cycles: e.cycles,
            pct: pct(e.cycles, total),
            count: e.count,
        })
        .collect()
}

/// Assemble the output (pure). Errors — never zeros — when the dump held
/// no per-address data (stub file AND empty console fallback).
pub(crate) fn build_output(
    frames: u32,
    entries: &[ProfiledAddr],
    ranges: &[NamedRange],
    top: usize,
    path: &Path,
) -> Result<ProfileRawOutput, McpError> {
    if entries.is_empty() {
        return Err(McpError::internal_error(
            format!(
                "profile dump {} has no per-address entries (Hatari wrote a stub \
                 and the console fallback had no data)",
                path.display()
            ),
            None,
        ));
    }
    let (total_instructions, total_cycles) = sum_where(entries, |_| true);
    let expected_cycles = u64::from(frames) * u64::from(cycles::VBL_CYCLES);
    Ok(ProfileRawOutput {
        frames,
        total_cycles,
        total_instructions,
        expected_cycles,
        coverage: total_cycles as f64 / expected_cycles as f64,
        regions: regions(entries, total_cycles),
        ranges: ranges
            .iter()
            .map(|r| range_out(entries, r, total_cycles))
            .collect(),
        top: top_n(entries, top, total_cycles),
        dump_path: path.display().to_string(),
        state: "paused".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(addr: u32, count: u64, cycles: u64) -> ProfiledAddr {
        ProfiledAddr {
            addr: Addr::new(addr),
            count,
            cycles,
        }
    }

    /// 2 frames' worth: 200000 RAM + 100000 TOS + 20000 cart + 512 other.
    fn sample() -> Vec<ProfiledAddr> {
        vec![
            e(0x1_a000, 10, 150_000),
            e(0x1_a002, 10, 50_000),
            e(0xE0_0800, 5, 100_000),
            e(0xFA_0010, 2, 20_000),
            e(0xFC_0020, 1, 512),
        ]
    }

    fn range(name: &str, from: u32, to: u32) -> NamedRange {
        NamedRange {
            name: name.to_string(),
            from: Addr::new(from),
            to: Addr::new(to),
        }
    }

    #[test]
    fn totals_expected_and_coverage() {
        let out = build_output(2, &sample(), &[], 40, Path::new("/w/p.txt")).unwrap();
        assert_eq!(out.total_cycles, 320_512);
        assert_eq!(out.total_instructions, 28);
        assert_eq!(out.expected_cycles, 2 * 160_256);
        assert_eq!(out.coverage, 1.0);
        assert_eq!(out.dump_path, "/w/p.txt");
        assert_eq!(out.state, "paused");
    }

    #[test]
    fn region_split_boundaries() {
        let out = build_output(2, &sample(), &[], 40, Path::new("/w/p.txt")).unwrap();
        let got: Vec<(&str, u64, u64)> = out
            .regions
            .iter()
            .map(|r| (r.name.as_str(), r.cycles, r.instructions))
            .collect();
        assert_eq!(
            got,
            vec![
                ("st_ram", 200_000, 20),
                ("rom_tos", 100_000, 5),
                ("cartridge", 20_000, 2),
                ("other", 512, 1),
            ]
        );
        assert_eq!(in_region(0xDF_FFFE), Some("st_ram"));
        assert_eq!(in_region(0xE3_FFFE), Some("rom_tos"));
        assert_eq!(in_region(0xE4_0000), None);
        assert_eq!(in_region(0xFB_FFFE), Some("cartridge"));
    }

    #[test]
    fn named_range_cycles_pct_and_entry_hits() {
        let r = [
            range("loop", 0x1_a000, 0x1_a002),
            range("both", 0x1_a000, 0x1_a004),
        ];
        let out = build_output(2, &sample(), &r, 40, Path::new("/p")).unwrap();
        let only_first = &out.ranges[0];
        assert_eq!(only_first.cycles, 150_000); // `to` is exclusive
        assert_eq!(only_first.entry_hits, 10);
        assert_eq!(only_first.from, "$01A000");
        let both = &out.ranges[1];
        assert_eq!((both.cycles, both.instructions), (200_000, 20));
        assert_eq!(both.pct, 200_000.0 / 320_512.0 * 100.0);
    }

    #[test]
    fn entry_hits_zero_when_from_never_executed() {
        let r = [range("mid", 0x1_a001, 0x1_a100)];
        let out = build_output(1, &sample(), &r, 40, Path::new("/p")).unwrap();
        assert_eq!(out.ranges[0].entry_hits, 0);
        assert_eq!(out.ranges[0].cycles, 50_000);
    }

    #[test]
    fn top_n_sorted_by_cycles_ties_by_addr_and_truncated() {
        let mut entries = sample();
        entries.push(e(0x10, 1, 50_000)); // ties with $1A002
        let out = build_output(2, &entries, &[], 3, Path::new("/p")).unwrap();
        let addrs: Vec<&str> = out.top.iter().map(|h| h.addr.as_str()).collect();
        assert_eq!(addrs, vec!["$01A000", "$E00800", "$000010"]);
        assert_eq!(out.top[0].count, 10);
    }

    #[test]
    fn empty_dump_is_an_error_not_zeros() {
        let err = build_output(1, &[], &[], 40, Path::new("/w/p.txt")).unwrap_err();
        assert!(err.message.contains("no per-address entries"));
    }

    #[test]
    fn validate_bounds() {
        let args = |frames, top, n: usize| ProfileRawArgs {
            frames,
            ranges: Some(vec![
                RangeArg {
                    name: "r".into(),
                    from: "$0".into(),
                    to: "$2".into()
                };
                n
            ]),
            top,
            timeout_ms: None,
        };
        assert_eq!(validate(&args(1, None, 0)).unwrap(), TOP_DEFAULT);
        assert!(validate(&args(0, None, 0)).is_err());
        assert!(validate(&args(PROFILE_RAW_MAX_FRAMES + 1, None, 0)).is_err());
        assert!(validate(&args(1, Some(TOP_MAX + 1), 0)).is_err());
        assert!(validate(&args(1, None, RANGES_MAX + 1)).is_err());
        assert!(check_range(&range("bad", 0x10, 0x10)).is_err());
        assert!(check_range(&range("ok", 0x10, 0x12)).is_ok());
    }
}
