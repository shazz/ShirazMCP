//! Wire schema + pure helpers for `count_hits` (ADR-47). Split from
//! `count_hits.rs` (choreography) — the `profile_frame_wire.rs` precedent.

use rmcp::ErrorData as McpError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::debug::breakpoint::BreakpointEntry;
use crate::debug::trace::TRACE_SUFFIX;
use crate::types::Addr;

pub const PCS_MAX: usize = 16;
/// Lower than `run_frames`' 5000: every hit is two log lines that the
/// managed run buffers in memory until the window closes.
pub const COUNT_HITS_MAX_FRAMES: u32 = 500;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CountHitsArgs {
    /// 1..=16 instruction addresses ($hex, 0xhex, decimal or a symbol
    /// name); each must be even and distinct.
    pub pcs: Vec<String>,
    /// VBL frames to count over (1..=500).
    pub frames: u32,
    /// Wait budget for the window, ms. Default
    /// `frames::default_timeout(frames)` — raise it for very hot PCs
    /// (each hit is logged, which slows emulation).
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
pub struct CountHitsOutput {
    pub frames: u32,
    pub vbl_before: u32,
    pub vbl_after: u32,
    /// vbl_after - vbl_before; always == frames on success.
    pub vbl_delta: u32,
    /// One entry per requested PC, in request order.
    pub hits: Vec<PcHits>,
    /// Always "paused" on success — frozen at the last frame boundary.
    pub state: String,
}

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
pub struct PcHits {
    pub pc: String,
    /// Exact executions of the instruction at `pc` in the window
    /// (Hatari's own per-breakpoint "matched N times" counter).
    pub hits: u64,
    pub hits_per_frame: f64,
    /// hits / frames × 50 — calls per second at PAL 50 Hz.
    pub rate_hz: f64,
}

pub(crate) fn validate(a: &CountHitsArgs) -> Result<(), McpError> {
    if a.pcs.is_empty() || a.pcs.len() > PCS_MAX {
        return Err(McpError::invalid_params(
            format!("pcs must hold 1..={PCS_MAX} addresses"),
            None,
        ));
    }
    if !(1..=COUNT_HITS_MAX_FRAMES).contains(&a.frames) {
        return Err(McpError::invalid_params(
            format!("frames must be 1..={COUNT_HITS_MAX_FRAMES}"),
            None,
        ));
    }
    Ok(())
}

/// Even + distinct, AFTER resolution (a symbol and its hex can collide).
pub(crate) fn check_pcs(pcs: &[Addr]) -> Result<(), McpError> {
    if let Some(odd) = pcs.iter().find(|a| a.raw() % 2 != 0) {
        return Err(McpError::invalid_params(
            format!("pc {odd} is odd — instructions start on even addresses"),
            None,
        ));
    }
    for (i, a) in pcs.iter().enumerate() {
        if pcs[..i].contains(a) {
            return Err(McpError::invalid_params(
                format!("pc {a} given twice"),
                None,
            ));
        }
    }
    Ok(())
}

/// The trace condition armed for one PC.
pub(crate) fn condition(pc: Addr) -> String {
    format!("pc = ${:x}", pc.raw())
}

/// Indices of OUR trace breakpoints in a fresh `b` listing, highest first
/// (removal order — removing a lower index would shift the higher ones).
/// For each armed expression, the HIGHEST matching index is ours: it was
/// appended after any identical pre-existing user breakpoint, which is left
/// intact.
pub(crate) fn indices_to_remove(entries: &[BreakpointEntry], armed: &[String]) -> Vec<u32> {
    let mut picked: Vec<u32> = Vec::new();
    for expr in armed {
        let wanted = format!("{expr}{TRACE_SUFFIX}");
        let ours = entries
            .iter()
            .filter(|e| e.expression == wanted && !picked.contains(&e.index))
            .map(|e| e.index)
            .max();
        picked.extend(ours);
    }
    picked.sort_unstable_by(|a, b| b.cmp(a));
    picked
}

/// Assemble the output (pure). `hits` is in request order.
pub(crate) fn build_output(
    frames: u32,
    vbl_before: u32,
    vbl_after: u32,
    pcs: &[Addr],
    hits: &[u64],
) -> CountHitsOutput {
    let per_pc = pcs
        .iter()
        .zip(hits)
        .map(|(pc, &h)| {
            let per_frame = h as f64 / f64::from(frames);
            PcHits {
                pc: pc.to_string(),
                hits: h,
                hits_per_frame: per_frame,
                rate_hz: per_frame * 50.0,
            }
        })
        .collect();
    CountHitsOutput {
        frames,
        vbl_before,
        vbl_after,
        vbl_delta: vbl_after.wrapping_sub(vbl_before),
        hits: per_pc,
        state: "paused".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(index: u32, expression: &str) -> BreakpointEntry {
        BreakpointEntry {
            index,
            expression: expression.to_string(),
        }
    }

    #[test]
    fn validate_bounds() {
        let args = |n: usize, frames| CountHitsArgs {
            pcs: vec!["$1000".to_string(); n],
            frames,
            timeout_ms: None,
        };
        assert!(validate(&args(1, 1)).is_ok());
        assert!(validate(&args(PCS_MAX, COUNT_HITS_MAX_FRAMES)).is_ok());
        assert!(validate(&args(0, 1)).is_err());
        assert!(validate(&args(PCS_MAX + 1, 1)).is_err());
        assert!(validate(&args(1, 0)).is_err());
        assert!(validate(&args(1, COUNT_HITS_MAX_FRAMES + 1)).is_err());
    }

    #[test]
    fn check_pcs_rejects_odd_and_duplicates() {
        assert!(check_pcs(&[Addr::new(0x1000), Addr::new(0x2000)]).is_ok());
        let odd = check_pcs(&[Addr::new(0x1001)]).unwrap_err();
        assert!(odd.message.contains("odd"));
        let dup = check_pcs(&[Addr::new(0x1000), Addr::new(0x1000)]).unwrap_err();
        assert!(dup.message.contains("twice"));
    }

    #[test]
    fn condition_is_hex() {
        assert_eq!(condition(Addr::new(0xE0_0800)), "pc = $e00800");
    }

    /// Pre-existing user breakpoints (incl. an identical trace one) stay.
    #[test]
    fn indices_to_remove_keeps_user_breakpoints() {
        let listing = [
            entry(1, "pc = $1234 :trace"), // user's own, identical
            entry(2, "VBL > 5 :once"),     // user's
            entry(3, "pc = $1234 :trace"), // ours
            entry(4, "pc = $5678 :trace"), // ours
        ];
        let armed = ["pc = $1234".to_string(), "pc = $5678".to_string()];
        assert_eq!(indices_to_remove(&listing, &armed), vec![4, 3]);
    }

    #[test]
    fn indices_to_remove_tolerates_missing() {
        let listing = [entry(1, "pc = $1234 :trace")];
        let armed = ["pc = $1234".to_string(), "pc = $9999".to_string()];
        assert_eq!(indices_to_remove(&listing, &armed), vec![1]);
        assert!(indices_to_remove(&[], &armed).is_empty());
    }

    #[test]
    fn build_output_rates() {
        let out = build_output(
            10,
            100,
            110,
            &[Addr::new(0x1000), Addr::new(0x2000)],
            &[25, 0],
        );
        assert_eq!(out.vbl_delta, 10);
        assert_eq!(out.hits[0].hits, 25);
        assert_eq!(out.hits[0].hits_per_frame, 2.5);
        assert_eq!(out.hits[0].rate_hz, 125.0);
        assert_eq!(out.hits[1].rate_hz, 0.0);
        assert_eq!(out.hits[0].pc, "$001000");
    }
}
