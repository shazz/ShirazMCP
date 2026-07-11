//! Masked-word matcher + types for player fingerprints (§4.3 of P6,
//! ADR-16; DB corpus-derived in P19, ADR-35, and lives in `ym_sig_db.rs`).
//! Unknown always degrades to ("unknown", 0.25) — never an error.
//! WholeImage caveat unchanged: a data magic names every cluster in the
//! image.

use super::ym_sig_db::SIGNATURES;

/// One masked big-endian word: matches iff (word & mask) == value.
#[derive(Debug, Clone, Copy)]
pub struct SigWord {
    pub value: u16,
    pub mask: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigScope {
    AnchorWindow,
    WholeImage,
}

pub struct Signature {
    pub player: &'static str,
    pub scope: SigScope,
    pub confidence: f32,
    pub pattern: &'static [SigWord],
}

/// `identify()`'s result — always well-formed, never an error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerId {
    pub player: &'static str,
    pub confidence: f32,
}

/// Anchor found but no signature matched (§4.3).
pub const CONF_ANCHOR_ONLY: f32 = 0.25;
/// AnchorWindow search slack around a cluster, bytes each side.
pub const WINDOW: usize = 512;

/// Name the player behind an anchor cluster. Best = highest confidence,
/// tie -> first in DB order; nothing matched -> ("unknown", 0.25).
pub fn identify(buf: &[u8], cluster_start: usize, cluster_end: usize) -> PlayerId {
    let mut best: Option<&Signature> = None;
    for sig in SIGNATURES {
        let (lo, hi, step) = match sig.scope {
            SigScope::AnchorWindow => (
                cluster_start.saturating_sub(WINDOW) & !1,
                (cluster_end + WINDOW).min(buf.len()),
                2,
            ),
            SigScope::WholeImage => (0, buf.len(), 1),
        };
        if match_in(buf, lo, hi, sig.pattern, step)
            && best.is_none_or(|b| sig.confidence > b.confidence)
        {
            best = Some(sig);
        }
    }
    best.map_or(
        PlayerId {
            player: "unknown",
            confidence: CONF_ANCHOR_ONLY,
        },
        |s| PlayerId {
            player: s.player,
            confidence: s.confidence,
        },
    )
}

/// Any pattern start in [lo, hi) (stepped); the pattern may extend past hi
/// but must fit the buffer.
fn match_in(buf: &[u8], lo: usize, hi: usize, pat: &[SigWord], step: usize) -> bool {
    (lo..hi).step_by(step).any(|i| matches_at(buf, i, pat))
}

fn matches_at(buf: &[u8], i: usize, pat: &[SigWord]) -> bool {
    if i + 2 * pat.len() > buf.len() {
        return false;
    }
    pat.iter().enumerate().all(|(j, sw)| {
        let word = u16::from_be_bytes([buf[i + 2 * j], buf[i + 2 * j + 1]]);
        word & sw.mask == sw.value
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §4.3 hippel-movem-w signature, laid out as raw code bytes (same
    /// shape as ym_player.rs's `replay_block`).
    fn replay_block_bytes() -> Vec<u8> {
        vec![
            0x41, 0xF8, 0x88, 0x00, // lea.l   $8800.w, a0
            0x11, 0xFC, 0x00, 0x0B, 0x88, 0x00, // move.b  #$b, $8800.w  (select R11)
            0x48, 0xF8, 0x00, 0x0F, 0x88, 0x00, // movem.l d0-d3, $8800.w (hippel-movem-w)
            0x4E, 0x75, // rts
        ]
    }

    /// Test-local mirror of the deleted production `w` helper — the DB now
    /// lives in `ym_sig_db.rs`, but these two mechanics tests build their
    /// own throwaway patterns and don't need the real DB.
    const fn w(value: u16, mask: u16) -> SigWord {
        SigWord { value, mask }
    }

    #[test]
    fn hippel_movem_w_matches_replay_block() {
        let id = identify(&replay_block_bytes(), 0, 18);
        assert_eq!(id.player, "Jochen Hippel (Mad Max)");
        assert!((id.confidence - 0.82).abs() < 1e-6);
    }

    #[test]
    fn masked_words_accept_any_value() {
        let pat = [w(0x1234, 0xFFFF), w(0x0000, 0x0000), w(0xABCD, 0xFFFF)];
        let mut a = vec![0x12, 0x34, 0x00, 0x00, 0xAB, 0xCD];
        let mut b = a.clone();
        a[2] = 0x11;
        a[3] = 0x22; // masked word differs
        b[2] = 0x99;
        b[3] = 0x88;
        assert!(matches_at(&a, 0, &pat));
        assert!(matches_at(&b, 0, &pat));
    }

    #[test]
    fn register_mask_accepts_d0_through_d7() {
        let pat = [w(0x13C0, 0xFFF8)];
        for reg in 0u16..=7 {
            let buf = (0x13C0 | reg).to_be_bytes();
            assert!(matches_at(&buf, 0, &pat), "d{reg} should match");
        }
        let buf = 0x13C8u16.to_be_bytes();
        assert!(!matches_at(&buf, 0, &pat));
    }

    #[test]
    fn whole_image_names_distant_magic() {
        let mut buf = vec![0u8; 0x1010];
        buf[0..8].copy_from_slice(b"AN COOL.");
        let id = identify(&buf, 0x1000, 0x1008);
        assert_eq!(id.player, "TCB Tracker");
        assert!((id.confidence - 0.80).abs() < 1e-6);
    }

    #[test]
    fn anchor_window_does_not_leak() {
        let mut buf = vec![0u8; 0x3000];
        buf[0..18].copy_from_slice(&replay_block_bytes());
        let id = identify(&buf, 0x2000, 0x2010);
        assert_eq!(id.player, "unknown");
        assert!((id.confidence - CONF_ANCHOR_ONLY).abs() < 1e-6);
    }

    #[test]
    fn best_of_multiple() {
        let mut buf = vec![0u8; 16];
        buf[0..4].copy_from_slice(b"TFMX");
        buf[8..16].copy_from_slice(b"AN COOL.");
        let id = identify(&buf, 0, 4);
        assert_eq!(id.player, "TCB Tracker");
    }

    #[test]
    fn empty_buffer_unknown() {
        let id = identify(&[], 0, 0);
        assert_eq!(id.player, "unknown");
        assert!((id.confidence - CONF_ANCHOR_ONLY).abs() < 1e-6);
    }
}
