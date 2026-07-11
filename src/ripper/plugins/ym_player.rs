//! YM2149 replay locator (§4.2, ADR-16): byte candidates for the
//! `$FFFF8800`/`$FFFF8802` operands, confirmed by one-instruction Capstone
//! decodes (P2 `analyze::disasm`), clustered, then named via `ym_signatures`.
//! The label carries `player`/`confidence`/`hits` in a pinned
//! machine-parseable format (`format_label`/`parse_label` round-trip) — the
//! channel `tools/music.rs` reads structured results back through.
//!
//! Performance note: candidates in a real 4 MB RAM image number in the
//! dozens; `confirm` builds Capstone up to 3x per candidate (P2 pins
//! per-call construction — `Capstone` is not `Sync`), microseconds each. Do
//! NOT try to cache a `Capstone` in the plugin (it would break `Send + Sync`
//! for `RipPlugin`).

use memchr::memmem;

use crate::analyze::disasm::disasm;
use crate::ripper::plugins::ym_signatures;
use crate::ripper::{Finding, RipKind, RipPlugin};
use crate::types::{Addr, MemImage, Offset, PluginId};

/// Confirmed hits this close (bytes) merge into one replay cluster.
pub const CLUSTER_GAP: usize = 512;
/// Instruction starts to try behind an operand: opcode | +imm.w | +imm.l.
const BACKS: [usize; 3] = [2, 4, 6];

#[derive(Debug, Clone, Copy)]
struct Hit {
    off: usize,
    len: usize,
}

#[derive(Debug, Clone, Copy)]
struct Cluster {
    start: usize,
    end: usize,
    hits: usize,
}

pub struct YmPlayer;

impl RipPlugin for YmPlayer {
    fn id(&self) -> PluginId {
        PluginId::new("ym-player")
    }
    fn kind(&self) -> RipKind {
        RipKind::Music
    }
    fn describe(&self) -> &'static str {
        "YM2149 replay routines ($FFFF8800/$FFFF8802 write anchor + player fingerprint)"
    }
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        let hits: Vec<Hit> = find_candidates(mem.bytes())
            .into_iter()
            .filter_map(|c| confirm(mem, c))
            .collect();
        clusters(hits)
            .into_iter()
            .map(|c| {
                let id = ym_signatures::identify(mem.bytes(), c.start, c.end);
                Finding::new(
                    self.id(),
                    RipKind::Music,
                    Offset::new(c.start),
                    c.end - c.start,
                    format_label(id.player, id.confidence, c.hits),
                )
            })
            .collect()
    }
}

