//! Wire schema + output assembly for `profile`. Moved out of
//! analyze_wire.rs when P18 added the region-aware contended estimate
//! (file-size ceiling; insn_out.rs precedent).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::analyze::cycles::{self, Region};
use crate::types::Addr;

pub const PROFILE_LEG_TIMEOUT_MS: u64 = 20_000;
pub const PROFILE_MAX_RANGE: u32 = 65_536;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProfileArgs {
    /// Run-from PC (breakpoint `pc = from` is armed and awaited).
    pub from: String,
    /// Run-to PC. Must differ from `from`; `to - from` ≤ 65536 bounds the
    /// static-disasm window (measurement itself is unbounded).
    pub to: String,
    /// Per-leg wait budget (reach `from`, then reach `to`), ms. Default 20000.
    pub timeout_ms: Option<u64>,
    /// Display region the code runs in, for the contended static estimate:
    /// "visible" (default — active display, Shifter steals bus slots; the
    /// worst case), "vblank" (full bus, no contention), "mixed" (code that
    /// runs across the whole frame). Does not affect measured_cycles.
    pub region: Option<RegionArg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RegionArg {
    Visible,
    Vblank,
    Mixed,
}

impl RegionArg {
    pub(crate) fn to_region(self) -> Region {
        match self {
            RegionArg::Visible => Region::Visible,
            RegionArg::Vblank => Region::Vblank,
            RegionArg::Mixed => Region::Mixed,
        }
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            RegionArg::Visible => "visible",
            RegionArg::Vblank => "vblank",
            RegionArg::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfileOutput {
    pub from: String,
    pub to: String,
    /// Hatari-measured cycles summed over profiled addresses in
    /// [from, to) — interrupt/TOS time outside the range excluded. None
    /// only if no profiled address fell in range. GROUND TRUTH: includes
    /// real emulated bus contention.
    pub measured_cycles: Option<u64>,
    /// Executed-instruction count over the same range.
    pub instructions: Option<u64>,
    /// Static straight-line estimate for ONE pass from..to — contention-
    /// BLIND lower bound (fall-through branches; see tool description).
    pub static_cycles: u64,
    /// static_cycles with the STE Shifter bus-contention model applied to
    /// its memory-access share for `region` (ADR-P18): ×3/2 visible, ×1
    /// vblank, ×413/313 mixed. Together with static_cycles this brackets
    /// the expected cost; measured_cycles remains the truth.
    pub static_cycles_contended: u64,
    /// Memory-access (bus) share of static_cycles — what contention
    /// scales. bus/static ≈ how bus-bound the routine is.
    pub static_bus_cycles: u64,
    /// Region assumed by static_cycles_contended ("visible"|"vblank"|"mixed").
    pub region: String,
    /// Instructions the static model could not cost (counted at 4 cycles).
    pub static_unknown: usize,
    /// measured_cycles / 160256 (falls back to static_cycles_contended
    /// when measurement is absent). VBLs the effect spans; <1 = sub-frame.
    pub vbl_fraction: f64,
    /// Same cycles / 512 — scanlines (HBLs) the effect spans (313 HBL = 1 VBL).
    /// The natural raster-budget unit when vbl_fraction < 1.
    pub hbl_count: f64,
    /// The raw `profile save` file (ADR-5: bulk data stays on disk).
    pub profile_path: String,
    /// Always "paused" on success — profiling ends frozen at `to`.
    pub state: String,
}

/// Assemble the wire output (moved from analyze_profile.rs; pure —
/// the P18 unit-test seam for the fallback + contention wiring).
pub(crate) fn build_output(
    from: Addr,
    to: Addr,
    instructions: u64,
    measured_cycles: u64,
    estimate: cycles::CycleEstimate,
    region: RegionArg,
    path: std::path::PathBuf,
) -> ProfileOutput {
    let contended = cycles::contended_cycles(&estimate, region.to_region());
    let has_data = instructions > 0;
    let eff = if has_data { measured_cycles } else { contended };
    let vbl_fraction = cycles::vbl_fraction(eff);
    let hbl_count = cycles::hbl_count(eff);
    ProfileOutput {
        from: from.to_string(),
        to: to.to_string(),
        measured_cycles: has_data.then_some(measured_cycles),
        instructions: has_data.then_some(instructions),
        static_cycles: estimate.cycles,
        static_cycles_contended: contended,
        static_bus_cycles: estimate.bus_cycles,
        region: region.name().to_string(),
        static_unknown: estimate.unknown,
        vbl_fraction,
        hbl_count,
        profile_path: path.display().to_string(),
        state: "paused".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::cycles::CycleEstimate;

    #[test]
    fn region_arg_maps_and_names() {
        let cases = [
            (RegionArg::Visible, Region::Visible, "visible"),
            (RegionArg::Vblank, Region::Vblank, "vblank"),
            (RegionArg::Mixed, Region::Mixed, "mixed"),
        ];
        for (arg, region, name) in cases {
            assert_eq!(arg.to_region(), region);
            assert_eq!(arg.name(), name);
        }
    }

    #[test]
    fn build_output_contended_and_fallback() {
        let estimate = CycleEstimate {
            cycles: 120,
            bus_cycles: 120,
            instructions: 10,
            unknown: 0,
        };
        let out = build_output(
            Addr::new(0x1000),
            Addr::new(0x1010),
            0,
            999,
            estimate,
            RegionArg::Visible,
            std::path::PathBuf::from("/tmp/p.txt"),
        );
        assert_eq!(out.static_cycles_contended, 180);
        assert_eq!(out.vbl_fraction, 180.0 / 160_256.0);
        assert_eq!(out.hbl_count, 180.0 / 512.0);
        assert_eq!(out.measured_cycles, None);

        let out2 = build_output(
            Addr::new(0x1000),
            Addr::new(0x1010),
            5,
            999,
            estimate,
            RegionArg::Visible,
            std::path::PathBuf::from("/tmp/p.txt"),
        );
        assert_eq!(out2.vbl_fraction, 999.0 / 160_256.0);
        assert_eq!(out2.hbl_count, 999.0 / 512.0);
        assert_eq!(out2.measured_cycles, Some(999));
    }
}
