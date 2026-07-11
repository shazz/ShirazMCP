//! STE Shifter/CPU bus-contention model (P18). Pinned from STEIsNotDead's
//! tools/cycles/ste_model.py via the m68000-cycles skill: during visible
//! scanlines the Shifter steals bus slots (~12 cyc/word vs ~8 in VBlank),
//! so MEMORY-ACCESS cycles scale ×3/2 while internal cycles never do —
//! the refinement over ste_model.py's flat ×1.45-on-total (which is this
//! same 1.5 diluted by a typical ~10% internal-cycle share). The result is
//! a *better estimate*, not truth: Hatari's profiler (debug/profile.rs)
//! stays the measured ground truth.

use super::CycleEstimate;

/// Where the code runs relative to the display (ste_model.py
/// ExecutionContext, minus UNKNOWN — the wire default plays that role).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// Active display: Shifter contention (worst case, the default).
    Visible,
    /// Vertical blank: CPU owns the bus.
    Vblank,
    /// Runs across the whole frame: scanline-weighted average.
    Mixed,
}

/// PAL 50 Hz frame split (ste_model.py docstring). Their sum is the
/// 313-line frame already pinned by VBL_CYCLES / SCANLINE_CYCLES.
pub const VISIBLE_LINES: u32 = 200;
pub const VBLANK_LINES: u32 = 113;

/// Bus-cycle scale factor as (numerator, denominator).
/// Visible = 12/8 cyc/word (ste_model.py "12/8 = 1.5x").
/// Mixed   = (VISIBLE_LINES·3 + VBLANK_LINES·2) / (313·2) = 826/626 = 413/313.
const fn factor(region: Region) -> (u64, u64) {
    match region {
        Region::Visible => (3, 2),
        Region::Vblank => (1, 1),
        Region::Mixed => (413, 313),
    }
}

/// Apply the contention model to a range estimate: internal cycles pass
/// through, bus cycles scale by the region factor (truncating integer
/// division, like ste_model.py's int(); bus is a multiple of 4, so the
/// visible 3/2 case is exact). Invariant used: est.bus_cycles <= est.cycles
/// (guaranteed by estimate()'s clamp).
pub fn contended_cycles(est: &CycleEstimate, region: Region) -> u64 {
    let (n, d) = factor(region);
    est.cycles - est.bus_cycles + est.bus_cycles * n / d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::cycles::{estimate, SCANLINE_CYCLES, VBL_CYCLES};
    use crate::analyze::disasm::Insn;
    use crate::types::Addr;

    fn insn(bytes: Vec<u8>) -> Insn {
        Insn {
            addr: Addr::new(0),
            mnemonic: String::new(),
            ops: String::new(),
            bytes,
        }
    }

    #[test]
    fn mem_heavy_visible_is_12_over_8() {
        let stream: Vec<_> = (0..10).map(|_| insn(vec![0x32, 0xD8])).collect();
        let e = estimate(&stream);
        assert_eq!(e.cycles, 120);
        assert_eq!(e.bus_cycles, 120);
        assert_eq!(contended_cycles(&e, Region::Visible), 180);
        assert_eq!(contended_cycles(&e, Region::Vblank), 120);
        assert_eq!(contended_cycles(&e, Region::Mixed), 158);
    }

    #[test]
    fn register_heavy_barely_contends() {
        let stream: Vec<_> = (0..4).map(|_| insn(vec![0xC1, 0xC1])).collect();
        let e = estimate(&stream);
        assert_eq!(e.cycles, 288);
        assert_eq!(e.bus_cycles, 16);
        assert_eq!(contended_cycles(&e, Region::Visible), 296);
    }

    #[test]
    fn mixed_stream() {
        let mut stream: Vec<_> = (0..9).map(|_| insn(vec![0x32, 0xD8])).collect();
        stream.push(insn(vec![0xC1, 0xC1]));
        let e = estimate(&stream);
        assert_eq!(e.cycles, 180);
        assert_eq!(e.bus_cycles, 112);
        assert_eq!(contended_cycles(&e, Region::Visible), 236);
        assert_eq!(contended_cycles(&e, Region::Mixed), 215);
        assert_eq!(contended_cycles(&e, Region::Vblank), 180);
    }

    #[test]
    fn vblank_is_identity() {
        let stream: Vec<_> = (0..10).map(|_| insn(vec![0x32, 0xD8])).collect();
        let e = estimate(&stream);
        assert_eq!(contended_cycles(&e, Region::Vblank), e.cycles);
    }

    #[test]
    fn mixed_factor_exact_on_multiple_of_313x4() {
        let e = CycleEstimate {
            cycles: 1252,
            bus_cycles: 1252,
            instructions: 1,
            unknown: 0,
        };
        assert_eq!(contended_cycles(&e, Region::Mixed), 1652);
    }

    #[test]
    fn frame_geometry_consistent() {
        assert_eq!(VISIBLE_LINES + VBLANK_LINES, VBL_CYCLES / SCANLINE_CYCLES);
    }
}
