//! Wire schema + pure math for `profile_frame`. Split from `profile_frame.rs`
//! (choreography) for the file-size ceiling — the `profile`/`profile_wire.rs`
//! precedent.

use std::path::{Path, PathBuf};

use rmcp::ErrorData as McpError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::mem::scan::parse_addr;
use crate::analyze::cycles;
use crate::types::Addr;

pub const PROFILE_FRAME_MAX: u32 = 200;
/// Max ST RAM extent (4 MiB) — the "extent must lie within ST RAM" gate.
const MAX_RAM_END: u32 = 0x40_0000;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProfileFrameArgs {
    /// VBL frames to sample (1..=200). One profiler window per frame.
    pub frames: u32,
    /// Address-filter extent start (hex "$af3c" or decimal, `profile`'s
    /// parser). Give with `to` or not at all.
    pub from: Option<String>,
    /// Address-filter extent end (exclusive). Cycles at [from,to) report as
    /// range_* ("your code"); everything executed always reports as total_*.
    pub to: Option<String>,
    /// Per-frame-leg wait budget, ms. Default `frames::default_timeout(1)`.
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfileFrameOutput {
    /// Frames requested == frames measured (hard-errors on a short sample).
    pub frames: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Series fits_50hz/headroom_pct/vbl_fraction/hbl_count are computed
    /// from: "range" when from/to given, else "total" (~0% headroom by
    /// construction for a vsync-locked program on a "total" basis).
    pub cost_basis: String,
    /// Whole-frame measured cycles (your code, interrupts, TOS, idle spin).
    /// ~160256/frame at 50 Hz — a validity check, not a cost metric.
    pub total_cycles_min: u64,
    pub total_cycles_avg: f64,
    pub total_cycles_max: u64,
    /// Cycles at addresses inside [from,to) per frame — "your code". Present
    /// iff from/to given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_cycles_min: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_cycles_avg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_cycles_max: Option<u64>,
    /// Per-frame series, index 0 = first sampled frame (spot periodic
    /// spikes, e.g. every-4th-frame music work).
    pub per_frame_total_cycles: Vec<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_frame_range_cycles: Option<Vec<u64>>,
    /// cost_avg / 160256 — average frames-worth of work per frame.
    pub vbl_fraction: f64,
    /// cost_avg / 512 — average scanlines of work per frame.
    pub hbl_count: f64,
    /// max(cost series) <= 160256 — the WORST frame, not the average.
    pub fits_50hz: bool,
    /// (160256 - max(cost)) / 160256 * 100. Negative when over budget.
    pub headroom_pct: f64,
    /// 1-based index of the max-cost frame.
    pub worst_frame: u32,
    /// Raw profiler dump of the worst frame — drill in for hot spots.
    pub worst_frame_path: String,
    /// Directory holding all N per-frame dumps (frame-0001.txt …).
    pub profile_dir: String,
    /// Always "paused" on success — frozen at the last frame boundary.
    pub state: String,
}

/// `dir/frame-{i:04}.txt` — shared by the per-frame save path
/// (`profile_frame.rs`) and the worst-frame path echoed back here.
pub(crate) fn frame_path(dir: &Path, i: u32) -> PathBuf {
    dir.join(format!("frame-{i:04}.txt"))
}

/// §4.1 validation: frames in range, then from/to both-or-neither, parsed
/// and checked as an address extent (a pure FILTER, not a measurement
/// window — see the tool description).
pub(crate) fn validate(a: &ProfileFrameArgs) -> Result<Option<(Addr, Addr)>, McpError> {
    if !(1..=PROFILE_FRAME_MAX).contains(&a.frames) {
        return Err(McpError::invalid_params(
            format!("frames must be 1..={PROFILE_FRAME_MAX}"),
            None,
        ));
    }
    match (a.from.as_deref(), a.to.as_deref()) {
        (None, None) => Ok(None),
        (Some(f), Some(t)) => Ok(Some(parse_extent(f, t)?)),
        _ => Err(McpError::invalid_params(
            "from and to must be given together",
            None,
        )),
    }
}

fn parse_extent(from: &str, to: &str) -> Result<(Addr, Addr), McpError> {
    let from = parse_addr(from)?;
    let to = parse_addr(to)?;
    if to.raw() <= from.raw() {
        return Err(McpError::invalid_params(
            "to must be greater than from",
            None,
        ));
    }
    if to.raw() > MAX_RAM_END {
        return Err(McpError::invalid_params(
            "extent must lie within ST RAM (to <= $400000)",
            None,
        ));
    }
    Ok((from, to))
}

/// (min, avg, max) over a cost series — never empty in practice (`frames`
/// validates to `1..=200`); `unwrap_or(0)` is a defensive fallback, not a
/// reachable path.
pub(crate) fn stats_min_avg_max(v: &[u64]) -> (u64, f64, u64) {
    let min = v.iter().copied().min().unwrap_or(0);
    let max = v.iter().copied().max().unwrap_or(0);
    let sum: u64 = v.iter().sum();
    (min, sum as f64 / v.len().max(1) as f64, max)
}

/// 0-based index of the max element; the FIRST such index on a tie.
pub(crate) fn worst_index(v: &[u64]) -> usize {
    v.iter()
        .enumerate()
        .fold(0, |best, (i, &val)| if val > v[best] { i } else { best })
}

pub(crate) fn fits_50hz(max_cost: u64) -> bool {
    max_cost <= u64::from(cycles::VBL_CYCLES)
}

/// May be negative — deliberately not clamped (over-budget is the point).
pub(crate) fn headroom_pct(max_cost: u64) -> f64 {
    (f64::from(cycles::VBL_CYCLES) - max_cost as f64) / f64::from(cycles::VBL_CYCLES) * 100.0
}

/// (basis name, cost_avg, cost_max, worst 0-based index) over whichever
/// series the budget numbers are defined on: range if given, else total.
fn cost_summary(totals: &[u64], ranges: Option<&[u64]>) -> (&'static str, f64, u64, usize) {
    let series = ranges.unwrap_or(totals);
    let (_, avg, max) = stats_min_avg_max(series);
    let basis = if ranges.is_some() { "range" } else { "total" };
    (basis, avg, max, worst_index(series))
}

/// Assemble the wire output (pure — the unit-test seam for the averaging /
/// headroom / worst-frame math, §10.1).
pub(crate) fn build_output(
    frames: u32,
    extent: Option<(Addr, Addr)>,
    totals: Vec<u64>,
    ranges: Option<Vec<u64>>,
    dir: &Path,
) -> ProfileFrameOutput {
    let (t_min, t_avg, t_max) = stats_min_avg_max(&totals);
    let range_stats = ranges.as_deref().map(stats_min_avg_max);
    let (cost_basis, cost_avg, cost_max, worst) = cost_summary(&totals, ranges.as_deref());
    let worst_frame = worst as u32 + 1;
    ProfileFrameOutput {
        frames,
        from: extent.map(|(f, _)| f.to_string()),
        to: extent.map(|(_, t)| t.to_string()),
        cost_basis: cost_basis.to_string(),
        total_cycles_min: t_min,
        total_cycles_avg: t_avg,
        total_cycles_max: t_max,
        range_cycles_min: range_stats.map(|(min, _, _)| min),
        range_cycles_avg: range_stats.map(|(_, avg, _)| avg),
        range_cycles_max: range_stats.map(|(_, _, max)| max),
        per_frame_total_cycles: totals,
        per_frame_range_cycles: ranges,
        vbl_fraction: cycles::vbl_fraction_f(cost_avg),
        hbl_count: cycles::hbl_count_f(cost_avg),
        fits_50hz: fits_50hz(cost_max),
        headroom_pct: headroom_pct(cost_max),
        worst_frame,
        worst_frame_path: frame_path(dir, worst_frame).display().to_string(),
        profile_dir: dir.display().to_string(),
        state: "paused".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(frames: u32, from: Option<&str>, to: Option<&str>) -> ProfileFrameArgs {
        ProfileFrameArgs {
            frames,
            from: from.map(str::to_string),
            to: to.map(str::to_string),
            timeout_ms: None,
        }
    }

    #[test]
    fn validate_rejects() {
        assert!(validate(&args(0, None, None)).is_err());
        assert!(validate(&args(201, None, None)).is_err());
        let a = validate(&args(1, Some("$1000"), None)).unwrap_err();
        assert!(a.message.contains("together"));
        let b = validate(&args(1, None, Some("$1000"))).unwrap_err();
        assert!(b.message.contains("together"));
        let c = validate(&args(1, Some("$1000"), Some("$1000"))).unwrap_err();
        assert!(c.message.contains("greater than"));
        let d = validate(&args(1, Some("$2000"), Some("$1000"))).unwrap_err();
        assert!(d.message.contains("greater than"));
        let e = validate(&args(1, Some("$0"), Some("$400002"))).unwrap_err();
        assert!(e.message.contains("ST RAM"));
    }

    #[test]
    fn validate_accepts() {
        assert_eq!(validate(&args(1, None, None)).unwrap(), None);
        assert_eq!(
            validate(&args(1, Some("$1000"), Some("$2000"))).unwrap(),
            Some((Addr::new(0x1000), Addr::new(0x2000)))
        );
        assert_eq!(
            validate(&args(1, Some("$0"), Some("$400000"))).unwrap(),
            Some((Addr::new(0), Addr::new(0x400000)))
        );
    }

    #[test]
    fn stats_min_avg_max_computes() {
        assert_eq!(stats_min_avg_max(&[3, 1, 2]), (1, 2.0, 3));
        assert_eq!(stats_min_avg_max(&[5]), (5, 5.0, 5));
    }

    #[test]
    fn worst_index_first_on_tie() {
        assert_eq!(worst_index(&[5, 9, 9]), 1);
        assert_eq!(worst_index(&[7]), 0);
    }

    #[test]
    fn headroom_and_fit_boundaries() {
        let vbl = u64::from(cycles::VBL_CYCLES);
        assert!(fits_50hz(vbl));
        assert_eq!(headroom_pct(vbl), 0.0);
        assert!(!fits_50hz(vbl + 1));
        assert!(headroom_pct(vbl + 1) < 0.0);
        assert_eq!(headroom_pct(vbl / 2), 50.0);
    }

    #[test]
    fn build_output_with_extent() {
        let totals = vec![160_000, 160_300, 160_100];
        let ranges = vec![40_000, 60_000, 50_000];
        let out = build_output(
            3,
            Some((Addr::new(0x1000), Addr::new(0x2000))),
            totals.clone(),
            Some(ranges.clone()),
            Path::new("/tmp/profile-frame-0001"),
        );
        assert_eq!(out.cost_basis, "range");
        assert_eq!(out.range_cycles_max, Some(60_000));
        assert_eq!(out.worst_frame, 2);
        assert!(out.worst_frame_path.ends_with("frame-0002.txt"));
        assert_eq!(out.vbl_fraction, 50_000.0 / 160_256.0);
        assert!(out.fits_50hz);
        assert_eq!(out.per_frame_total_cycles, totals);
        assert_eq!(out.per_frame_range_cycles, Some(ranges));
        assert_eq!(out.state, "paused");
    }

    #[test]
    fn build_output_without_extent() {
        let totals = vec![160_000, 160_300, 160_100];
        let out = build_output(3, None, totals, None, Path::new("/tmp/profile-frame-0002"));
        assert_eq!(out.cost_basis, "total");
        assert!(out.range_cycles_min.is_none());
        assert!(out.per_frame_range_cycles.is_none());
        assert_eq!(out.worst_frame, 2);
        assert!(!out.fits_50hz);
        assert!(out.headroom_pct < 0.0);
    }
}
