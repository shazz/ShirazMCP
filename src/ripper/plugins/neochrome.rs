//! NeoChrome .NEO detector & renderer. Layout: skill `atari-st-neochrome`,
//! authority Atari_ST_Sources/C/Jean Lusetti/Vision 4.1a/TOOLS/NEO_IO.C —
//! cited for semantics, not ported. Fixed 32128 bytes = 128-byte header +
//! 32000-byte interleaved screen. NEO's palette is at +4 (Degas's is at +2
//! — do not confuse them).

use crate::ripper::image::{
    decode_planar, neo_header_at, palette_words, png_bytes, score_screen, Palette, ScreenRes,
    SCREEN_BYTES,
};
use crate::ripper::plugins::degas::warn_none;
use crate::ripper::{Finding, RipKind, RipPlugin};
use crate::types::{MemImage, Offset, PluginId};

/// Header (128 bytes) + screen (32000 bytes).
pub const NEO_TOTAL: usize = 128 + SCREEN_BYTES;

pub struct NeoChrome;

impl RipPlugin for NeoChrome {
    fn id(&self) -> PluginId {
        PluginId::new("neochrome")
    }
    fn kind(&self) -> RipKind {
        RipKind::Picture
    }
    fn describe(&self) -> &'static str {
        "NeoChrome .NEO pictures (128-byte header + 32000-byte screen)"
    }
    // Same even-offset walk + skip-past-hit pattern as Degas (degas.rs).
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        let bytes = mem.bytes();
        let mut out = Vec::new();
        let mut o = 0usize;
        while o + NEO_TOTAL <= bytes.len() {
            match try_neo(bytes, o) {
                Some(f) => {
                    out.push(f);
                    o += NEO_TOTAL; // already even
                }
                None => o += 2,
            }
        }
        out
    }
    fn render_png(&self, bytes: &[u8]) -> Option<Vec<u8>> {
        render(bytes)
    }
}

fn be16(bytes: &[u8], o: usize) -> Option<u16> {
    let b = bytes.get(o..o + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn try_neo(bytes: &[u8], o: usize) -> Option<Finding> {
    // neo_header_at (ADR-13) also enforces width/height == 320/200 at
    // +58/+60 — without that geometry gate, a low-res Degas PI whose color
    // 0 is black aliases as a NEO header (flag=res word=0, palette window
    // over PAL[1..]+screen data passes plausible_palette too).
    if !neo_header_at(bytes, o) || o + NEO_TOTAL > bytes.len() {
        return None;
    }
    let res = ScreenRes::from_word(be16(bytes, o + 2)?)?;
    let cycling = if be16(bytes, o + 48)? & 0x8000 != 0 {
        ", color-cycling"
    } else {
        ""
    };
    let f = Finding::new(
        PluginId::new("neochrome"),
        RipKind::Picture,
        Offset::new(o),
        NEO_TOTAL,
        format!(
            "NeoChrome {}x{} picture ({}){cycling}",
            res.width(),
            res.height(),
            res.name()
        ),
    );
    let screen = &bytes[o + 128..o + 128 + SCREEN_BYTES];
    Some(annotate_confidence(f, screen, res))
}

/// Confidence annotation only — NEO's six-word geometry gate (ADR-13) is
/// already ~2^-48 specific; gating on structure would only break blank-
/// screen NEO rips for no precision gain (P21 §9.4).
fn annotate_confidence(f: Finding, screen: &[u8], res: ScreenRes) -> Finding {
    match score_screen(screen, res) {
        Some(s) => f.with_confidence(s.confidence),
        None => f,
    }
}

fn render(bytes: &[u8]) -> Option<Vec<u8>> {
    let res = ScreenRes::from_word(be16(bytes, 2)?)?;
    let words = palette_words(bytes.get(4..36)?)?;
    let pal = Palette::from_words(&words);
    let screen = bytes.get(128..128 + SCREEN_BYTES)?;
    let img = warn_none("neochrome", decode_planar(screen, res, &pal))?;
    warn_none("neochrome", png_bytes(&img))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Addr;

    const PAL: [u16; 16] = [
        0x000, 0x700, 0x070, 0x007, 0x770, 0x707, 0x077, 0x777, 0x333, 0x400, 0x040, 0x004, 0x440,
        0x404, 0x044, 0x555,
    ];

    fn make_neo() -> Vec<u8> {
        let mut out = Vec::with_capacity(NEO_TOTAL);
        out.extend_from_slice(&0u16.to_be_bytes()); // flag
        out.extend_from_slice(&0u16.to_be_bytes()); // res = low
        out.extend(PAL.iter().flat_map(|w| w.to_be_bytes()));
        out.extend(vec![0u8; 12]); // filename
        out.extend_from_slice(&0u16.to_be_bytes()); // color-anim limits
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&320u16.to_be_bytes());
        out.extend_from_slice(&200u16.to_be_bytes());
        out.extend(vec![0u8; 66]);
        assert_eq!(out.len(), 128);
        out.extend(vec![0u8; SCREEN_BYTES]);
        assert_eq!(out.len(), NEO_TOTAL);
        out
    }

    #[test]
    fn detects_neo_header() {
        let mut buf = vec![0u8; 40 + NEO_TOTAL];
        let neo = make_neo();
        buf[40..40 + NEO_TOTAL].copy_from_slice(&neo);
        let mem = MemImage::new(buf, Addr::new(0));
        let findings = NeoChrome.scan(&mem);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].len, NEO_TOTAL);
        assert!(findings[0].label.contains("NeoChrome 320x200"));
        assert!(findings[0].confidence.is_some());
    }

    /// §8.2-style PI1 buffer: res word (0, low) + palette at +2..+34 — this
    /// coincidentally satisfies NEO's flag(0)/res/palette checks too, but
    /// its bytes[58..62] are screen data, not the pinned 320/200 geometry.
    fn make_pi_like() -> Vec<u8> {
        let mut out = vec![0u8; NEO_TOTAL];
        let pal_bytes: Vec<u8> = PAL.iter().flat_map(|w| w.to_be_bytes()).collect();
        out[2..34].copy_from_slice(&pal_bytes);
        out
    }

    #[test]
    fn rejects() {
        let mut nonzero_flag = make_neo();
        nonzero_flag[1] = 1;
        assert!(try_neo(&nonzero_flag, 0).is_none());

        let mut bad_res = make_neo();
        bad_res[2..4].copy_from_slice(&3u16.to_be_bytes());
        assert!(try_neo(&bad_res, 0).is_none());

        let all_zero = vec![0u8; NEO_TOTAL];
        assert!(try_neo(&all_zero, 0).is_none());

        let neo = make_neo();
        assert!(try_neo(&neo[..NEO_TOTAL - 1], 0).is_none());

        // ADR-13: geometry gate rejects the PI-alias.
        assert!(try_neo(&make_pi_like(), 0).is_none());
    }

    #[test]
    fn cycling_label() {
        let mut neo = make_neo();
        neo[48..50].copy_from_slice(&0x8000u16.to_be_bytes());
        let f = try_neo(&neo, 0).unwrap();
        assert!(f.label.ends_with("color-cycling"));
    }
}
