//! ST planar-graphics decode (P4): interleaved bitplanes → chunky → RGBA,
//! ST/STE palette words → 8-bit color. Pure over byte slices; shared by the
//! degas/neochrome plugins and the `view_memory_as_screen` tool. Layout ref:
//! skill `atari-st-bitplanes`; Stone Tower/src/STLOWC2P.S (c2p, reversed).
//! `decode_planar`/`png_bytes` (bulk render path) live in `render.rs` —
//! split out for the 200-line budget, re-exported so callers keep using
//! `crate::ripper::image::{decode_planar, png_bytes}` unchanged.

mod render;
mod score;

pub use render::{decode_planar, png_bytes};
pub use score::{score_screen, PictureScore};

/// Every ST screen is exactly 32000 bytes, all three resolutions.
pub const SCREEN_BYTES: usize = 32000;

/// The three ST video modes. `Med` is the file/register value 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenRes {
    Low,  // 320×200, 16 colors, 4 planes
    Med,  // 640×200,  4 colors, 2 planes
    High, // 640×400, mono,      1 plane
}

impl ScreenRes {
    /// Degas/NEO resolution word and the `$FF8260` register value.
    pub const fn from_word(w: u16) -> Option<Self> {
        match w {
            0 => Some(Self::Low),
            1 => Some(Self::Med),
            2 => Some(Self::High),
            _ => None,
        }
    }

    pub const fn width(self) -> u32 {
        match self {
            Self::Low => 320,
            Self::Med | Self::High => 640,
        }
    }

    pub const fn height(self) -> u32 {
        match self {
            Self::Low | Self::Med => 200,
            Self::High => 400,
        }
    }

    pub const fn planes(self) -> usize {
        match self {
            Self::Low => 4,
            Self::Med => 2,
            Self::High => 1,
        }
    }

    /// width/8 × planes: 160 | 160 | 80. bytes_per_line × height == 32000.
    pub const fn bytes_per_line(self) -> usize {
        (self.width() as usize / 8) * self.planes()
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Med => "medium",
            Self::High => "high",
        }
    }
}

/// 16 decoded RGBA colors. Construct with `from_words`; index with `color`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    colors: [[u8; 4]; 16],
}

impl Palette {
    /// ST/STE auto-detection is per-palette: STE iff ANY of the 16 words has
    /// nibble bit 3 set (an ST program can only write 0-7 per nibble).
    pub fn from_words(words: &[u16; 16]) -> Self {
        let ste = is_ste(words);
        Self {
            colors: words.map(|w| decode_word(w, ste)),
        }
    }

    /// `index` is masked to 0..=15 — never panics.
    pub fn color(&self, index: u8) -> [u8; 4] {
        self.colors[(index & 15) as usize]
    }
}

/// STE iff ANY of the 16 words has a nibble bit 3 set (an ST program can
/// only write 0-7 per nibble). The single ST/STE detection rule — palette
/// decoding and `machine_state`'s `palette_type` (P10) both call this.
pub fn is_ste(words: &[u16; 16]) -> bool {
    words.iter().any(|w| w & 0x0888 != 0)
}

fn decode_word(w: u16, ste: bool) -> [u8; 4] {
    let chan = |n: u16| -> u8 {
        if ste {
            (((((n & 7) << 1) | (n >> 3)) * 17) & 0xFF) as u8
        } else {
            ((n & 7) * 255 / 7) as u8
        }
    };
    [
        chan((w >> 8) & 0xF),
        chan((w >> 4) & 0xF),
        chan(w & 0xF),
        255,
    ]
}

/// Read 16 big-endian words from a 32-byte slice. None iff bytes.len() < 32.
pub fn palette_words(bytes: &[u8]) -> Option<[u16; 16]> {
    let b = bytes.get(..32)?;
    let mut out = [0u16; 16];
    for (i, w) in out.iter_mut().enumerate() {
        *w = u16::from_be_bytes([b[2 * i], b[2 * i + 1]]);
    }
    Some(out)
}

/// Detection heuristic shared by degas/neochrome: every word's unused upper
/// nibble is clear AND at least 4 distinct values appear — the distinctness
/// gate stops zero-filled RAM matching at every offset (ADR-12).
pub fn plausible_palette(words: &[u16; 16]) -> bool {
    if words.iter().any(|w| w & 0xF000 != 0) {
        return false;
    }
    let mut sorted = *words;
    sorted.sort_unstable();
    let distinct = 1 + sorted.windows(2).filter(|p| p[0] != p[1]).count();
    distinct >= 4
}

