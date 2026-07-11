//! Wire schema + the pure `build` seam for `save_prg` mode="snapshot"
//! (P22 §8.4) — internal to the tool layer, pure, unit-tested without a
//! session. `not_restored` is the fixed §4.2 list, verbatim; `warnings`
//! carries the conditional §4.2 warnings computed in `snapshot_prg.rs`.

use crate::ripper::snapshot::SnapshotFacts;
use crate::types::Addr;

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct SnapshotOutput {
    pub out_path: String,
    pub size: u64,
    /// Always "snapshot".
    pub mode: String,
    /// "$…" — frozen PC the stub resumes at.
    pub entry_pc: String,
    /// "$…" — frozen SR word.
    pub sr: String,
    /// "$…" — frozen RAM extent `R`.
    pub region_end: String,
    /// Packed SNAP1 payload bytes (the compressed RAM image).
    pub payload_len: usize,
    /// `payload_len * 100 / region_end`.
    pub packed_pct: u8,
    /// "ste" | "st" (from the session's machine config).
    pub machine: String,
    /// "$1C0..$400 — restore-stub residue" (the actual hole range).
    pub hole: String,
    /// "$xxxx..$yyyy — RTE frame scratch below frozen SSP".
    pub frame_scratch: String,
    /// Fixed list (§4.2) — what a resume honestly cannot bring back.
    pub not_restored: Vec<String>,
    /// Conditional warnings (§4.2) computed from the captured registers.
    pub warnings: Vec<String>,
}

/// The fixed §4.2 "not restored" list, verbatim (plus one live-verified
/// addition, item 11 — see `ripper::snapshot::pack::LOW_MEM_RESERVED`) —
/// every snapshot output carries all eleven, regardless of content (an
/// honest, unconditional disclosure, not a best-effort detection).
const NOT_RESTORED: [&str; 11] = [
    "video beam position — the restored frame starts at the top (base/res/palette are exact)",
    "MFP timer countdown phase — timers restart at their reload values (period-top)",
    "MFP pending/in-service interrupts (IPRA/B, ISRA/B are clear-only registers)",
    "IKBD controller mode (its own MCU; a frozen program's IKBD commands are not replayed)",
    "in-flight FDC/DMA disk operations",
    "blitter registers and any half-done blit",
    "STE DMA-sound frame position — playback restarts at frame start",
    "STE microwire/LMC1992 volume state",
    "hole $1C0–$3FF contains restore-stub residue instead of frozen bytes",
    "6 bytes below the frozen SSP (RTE frame scratch)",
    "$0-$7 (the 68000 reset SSP/PC) — real ST hardware refuses writes there \
     even from supervisor mode, live-verified; always left as whatever TOS booted with",
];

/// Pure builder: facts + machine string + precomputed conditional warnings
/// (§8.3) -> the wire struct. Owns the fixed `not_restored` list so no
/// caller can accidentally omit or truncate it.
pub(crate) fn build(
    out_path: &str,
    facts: &SnapshotFacts,
    machine: &str,
    warnings: Vec<String>,
) -> SnapshotOutput {
    SnapshotOutput {
        out_path: out_path.to_string(),
        size: facts.file_len as u64,
        mode: "snapshot".to_string(),
        entry_pc: Addr::new(facts.entry_pc).to_string(),
        sr: format!("${:04X}", facts.sr),
        region_end: Addr::new(facts.region_end).to_string(),
        payload_len: facts.payload_len,
        packed_pct: facts.packed_pct,
        machine: machine.to_string(),
        hole: format!(
            "{}..{} — restore-stub residue",
            Addr::new(facts.hole.0),
            Addr::new(facts.hole.1)
        ),
        frame_scratch: format!(
            "{}..{} — RTE frame scratch below frozen SSP",
            Addr::new(facts.frame_scratch.0),
            Addr::new(facts.frame_scratch.1)
        ),
        not_restored: NOT_RESTORED.iter().map(|s| s.to_string()).collect(),
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> SnapshotFacts {
        SnapshotFacts {
            file_len: 12345,
            region_end: 0x0010_0000,
            payload_len: 4096,
            packed_pct: 4,
            entry_pc: 0x1200,
            sr: 0x2700,
            hole: (0x1C0, 0x3EE),
            frame_scratch: (0x1FFA, 0x2000),
        }
    }

    #[test]
    fn build_carries_fixed_not_restored_list() {
        let out = build("/tmp/SNAP.PRG", &facts(), "ste", vec![]);
        assert_eq!(out.not_restored.len(), 11);
        assert_eq!(out.mode, "snapshot");
        assert!(out.not_restored[0].contains("video beam position"));
        assert!(out.not_restored[9].contains("RTE frame scratch"));
    }

    #[test]
    fn build_formats_fields() {
        let out = build("/tmp/SNAP.PRG", &facts(), "st", vec!["w1".to_string()]);
        assert_eq!(out.out_path, "/tmp/SNAP.PRG");
        assert_eq!(out.size, 12345);
        assert_eq!(out.entry_pc, "$001200");
        assert_eq!(out.sr, "$2700");
        assert_eq!(out.region_end, "$100000");
        assert_eq!(out.payload_len, 4096);
        assert_eq!(out.packed_pct, 4);
        assert_eq!(out.machine, "st");
        assert!(out.hole.starts_with("$0001C0..$0003EE"));
        assert!(out.frame_scratch.contains("RTE frame scratch"));
        assert_eq!(out.warnings, vec!["w1".to_string()]);
    }
}
