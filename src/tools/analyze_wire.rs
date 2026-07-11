//! Wire schema + pure helpers for `disasm` (§8.2). Split out of
//! `analyze.rs` purely to keep that file under the line ceiling — same
//! "wire.rs" split as rip.rs/rip_wire.rs and mem/scan.rs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::insn_out::InsnOut;
use crate::analyze::{self, PsgSelect};
use crate::tools::mem::types::{AnnotationOut, ANNOTATIONS_CAP};
use crate::types::Addr;

pub const DISASM_DEFAULT: u32 = 32;
pub const DISASM_MAX: u32 = 128;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DisasmArgs {
    /// Start address: "$D106", "0xd106", or decimal. Must be even.
    pub addr: String,
    /// Instructions to decode (default 32, max 128). Page with next_addr.
    pub count: Option<u32>,
    /// "snapshot" (default): savebin the range, decode with embedded
    /// Capstone — works Running or Paused, one debugger round-trip.
    /// "live": Hatari's own `d` — knows symbols/EA hints, costs one
    /// round-trip too but returns Hatari's text.
    pub source: Option<DisasmSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DisasmSource {
    Snapshot,
    Live,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DisasmOutput {
    pub source: String,
    pub insns: Vec<InsnOut>,
    /// First address after the last decoded instruction — pass as `addr`
    /// to continue (pagination cursor, same convention as read_memory).
    pub next_addr: String,
}

/// One instruction parsed from a live `d` (§3.7) line.
pub(crate) struct LiveInsn {
    pub addr: Addr,
    pub word_count: usize,
    /// The opcode-word tokens concatenated verbatim (already lowercase hex,
    /// no spaces) — Hatari's own text doubles as `bytes_hex`.
    pub bytes_hex: String,
    pub mnemonic: String,
    pub ops: String,
}

/// Extract `$hex[.w]` operand tokens from an ops string, resolve each to a
/// 24-bit bus address, annotate. Capped at `ANNOTATIONS_CAP` per call.
pub(crate) fn annotate_ops(ops: &str, psg: Option<PsgSelect>) -> Vec<AnnotationOut> {
    ops.match_indices('$')
        .filter_map(|(i, _)| resolve_token_addr(&ops[i..]))
        .filter_map(|addr| analyze::annotate(Addr::new(addr), psg))
        .map(|a| AnnotationOut {
            addr: a.addr.to_string(),
            label: format!("{} — {}", a.register, a.detail),
        })
        .take(ANNOTATIONS_CAP)
        .collect()
}

/// Parse one `$hex[.w]` token at the START of `s` (which must begin with
/// '$'): sign-extends bit 15 for an `.w`-suffixed token before masking to
/// the 24-bit ST bus.
fn resolve_token_addr(s: &str) -> Option<u32> {
    let rest = s.strip_prefix('$')?;
    let hex_len = rest
        .bytes()
        .take_while(u8::is_ascii_hexdigit)
        .count()
        .min(8);
    if hex_len == 0 {
        return None;
    }
    let mut addr = u32::from_str_radix(&rest[..hex_len], 16).ok()?;
    if rest[hex_len..].starts_with(".w") && addr & 0x8000 != 0 {
        addr |= 0xFFFF_0000;
    }
    Some(addr & 0x00FF_FFFF)
}

/// PSG select tracking across a listing: if an instruction's mnemonic
/// starts with "move" and its ops are `#$N,<dest>` where `<dest>` resolves
/// to $FF8800, the state for SUBSEQUENT instructions becomes
/// `Some(PsgSelect(N & 0x0F))`. Otherwise carries the previous state.
pub(crate) fn track_psg(prev: Option<PsgSelect>, mnemonic: &str, ops: &str) -> Option<PsgSelect> {
    if !mnemonic.starts_with("move") {
        return prev;
    }
    let mut parts = ops.splitn(2, ',');
    let src = parts.next().unwrap_or("").trim();
    let dst = parts.next().unwrap_or("").trim();
    let Some(n) = src
        .strip_prefix("#$")
        .and_then(|h| u8::from_str_radix(h, 16).ok())
    else {
        return prev;
    };
    match resolve_token_addr(dst) {
        Some(0x00FF_8800) => Some(PsgSelect(n & 0x0F)),
        _ => prev,
    }
}

/// Parse one live `d` line (§3.7): addr = leading 8-hex token; opcode
/// words are the following whitespace tokens that are exactly 4 hex
/// digits; the first non-4-hex token starts the asm text (mnemonic + the
/// remainder as ops). Lines not starting with 8 hex digits are skipped.
pub(crate) fn parse_live_line(line: &str) -> Option<LiveInsn> {
    let mut tokens = line.split_whitespace();
    let addr_tok = tokens.next()?;
    if addr_tok.len() != 8 || !addr_tok.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let addr = u32::from_str_radix(addr_tok, 16).ok()?;
    let mut word_count = 0;
    let mut bytes_hex = String::new();
    let mut mnemonic = None;
    for tok in tokens.by_ref() {
        if tok.len() == 4 && tok.bytes().all(|b| b.is_ascii_hexdigit()) {
            word_count += 1;
            bytes_hex.push_str(tok);
        } else {
            mnemonic = Some(tok.to_string());
            break;
        }
    }
    Some(LiveInsn {
        addr: Addr::new(addr),
        word_count,
        bytes_hex,
        mnemonic: mnemonic?,
        ops: tokens.collect::<Vec<_>>().join(" "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotate_ops_finds_hardware_addr() {
        let out = annotate_ops("#$777, $ffff8240.l", None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].addr, "$FF8240");
        assert!(out[0].label.contains("palette register 0"));
    }

    #[test]
    fn annotate_ops_sign_extends_abs_w() {
        let out = annotate_ops("#$777, $8240.w", None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].addr, "$FF8240");
    }

    #[test]
    fn annotate_ops_no_hardware_addr_is_empty() {
        assert!(annotate_ops("d1, d0", None).is_empty());
    }

    #[test]
    fn track_psg_selects_and_persists() {
        let s = track_psg(None, "move.b", "#$7, $ffff8800.l");
        assert_eq!(s, Some(PsgSelect(7)));
        // A later, unrelated move must not clear the tracked select.
        let s2 = track_psg(s, "move.w", "d1, d0");
        assert_eq!(s2, s);
    }

    #[test]
    fn track_psg_ignores_non_move() {
        assert_eq!(track_psg(None, "add.w", "#$7, $ffff8800.l"), None);
    }

    #[test]
    fn parse_live_line_splits_addr_words_mnemonic_ops() {
        let line = "0000aa9a 6100 00ba                bsr.w #$00ba == $0000ab56";
        let insn = parse_live_line(line).unwrap();
        assert_eq!(insn.addr, Addr::new(0xaa9a));
        assert_eq!(insn.word_count, 2);
        assert_eq!(insn.bytes_hex, "610000ba");
        assert_eq!(insn.mnemonic, "bsr.w");
        assert_eq!(insn.ops, "#$00ba == $0000ab56");
    }

    #[test]
    fn parse_live_line_skips_banner_lines() {
        assert!(parse_live_line("CPU=$d106, VBL=35936").is_none());
    }
}
