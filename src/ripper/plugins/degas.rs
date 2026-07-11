//! Degas PI1/PI2/PI3 + Degas Elite PC1/PC2/PC3 detector & renderer.
//! Layouts: skill `atari-st-degas`. RLE + de-interleave semantics derived
//! from Atari_ST_Sources/C/Warwick Grigg/C_series.5/DEGAS.C (DegasGet /
//! DegasPutByte) — cited for semantics, not ported.

use std::borrow::Cow;

use super::degas_rle::{reinterleave, rle_decode};
use crate::ripper::image::{
    decode_planar, neo_header_at, palette_words, plausible_palette, png_bytes, score_screen,
    Palette, ScreenRes, SCREEN_BYTES,
};
use crate::ripper::{rank_and_cap, Finding, RipKind, RipPlugin};
use crate::types::{MemImage, Offset, PluginId};

/// PI file size: 2 (res) + 32 (palette) + 32000 (screen).
const PI_TOTAL: usize = 34 + SCREEN_BYTES;
/// Defensive cap on a PC compressed stream. Worst-case legal RLE output is
/// 32000 literal bytes + 250 control bytes ≈ 32250; 64 KiB is generous.
const MAX_PC_INPUT: usize = 64 * 1024;
/// Survivors are ranked best-first and capped: the Uhane evidence (45 FPs)
/// showed uncapped output swamps the agent even with good gates (ADR-37).
const MAX_FINDINGS: usize = 8;

pub struct Degas;

impl RipPlugin for Degas {
    fn id(&self) -> PluginId {
        PluginId::new("degas")
    }
    fn kind(&self) -> RipKind {
        RipKind::Picture
    }
    fn describe(&self) -> &'static str {
        "Degas PI1/PI2/PI3 and Degas Elite PC1/PC2/PC3 pictures"
    }
    // Scan even offsets only — 68000 code loads word-aligned data; halves
    // the work. On a hit, jump past the artifact (rounded up to even) so
    // overlapping duplicates of the same picture are not reported as noise.
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        let bytes = mem.bytes();
        let mut out = Vec::new();
        let mut o = 0usize;
        while o + 34 <= bytes.len() {
            match try_pi(bytes, o).or_else(|| try_pc(bytes, o)) {
                Some(f) => {
                    let len = f.len;
                    out.push(f);
                    o += (len + 1) & !1; // skip the artifact, keep even
                }
                None => o += 2,
            }
        }
        rank_and_cap(out, MAX_FINDINGS)
    }
    fn render_png(&self, bytes: &[u8]) -> Option<Vec<u8>> {
        render(bytes)
    }
}

