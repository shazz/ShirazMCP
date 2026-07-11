//! Const cycle-cost data transcribed from the m68000-cycles skill reference
//! (`.claude/skills/m68000-cycles/reference/68000_execution_cycles.md`).
//! Pure data + tiny lookup helpers; no opcode-word parsing here (that's
//! `decode.rs`/`decode_misc.rs`).

use super::EaMode;

/// MOVE total cycles (reference §"MOVE Instructions" — the full matrix,
/// NOT base+EA: the table does not decompose cleanly, e.g. dn→-(an).l is
/// 14). Rows = source EaMode (all 12), cols = dest EaMode (first 9 — a
/// MOVE destination is never PC-relative or immediate).
pub(super) const MOVE_BW: [[u8; 9]; 12] = [
    [4, 4, 8, 8, 8, 12, 14, 12, 16],      // src dn
    [4, 4, 8, 8, 8, 12, 14, 12, 16],      // src an
    [8, 8, 12, 12, 12, 16, 18, 16, 20],   // src (an)
    [8, 8, 12, 12, 12, 16, 18, 16, 20],   // src (an)+
    [10, 10, 14, 14, 14, 18, 20, 18, 22], // src -(an)
    [12, 12, 16, 16, 16, 20, 22, 20, 24], // src d(an)
    [14, 14, 18, 18, 18, 22, 24, 22, 26], // src d(an,ri)
    [12, 12, 16, 16, 16, 20, 22, 20, 24], // src abs.w
    [16, 16, 20, 20, 20, 24, 26, 24, 28], // src abs.l
    [12, 12, 16, 16, 16, 20, 22, 20, 24], // src d(pc)
    [14, 14, 18, 18, 18, 22, 24, 22, 26], // src d(pc,ri)
    [8, 8, 12, 12, 12, 16, 18, 16, 20],   // src #imm
];

pub(super) const MOVE_L: [[u8; 9]; 12] = [
    [4, 4, 12, 12, 14, 16, 18, 16, 20],
    [4, 4, 12, 12, 14, 16, 18, 16, 20],
    [12, 12, 20, 20, 20, 24, 26, 24, 28],
    [12, 12, 20, 20, 20, 24, 26, 24, 28],
    [14, 14, 22, 22, 22, 26, 28, 26, 30],
    [16, 16, 24, 24, 24, 28, 30, 28, 32],
    [18, 18, 26, 26, 26, 30, 32, 30, 34],
    [16, 16, 24, 24, 24, 28, 30, 28, 32],
    [20, 20, 28, 28, 28, 32, 34, 32, 36],
    [16, 16, 24, 24, 24, 28, 30, 28, 32],
    [18, 18, 26, 26, 26, 30, 32, 30, 34],
    [12, 12, 20, 20, 20, 24, 26, 24, 28],
];

/// MOVE cost: `src`/`dst` in the full 12-mode `EaMode` set (dst restricted
/// to the first 9 by the caller — MOVE has no PC-relative/immediate dest).
pub(super) fn move_cost(src: EaMode, dst_col: usize, long: bool) -> u32 {
    let row = src as usize;
    u32::from(if long {
        MOVE_L[row][dst_col]
    } else {
        MOVE_BW[row][dst_col]
    })
}

/// jmp/jsr/lea/pea/movem by control EA mode (reference §"Other" /
/// §"Movement of Multiple Registers"). `None` for modes the instruction
/// cannot address (register-direct/postinc/predecrement/immediate).
fn control_cost(mode: EaMode, table: &[(EaMode, u32)]) -> Option<u32> {
    table.iter().find(|(m, _)| *m == mode).map(|(_, c)| *c)
}

pub(super) fn jmp_cost(mode: EaMode) -> Option<u32> {
    use EaMode::*;
    control_cost(
        mode,
        &[
            (AnInd, 8),
            (AnDisp, 10),
            (AnIdx, 14),
            (AbsW, 10),
            (AbsL, 12),
            (PcDisp, 10),
            (PcIdx, 14),
        ],
    )
}

pub(super) fn jsr_cost(mode: EaMode) -> Option<u32> {
    use EaMode::*;
    control_cost(
        mode,
        &[
            (AnInd, 16),
            (AnDisp, 18),
            (AnIdx, 22),
            (AbsW, 18),
            (AbsL, 20),
            (PcDisp, 18),
            (PcIdx, 22),
        ],
    )
}

pub(super) fn lea_cost(mode: EaMode) -> Option<u32> {
    use EaMode::*;
    control_cost(
        mode,
        &[
            (AnInd, 4),
            (AnDisp, 8),
            (AnIdx, 12),
            (AbsW, 8),
            (AbsL, 12),
            (PcDisp, 8),
            (PcIdx, 12),
        ],
    )
}

pub(super) fn pea_cost(mode: EaMode) -> Option<u32> {
    use EaMode::*;
    control_cost(
        mode,
        &[
            (AnInd, 12),
            (AnDisp, 16),
            (AnIdx, 20),
            (AbsW, 16),
            (AbsL, 20),
            (PcDisp, 16),
            (PcIdx, 20),
        ],
    )
}

/// movem memory→register base cost (register list cost is added by the
/// caller: 4/reg word, 8/reg long).
pub(super) fn movem_load_base(mode: EaMode) -> Option<u32> {
    use EaMode::*;
    control_cost(
        mode,
        &[
            (AnInd, 12),
            (AnPost, 12),
            (AnDisp, 16),
            (AnIdx, 18),
            (AbsW, 16),
            (AbsL, 20),
            (PcDisp, 16),
            (PcIdx, 18),
        ],
    )
}

/// movem register→memory base cost (register list cost added by caller).
pub(super) fn movem_store_base(mode: EaMode) -> Option<u32> {
    use EaMode::*;
    control_cost(
        mode,
        &[
            (AnInd, 8),
            (AnPre, 8),
            (AnDisp, 12),
            (AnIdx, 14),
            (AbsW, 12),
            (AbsL, 16),
        ],
    )
}

// Fixed-cost instructions (reference §"Other Instructions" /
// §"Immediate Instructions"). Multiply/divide are data-dependent maxima —
// the WORST case is pinned per §7.1.
pub(super) const RTS: u32 = 16;
pub(super) const RTE: u32 = 20;
pub(super) const RTR: u32 = 20;
pub(super) const NOP: u32 = 4;
pub(super) const SWAP: u32 = 4;
pub(super) const EXG: u32 = 6;
pub(super) const EXT: u32 = 4;
pub(super) const LINK: u32 = 18;
pub(super) const UNLK: u32 = 12;
pub(super) const TRAPV: u32 = 4;
pub(super) const TRAP: u32 = 34;
pub(super) const RESET: u32 = 132;
pub(super) const STOP: u32 = 4;
pub(super) const MOVE_USP: u32 = 4;
pub(super) const MULX: u32 = 70;
pub(super) const DIVU: u32 = 140;
pub(super) const DIVS: u32 = 158;