/// (ADR-13) NEO header at `o`: flag 0, valid res word, plausible palette at
/// +4, width/height == 320/200 at +58/+60 (NEO_IO.C). Lives here (not
/// neochrome.rs) — degas.rs uses it too, as a negative guard.
pub fn neo_header_at(bytes: &[u8], o: usize) -> bool {
    let word = |off: usize| {
        bytes
            .get(off..off + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    };
    let pal_ok = bytes
        .get(o + 4..o + 36)
        .and_then(palette_words)
        .is_some_and(|w| plausible_palette(&w));
    word(o) == Some(0)
        && word(o + 2).is_some_and(|w| ScreenRes::from_word(w).is_some())
        && pal_ok
        && word(o + 58) == Some(320)
        && word(o + 60) == Some(200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn res_from_word_roundtrip() {
        let cases = [
            (ScreenRes::Low, 320, 200, 4, 160),
            (ScreenRes::Med, 640, 200, 2, 160),
            (ScreenRes::High, 640, 400, 1, 80),
        ];
        for (i, (res, w, h, planes, bpl)) in cases.into_iter().enumerate() {
            assert_eq!(ScreenRes::from_word(i as u16), Some(res));
            assert_eq!(res.width(), w);
            assert_eq!(res.height(), h);
            assert_eq!(res.planes(), planes);
            assert_eq!(res.bytes_per_line(), bpl);
            assert_eq!(bpl * h as usize, SCREEN_BYTES);
        }
        assert_eq!(ScreenRes::from_word(3), None);
    }

    fn pal6() -> [u16; 16] {
        let mut w = [
            0x000, 0x700, 0x070, 0x007, 0x777, 0x123, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        for (i, slot) in w.iter_mut().enumerate().skip(6) {
            // ST-safe filler: every nibble <= 7, so bit 3 is never set
            // regardless of `i` (unlike e.g. `0x100 + i`, which sets bit 3
            // of the low nibble once i reaches 8 and silently flips the
            // whole palette to STE decoding).
            let n = (i % 8) as u16;
            *slot = (n << 8) | (n << 4) | n;
        }
        w
    }

    #[test]
    fn st_palette_values() {
        let pal = Palette::from_words(&pal6());
        assert_eq!(pal.color(0), [0, 0, 0, 255]);
        assert_eq!(pal.color(1), [255, 0, 0, 255]);
        assert_eq!(pal.color(2), [0, 255, 0, 255]);
        assert_eq!(pal.color(3), [0, 0, 255, 255]);
        assert_eq!(pal.color(4), [255, 255, 255, 255]);
        assert_eq!(pal.color(5), [36, 72, 109, 255]);
    }

    #[test]
    fn ste_palette_rotation() {
        let mut words = pal6();
        words[6] = 0x0888;
        words[7] = 0x0777;
        let pal = Palette::from_words(&words);
        assert_eq!(pal.color(6), [17, 17, 17, 255]);
        assert_eq!(pal.color(7), [238, 238, 238, 255]);
    }

    #[test]
    fn color_index_masked() {
        let pal = Palette::from_words(&pal6());
        assert_eq!(pal.color(200), pal.color(8));
    }

    #[test]
    fn plausible_palette_gates() {
        assert!(!plausible_palette(&[0u16; 16]));
        let mut junk = pal6();
        junk[0] = 0x1777;
        assert!(!plausible_palette(&junk));
        assert!(plausible_palette(&pal6()));
        let mut three = [0x000u16; 16];
        three[1] = 0x111;
        three[2] = 0x222;
        assert!(!plausible_palette(&three));
        three[3] = 0x333;
        assert!(plausible_palette(&three));
    }

    #[test]
    fn is_ste_detection() {
        assert!(!is_ste(&pal6()));
        let mut words = pal6();
        words[6] = 0x0888;
        assert!(is_ste(&words));
    }

    #[test]
    fn palette_words_short() {
        assert_eq!(palette_words(&[0u8; 31]), None);
        let mut bytes = vec![0u8; 33];
        bytes[0] = 0x07;
        bytes[1] = 0x77;
        let words = palette_words(&bytes).unwrap();
        assert_eq!(words[0], 0x0777);
    }

    /// 16 distinct ST-range (no nibble bit 3 set) palette words, one per
    /// index — used by `neo_header_at_gates` as a plausible palette.
    fn counting_pal() -> [u16; 16] {
        let mut w = [0u16; 16];
        for (i, slot) in w.iter_mut().enumerate() {
            let r = (i as u16) & 7;
            let g = (i as u16 >> 3) & 7;
            *slot = (r << 8) | (g << 4);
        }
        w
    }

    /// 62 bytes: flag(2) + res(2) + palette(32) + filename/anim(20, unused
    /// by `neo_header_at`) + width(2) + height(2).
    fn neo_prefix_bytes(width: u16, height: u16) -> Vec<u8> {
        let mut out = vec![0u8; 62];
        for (i, w) in counting_pal().iter().enumerate() {
            out[4 + 2 * i..6 + 2 * i].copy_from_slice(&w.to_be_bytes());
        }
        out[58..60].copy_from_slice(&width.to_be_bytes());
        out[60..62].copy_from_slice(&height.to_be_bytes());
        out
    }

    #[test]
    fn neo_header_at_gates() {
        let good = neo_prefix_bytes(320, 200);
        assert!(neo_header_at(&good, 0));

        let mut bad_width = good.clone();
        bad_width[58..60].copy_from_slice(&0u16.to_be_bytes());
        assert!(!neo_header_at(&bad_width, 0));

        // A PI1-style header: res word (== NEO's flag, coincidentally 0 for
        // low-res) + palette at +2..+34, but bytes 58..62 are screen data,
        // not the pinned 320/200 geometry words — must not alias as NEO.
        let mut pi_like = vec![0u8; 62];
        for (i, w) in counting_pal().iter().enumerate() {
            pi_like[2 + 2 * i..4 + 2 * i].copy_from_slice(&w.to_be_bytes());
        }
        assert!(!neo_header_at(&pi_like, 0));

        assert!(!neo_header_at(&good[..60], 0));
    }
}