fn be16(bytes: &[u8], o: usize) -> Option<u16> {
    let b = bytes.get(o..o + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

/// Structure-score `screen` (§P21.4) and gate on `plausible()`. Shared by
/// `try_pi`/`try_pc` — the only difference between them is how `screen` is
/// obtained (a direct slice vs a decoded+reinterleaved RLE stream).
fn score_gate(screen: &[u8], res: ScreenRes) -> Option<u8> {
    let score = score_screen(screen, res)?;
    score.plausible().then_some(score.confidence)
}

/// The res/bounds/palette/NEO-deferral gates shared by every PI candidate at
/// `o`, split out so `try_pi` fits the 30-line budget. `w` is returned
/// alongside `res` — `try_pi` needs it for the PI1/2/3 label.
fn pi_header_ok(bytes: &[u8], o: usize) -> Option<(u16, ScreenRes)> {
    let w = be16(bytes, o)?;
    let res = ScreenRes::from_word(w)?; // rejects w > 2
    if o + PI_TOTAL > bytes.len() {
        return None;
    }
    let words = palette_words(&bytes[o + 2..o + 34])?;
    if !plausible_palette(&words) {
        return None;
    }
    // ADR-13: Degas defers to NeoChrome. Every NEO aliases as a PI header at
    // its own offset (flag word 0 = valid low-res word) and at +2 (res word
    // 0 + full palette); the far more specific NEO signature wins the tie.
    if neo_header_at(bytes, o) || (o >= 2 && neo_header_at(bytes, o - 2)) {
        return None;
    }
    Some((w, res))
}

fn try_pi(bytes: &[u8], o: usize) -> Option<Finding> {
    let (w, res) = pi_header_ok(bytes, o)?;
    let confidence = score_gate(&bytes[o + 34..o + 34 + SCREEN_BYTES], res)?;
    Some(
        Finding::new(
            PluginId::new("degas"),
            RipKind::Picture,
            Offset::new(o),
            PI_TOTAL,
            format!(
                "Degas PI{} {}x{} picture ({})",
                w + 1,
                res.width(),
                res.height(),
                res.name()
            ),
        )
        .with_confidence(confidence),
    )
}

fn try_pc(bytes: &[u8], o: usize) -> Option<Finding> {
    let w = be16(bytes, o)?;
    if w & 0x8000 == 0 {
        return None;
    }
    let res = ScreenRes::from_word(w & 0x7FFF)?;
    let words = palette_words(bytes.get(o + 2..o + 34)?)?;
    if !plausible_palette(&words) {
        return None;
    }
    let end = (o + 34 + MAX_PC_INPUT).min(bytes.len());
    let (linear, consumed) = rle_decode(&bytes[o + 34..end])?;
    let confidence = score_gate(&reinterleave(&linear, res), res)?;
    Some(
        Finding::new(
            PluginId::new("degas"),
            RipKind::Picture,
            Offset::new(o),
            34 + consumed,
            format!(
                "Degas PC{} {}x{} RLE picture, {consumed} bytes compressed ({})",
                (w & 0x7FFF) + 1,
                res.width(),
                res.height(),
                res.name()
            ),
        )
        .with_confidence(confidence),
    )
}

fn render(bytes: &[u8]) -> Option<Vec<u8>> {
    let w = be16(bytes, 0)?;
    let res = ScreenRes::from_word(w & 0x7FFF)?;
    let words = palette_words(bytes.get(2..34)?)?;
    let pal = Palette::from_words(&words);
    let screen: Cow<[u8]> = if w & 0x8000 != 0 {
        let (linear, _) = rle_decode(bytes.get(34..)?)?;
        Cow::Owned(reinterleave(&linear, res))
    } else {
        Cow::Borrowed(bytes.get(34..34 + SCREEN_BYTES)?)
    };
    let img = warn_none("degas", decode_planar(&screen, res, &pal))?;
    warn_none("degas", png_bytes(&img))
}

/// Result → Option with a tracing::warn on Err (render contract, `RipPlugin`
/// §4.2). Shared by `neochrome.rs`.
pub(crate) fn warn_none<T>(plugin: &str, r: crate::error::Result<T>) -> Option<T> {
    r.map_err(|e| tracing::warn!(plugin, error = %e, "render_png failed"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Addr;

    /// Same 16-distinct-word ST palette as tests/graphics.rs's PAL fixture
    /// (kept local so this file's unit tests don't depend on the integration
    /// test crate).
    const PAL: [u16; 16] = [
        0x000, 0x700, 0x070, 0x007, 0x770, 0x707, 0x077, 0x777, 0x333, 0x400, 0x040, 0x004, 0x440,
        0x404, 0x044, 0x555,
    ];

    fn pal_bytes() -> Vec<u8> {
        PAL.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    /// Emit run-length control/value pairs (blocks of <= 128) totaling
    /// exactly `n` zero bytes of decoded output.
    fn run_fill(n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut remaining = n;
        while remaining > 0 {
            let block = remaining.min(128);
            out.push((257 - block) as u8);
            out.push(0);
            remaining -= block;
        }
        out
    }

    /// Gate-passing low-res body: every line = 80x0xFF + 80x0x00 (colors
    /// 15|0) — the scorer rejects an all-zero body (P21 G1), so fixtures
    /// that need `try_pi`/`try_pc` to accept use this instead.
    fn body_halves() -> Vec<u8> {
        let mut out = vec![0u8; SCREEN_BYTES];
        for line in out.chunks_mut(160) {
            line[..80].fill(0xFF);
        }
        out
    }

    /// RLE stream decoding to the linear form of `body_halves`: 800
    /// plane-rows, each run(20,0xFF)+run(20,0x00) — 3200 bytes, decodes to
    /// exactly 32000. (0xED = 257-20; the halves pattern is plane-symmetric,
    /// so its linear and interleaved forms coincide per line.)
    fn structured_pc_stream() -> Vec<u8> {
        [0xEDu8, 0xFF, 0xED, 0x00]
            .iter()
            .copied()
            .cycle()
            .take(3200)
            .collect()
    }

    fn pi_header(res_word: u16) -> Vec<u8> {
        let mut out = res_word.to_be_bytes().to_vec();
        out.extend(pal_bytes());
        out.extend(body_halves());
        out
    }

    /// Minimal §8.2-style NEO buffer: satisfies `neo_header_at` (flag, res,
    /// palette, 320x200 geometry at +58/+60); screen data stays zero.
    fn make_neo_like() -> Vec<u8> {
        let mut out = vec![0u8; 128 + SCREEN_BYTES];
        out[4..36].copy_from_slice(&pal_bytes());
        out[58..60].copy_from_slice(&320u16.to_be_bytes());
        out[60..62].copy_from_slice(&200u16.to_be_bytes());
        out
    }

    #[test]
    fn try_pi_rejects() {
        // zero palette (distinct gate)
        let mut buf = vec![0u8; PI_TOTAL];
        assert!(try_pi(&buf, 0).is_none());
        // upper-nibble junk
        buf[..2].copy_from_slice(&0u16.to_be_bytes());
        buf[2..34].copy_from_slice(&pal_bytes());
        buf[2] = 0x17;
        assert!(try_pi(&buf, 0).is_none());
        // body overrunning the buffer
        let short = &pi_header(0)[..PI_TOTAL - 1];
        assert!(try_pi(short, 0).is_none());
        // res word 3
        let bad_res = pi_header(3);
        assert!(try_pi(&bad_res, 0).is_none());
        // ADR-13: a real NEO buffer aliases as a PI header both at its own
        // offset (flag word 0 = valid low-res word) and at +2 (its own res
        // word + full palette) — try_pi must defer to NEO at both.
        let neo = make_neo_like();
        assert!(try_pi(&neo, 0).is_none());
        assert!(try_pi(&neo, 2).is_none());
        // P21 G1: blank body — a valid res word + palette behind an
        // all-zero screen is exactly the Uhane false-positive shape.
        let mut blank = 0u16.to_be_bytes().to_vec();
        blank.extend(pal_bytes());
        blank.extend(vec![0u8; SCREEN_BYTES]);
        assert!(try_pi(&blank, 0).is_none());
    }

    #[test]
    fn try_pi_accepts() {
        let buf = pi_header(0);
        let f = try_pi(&buf, 0).unwrap();
        assert_eq!(f.len, PI_TOTAL);
        assert!(f.label.contains("PI1"));
        assert!(f.confidence.is_some_and(|c| (60..=90).contains(&c)));
    }

    #[test]
    fn try_pc_requires_bit15_and_decode() {
        let mut no_bit15 = 0x0001u16.to_be_bytes().to_vec();
        no_bit15.extend(pal_bytes());
        no_bit15.extend(run_fill(SCREEN_BYTES));
        assert!(try_pc(&no_bit15, 0).is_none());

        let mut good = 0x8000u16.to_be_bytes().to_vec();
        good.extend(pal_bytes());
        good.extend(structured_pc_stream());
        let f = try_pc(&good, 0).unwrap();
        assert_eq!(f.len, 34 + 3200);
        assert!(f.confidence.is_some_and(|c| (60..=90).contains(&c)));

        let mut garbage = 0x8000u16.to_be_bytes().to_vec();
        garbage.extend(pal_bytes());
        garbage.extend([0x02u8, 1, 2]); // truncated literal, never reaches SCREEN_BYTES
        assert!(try_pc(&garbage, 0).is_none());
    }

    #[test]
    fn scan_skips_past_hit() {
        // 0xFF padding (not 0x00): a zero res word is itself a valid Low
        // header, so an all-zero preamble can accidentally straddle-match
        // the real picture's leading palette bytes. 0xFFFF is never a valid
        // res word (nor `0x8000|res`), so every padding-only offset is
        // rejected at the very first check, regardless of what it overlaps.
        let mut buf = vec![0xFFu8; 68 + PI_TOTAL + 64];
        let pic = pi_header(0);
        buf[68..68 + pic.len()].copy_from_slice(&pic);
        let mem = MemImage::new(buf, Addr::new(0));
        let findings = Degas.scan(&mem);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].offset, Offset::new(68));
    }
}