/// Even offsets of PSG operand byte patterns (§4.2 step 1). Short-form hits
/// preceded by FF FF are the tail of a long-form hit — suppressed.
fn find_candidates(bytes: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    for pat in [&[0xFF, 0xFF, 0x88, 0x00][..], &[0xFF, 0xFF, 0x88, 0x02]] {
        out.extend(memmem::find_iter(bytes, pat).filter(|i| i % 2 == 0));
    }
    for pat in [&[0x88, 0x00][..], &[0x88, 0x02]] {
        out.extend(
            memmem::find_iter(bytes, pat)
                .filter(|&i| i % 2 == 0 && !(i >= 2 && bytes[i - 2..i] == [0xFF, 0xFF])),
        );
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// §4.2 step 2: try instruction starts at cand-2/-4/-6; accept the first
/// single-insn decode that is a move/lea whose text references the PSG and
/// whose bytes cover the operand. None = data, not code.
fn confirm(mem: &MemImage, cand: usize) -> Option<Hit> {
    for back in BACKS {
        let Some(off) = cand.checked_sub(back) else {
            continue;
        };
        let at = Addr::new(mem.base().raw() + off as u32);
        let Ok(insns) = disasm(mem, at, 1) else {
            continue;
        };
        let Some(i) = insns.first() else { continue };
        if (i.mnemonic.starts_with("move") || i.mnemonic.starts_with("lea"))
            && is_psg(&i.ops)
            && i.bytes.len() > back
        {
            return Some(Hit {
                off,
                len: i.bytes.len(),
            });
        }
    }
    None
}

/// P2-pinned Capstone spellings of the two PSG ports (long + short forms).
/// // verify live: re-pin against P2.md §6.2 table on any capstone bump.
fn is_psg(ops: &str) -> bool {
    ["$ffff8800", "$ffff8802", "$8800.w", "$8802.w"]
        .iter()
        .any(|t| ops.contains(t))
}

/// §4.2 step 3. Input order arbitrary; output sorted by start.
fn clusters(mut hits: Vec<Hit>) -> Vec<Cluster> {
    hits.sort_unstable_by_key(|h| h.off);
    hits.dedup_by_key(|h| h.off);
    let mut out: Vec<Cluster> = Vec::new();
    for h in hits {
        match out.last_mut() {
            Some(c) if h.off <= c.end + CLUSTER_GAP => {
                c.end = c.end.max(h.off + h.len);
                c.hits += 1;
            }
            _ => out.push(Cluster {
                start: h.off,
                end: h.off + h.len,
                hits: 1,
            }),
        }
    }
    out
}

/// The pinned label wire-format. `player` is bracketed so names with spaces
/// and parentheses survive; confidence prints 2 decimals (DB constants are
/// 2-decimal literals, §4.3, so the round-trip is exact).
pub fn format_label(player: &str, confidence: f32, hits: usize) -> String {
    format!("YM replay [{player}] confidence={confidence:.2} hits={hits}")
}

/// Inverse of `format_label`. None = not a ym-player label (defensive; the
/// tool treats that as no identification, §6.2).
pub fn parse_label(label: &str) -> Option<(String, f32, u32)> {
    let rest = label.strip_prefix("YM replay [")?;
    let close = rest.rfind("] confidence=")?;
    let tail = &rest[close + "] confidence=".len()..];
    let (conf, hits) = tail.split_once(" hits=")?;
    Some((
        rest[..close].to_string(),
        conf.parse().ok()?,
        hits.parse().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 20 bytes of hand-assembled replay code containing the §4.3
    /// hippel-code signature.
    fn replay_block() -> Vec<u8> {
        vec![
            0x41, 0xF8, 0x88, 0x00, // lea.l   $8800.w, a0
            0x13, 0xFC, 0x00, 0x0D, 0xFF, 0xFF, 0x88, 0x00, // move.b  #$d, $ffff8800.l
            0x13, 0xC0, 0xFF, 0xFF, 0x88, 0x02, // move.b  d0, $ffff8802.l
            0x4E, 0x75, // rts
        ]
    }

    /// 10 bytes: short-form PSG writes only — anchors, matches NO signature.
    fn raw_writes_block() -> Vec<u8> {
        vec![
            0x11, 0xC0, 0x88, 0x00, // move.b d0, $8800.w
            0x11, 0xC0, 0x88, 0x02, // move.b d0, $8802.w
            0x4E, 0x75, // rts
        ]
    }

    #[test]
    fn candidates_long_short_and_suppression() {
        let mut buf = vec![0u8; 32];
        let rb = replay_block();
        buf[0..rb.len()].copy_from_slice(&rb);
        assert_eq!(find_candidates(&buf), vec![2, 8, 14]);
    }

    #[test]
    fn confirm_hits_pinned() {
        let mem = MemImage::new(replay_block(), Addr::new(0x10000));
        let findings = YmPlayer.scan(&mem);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].offset, Offset::new(0));
        assert_eq!(findings[0].len, 18);
    }

    #[test]
    fn data_not_code_rejected() {
        let mut buf = vec![0u8; 4];
        buf.extend_from_slice(&[0xFF, 0xFF, 0x88, 0x00]);
        buf.extend_from_slice(&[0u8; 4]);
        let mem = MemImage::new(buf, Addr::new(0));
        assert!(YmPlayer.scan(&mem).is_empty());
    }

    #[test]
    fn short_form_writes_confirmed() {
        let mem = MemImage::new(raw_writes_block(), Addr::new(0));
        let findings = YmPlayer.scan(&mem);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].offset, Offset::new(0));
        assert_eq!(findings[0].len, 8);
        assert_eq!(
            findings[0].label,
            "YM replay [unknown] confidence=0.25 hits=2"
        );
    }

    #[test]
    fn cluster_gap_splits() {
        let rb = raw_writes_block();
        let mut close = vec![0u8; 0x40 + rb.len()];
        close[0..rb.len()].copy_from_slice(&rb);
        close[0x40..0x40 + rb.len()].copy_from_slice(&rb);
        let mem = MemImage::new(close, Addr::new(0));
        assert_eq!(YmPlayer.scan(&mem).len(), 1);

        let mut far = vec![0u8; 0x800 + rb.len()];
        far[0..rb.len()].copy_from_slice(&rb);
        far[0x800..0x800 + rb.len()].copy_from_slice(&rb);
        let mem = MemImage::new(far, Addr::new(0));
        assert_eq!(YmPlayer.scan(&mem).len(), 2);
    }

    #[test]
    fn label_round_trip() {
        let label = format_label("Jochen Hippel (Mad Max)", 0.70, 3);
        let (player, conf, hits) = parse_label(&label).unwrap();
        assert_eq!(player, "Jochen Hippel (Mad Max)");
        assert!((conf - 0.70).abs() < 1e-6);
        assert_eq!(hits, 3);
    }

    #[test]
    fn parse_label_rejects_foreign() {
        assert_eq!(parse_label("Degas PI1 320x200 (low)"), None);
    }

    #[test]
    fn odd_candidate_ignored() {
        let mut buf = vec![0u8; 16];
        buf[3..5].copy_from_slice(&[0x88, 0x00]);
        assert!(find_candidates(&buf).is_empty());
    }
}
