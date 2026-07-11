//! Wire schema + pure helpers for the `watch` tool (P14 §7.2).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::Addr;

/// `watch`/`run_frames`'s own output budget — unlike `breakpoint(set)`'s
/// near-instant conditions, these can wait real wall-clock time, during
/// which Hatari's log accumulates control-plane polling noise BEFORE the
/// informative match-report block (which always arrives at the END of the
/// captured text) — `cap_output_tail` below caps from the tail, not the
/// head, for exactly this reason (deviation from `breakpoint_wire::cap_output`
/// confirmed live during P14 §9.4; see decisions.md's follow-up ADR).
const OUTPUT_CAP: usize = 2 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WatchArgs {
    /// Address to watch: "$2D645", "0x2d645", or decimal.
    pub addr: String,
    /// What to watch for. Only "write" exists: Hatari's condition engine
    /// observes VALUES, not bus cycles — this fires when the value CHANGES
    /// (a write of the identical value is invisible; reads are never
    /// observable — use xrefs for "who reads this").
    pub on: Option<WatchOn>,
    /// Watched width in bytes: 1, 2 (default) or 4. addr must be even for
    /// widths 2/4.
    pub len: Option<u32>,
    /// Wait budget for the change, ms (default 10000).
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WatchOn {
    Write,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WatchOutput {
    /// Did the watch fire within the budget?
    pub fired: bool,
    /// true = the fired breakpoint was THIS watch; false = a foreign
    /// breakpoint fired first (its report is in `output`; the watch was
    /// disarmed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ours: Option<bool>,
    /// watched address, "$02D645" form
    pub addr: String,
    pub len: u32,
    /// Value at arm time (frozen read), "$xxxx" zero-padded to 2*len hex
    /// digits.
    pub old_value: String,
    /// Value at the freeze after the fire (exact: zero instructions run
    /// between the change being detected and the freeze). Fired only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<String>,
    /// PC at the freeze = the instruction AFTER the writer (fired only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pc: Option<String>,
    /// Best-effort decode of the writing instruction (the one ending at
    /// pc). exact=true ⇒ its operand resolves to the watched address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writer: Option<WriterOut>,
    /// Debugger-entry block (fired only), capped like breakpoint's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// "paused" (fired) | "running" (timeout).
    pub state: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WriterOut {
    pub addr: String,
    pub mnemonic: String,
    pub ops: String,
    pub bytes_hex: String,
    pub exact: bool,
}

/// Widths: 1→b, 2→w, 4→l. Addresses >= $FF0000 render sign-extended to 32
/// bits ("$ffff8240"), lower as bare hex ("$2d645") — §3.2.
pub(crate) fn watch_condition(addr: Addr, len: u32) -> String {
    let width = match len {
        1 => 'b',
        4 => 'l',
        _ => 'w',
    };
    let a = render_addr(addr);
    format!("({a}).{width} ! ({a}).{width}")
}

/// Sign-extend a 24-bit ST bus address to 32 bits when it lies in the IO
/// range ($FF0000+); lower addresses render as bare (unpadded) hex.
fn render_addr(addr: Addr) -> String {
    if addr.raw() >= 0xFF_0000 {
        let signed = (addr.raw() as i32) << 8 >> 8;
        format!("${:08x}", signed as u32)
    } else {
        format!("${:x}", addr.raw())
    }
}

/// "$xxxx" lowercase, zero-padded to 2*len hex digits.
pub(crate) fn format_value(v: u32, len: u32) -> String {
    format!("${:0width$x}", v, width = (len * 2) as usize)
}

/// BE fold of 1/2/4 bytes to u32 (input length already validated).
pub(crate) fn be_value(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |acc, &b| (acc << 8) | u32::from(b))
}

/// Cap `output` to its LAST `OUTPUT_CAP` bytes (cut at the first newline
/// at-or-after the window start) — the tail-keeping counterpart to
/// `breakpoint_wire::cap_output`'s head-keeping (that one is correct for
/// `breakpoint(set)`'s near-instant, report-at-head conditions; this one is
/// correct for `watch`/`run_frames`'s wait-then-report-at-tail shape).
/// Char-boundary safe (Hatari output is ASCII in practice, but this must
/// never panic on a multi-byte split); falls back to the boundary-adjusted
/// tail (never empty) when no newline exists inside the window.
pub(super) fn cap_output_tail(output: String) -> String {
    if output.len() <= OUTPUT_CAP {
        return output;
    }
    let mut start = output.len() - OUTPUT_CAP;
    while !output.is_char_boundary(start) {
        start += 1;
    }
    let cut = output[start..]
        .find('\n')
        .map(|i| start + i + 1)
        .unwrap_or(start);
    output[cut..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_condition_word() {
        assert_eq!(
            watch_condition(Addr::new(0x2D645), 2),
            "($2d645).w ! ($2d645).w"
        );
    }

    #[test]
    fn watch_condition_byte_long() {
        assert_eq!(
            watch_condition(Addr::new(0x2D645), 1),
            "($2d645).b ! ($2d645).b"
        );
        assert_eq!(
            watch_condition(Addr::new(0x2D645), 4),
            "($2d645).l ! ($2d645).l"
        );
    }

    #[test]
    fn watch_condition_io_sign_extends() {
        assert_eq!(
            watch_condition(Addr::new(0xFF8240), 2),
            "($ffff8240).w ! ($ffff8240).w"
        );
    }

    #[test]
    fn format_value_pads() {
        assert_eq!(format_value(0x4C6, 2), "$04c6");
        assert_eq!(format_value(5, 1), "$05");
        assert_eq!(format_value(0xDEAD_BEEF, 4), "$deadbeef");
    }

    #[test]
    fn be_value_folds() {
        assert_eq!(be_value(&[0xDE, 0xAD]), 0xDEAD);
        assert_eq!(be_value(&[0xDE, 0xAD, 0xBE, 0xEF]), 0xDEAD_BEEF);
    }

    #[test]
    fn cap_output_tail_short_passthrough() {
        assert_eq!(cap_output_tail("hi".to_string()), "hi");
    }

    #[test]
    fn cap_output_tail_keeps_tail_drops_head() {
        let long = "noise-head ".repeat(OUTPUT_CAP) + "\nmatch-report line";
        let capped = cap_output_tail(long);
        assert!(capped.len() <= OUTPUT_CAP + "match-report line".len());
        assert!(capped.contains("match-report"));
        assert!(!capped.contains("noise-head"));
    }

    #[test]
    fn cap_output_tail_no_newline_falls_back_to_raw_tail() {
        let long = "x".repeat(OUTPUT_CAP * 2);
        let capped = cap_output_tail(long);
        assert!(!capped.is_empty());
        assert!(capped.len() <= OUTPUT_CAP);
    }
}
