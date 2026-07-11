//! MPP (Multi-Palette Picture) detector — Zerkman/Sector One's format for
//! more than 16 colours via per-scanline palette rewrites (semantics pinned
//! from RECOIL 6.4.5 recoil.c, cited not ported; container: codeberg.org/
//! zerkman/mpp README). Renderer lives in `mpp_decode.rs` (line budget).
//! Canonical modes are 0..=3; mode 4..=7 is reported header-only (ADR-26).

use super::mpp_decode::render;
use crate::ripper::{Finding, RipKind, RipPlugin};
use crate::types::{MemImage, Offset, PluginId};

/// "MPP" + mode + flags + 3 reserved + u32 BE extra_len.
pub(crate) const HEADER_LEN: usize = 12;
/// Cap on the MPPH metadata block (short text tags; 4 KiB is generous).
const MAX_EXTRA: usize = 4096;

/// Palette-entry encoding, from flags bits 1..0 (bit 0 STE, bit 1 extra).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaletteKind {
    St9,
    Ste12,
    Ste15,
}

impl PaletteKind {
    fn from_flags(flags: u8) -> Option<Self> {
        match flags & 3 {
            0 => Some(Self::St9),
            1 => Some(Self::Ste12),
            3 => Some(Self::Ste15),
            _ => None, // extra palette bit without STE is invalid
        }
    }
    pub(crate) const fn bits(self) -> usize {
        match self {
            Self::St9 => 9,
            Self::Ste12 => 12,
            Self::Ste15 => 15,
        }
    }
    const fn name(self) -> &'static str {
        match self {
            Self::St9 => "ST 512",
            Self::Ste12 => "STE 4096",
            Self::Ste15 => "STE 4096+extra",
        }
    }
}

/// Parsed, size-validated header for the four canonical modes (0..=3).
#[derive(Debug, Clone, Copy)]
pub(crate) struct MppHeader {
    pub mode: u8,
    pub kind: PaletteKind,
    pub double: bool,
    pub extra_len: usize,
}

impl MppHeader {
    pub(crate) const fn width(&self) -> usize {
        [320, 320, 320, 416][self.mode as usize]
    }
    /// Stored lines: README's 200/276 is *displayed*, RECOIL's 199/273 is
    /// *stored* and wins — length-validates real files. // verify vs RECOIL.
    pub(crate) const fn height(&self) -> usize {
        [199, 199, 199, 273][self.mode as usize]
    }
    /// Reload range at line start (§4.4): modes 0-2 -> 1..=15, mode 3 -> 6..=15.
    pub(crate) const fn line_start_color(&self) -> usize {
        [1, 1, 1, 6][self.mode as usize]
    }
    /// Stream entries consumed per scanline (RECOIL MODE_COLORS_PER_LINE).
    pub(crate) const fn entries_per_line(&self) -> usize {
        [52, 46, 54, 48][self.mode as usize]
    }
    /// Total bits rounded up to a 16-bit multiple, in bytes (§4.1).
    pub(crate) const fn palette_bytes(&self) -> usize {
        (self.entries_per_line() * self.height() * self.kind.bits()).div_ceil(16) * 2
    }
    /// Always 4 interleaved planes: width/2 bytes per line.
    pub(crate) const fn bitmap_bytes(&self) -> usize {
        self.width() * self.height() / 2
    }
    /// 1, or 2 for a double-image file (flags bit 2, §4.1).
    pub(crate) const fn frames(&self) -> usize {
        1 + self.double as usize // WHY: usize::from(bool) is not yet const
    }
    pub(crate) const fn total_len(&self) -> usize {
        HEADER_LEN + self.extra_len + (self.palette_bytes() + self.bitmap_bytes()) * self.frames()
    }

    /// Gates + canonical mode + whole-artifact-fits; None for mode 4..=7
    /// (`try_mpp` reports those header-only).
    pub(crate) fn parse(bytes: &[u8], o: usize) -> Option<Self> {
        let (mode, flags, extra_len) = gates(bytes, o)?;
        if mode > 3 {
            return None;
        }
        let hdr = Self {
            mode,
            kind: PaletteKind::from_flags(flags)?,
            double: flags & 4 != 0,
            extra_len,
        };
        (o + hdr.total_len() <= bytes.len()).then_some(hdr)
    }
}

/// §4.2 gates 1-6: magic, mode<=7, flags shape, reserved zero, extra_len
/// even+capped, "MPPH" iff extra_len!=0 (mode may still be 4..=7).
fn gates(bytes: &[u8], o: usize) -> Option<(u8, u8, usize)> {
    let h = bytes.get(o..o + HEADER_LEN)?;
    if &h[..3] != b"MPP" || h[3] > 7 || h[4] & 0xF8 != 0 || h[4] & 3 == 2 || h[5..8] != [0, 0, 0] {
        return None;
    }
    let extra = u32::from_be_bytes([h[8], h[9], h[10], h[11]]) as usize;
    if !extra.is_multiple_of(2) || extra > MAX_EXTRA {
        return None;
    }
    if extra != 0 && bytes.get(o + 12..o + 16)? != b"MPPH" {
        return None;
    }
    Some((h[3], h[4], extra))
}

pub struct Mpp;

impl RipPlugin for Mpp {
    fn id(&self) -> PluginId {
        PluginId::new("mpp")
    }
    fn kind(&self) -> RipKind {
        RipKind::Picture
    }
    fn describe(&self) -> &'static str {
        "MPP multi-palette pictures (per-scanline palette changes; modes 0-3 rendered)"
    }
    /// Magic-anchored memmem scan (ADR-14, no alignment); skips prior hits.
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        let bytes = mem.bytes();
        let mut out = Vec::new();
        let mut next = 0usize;
        for o in memchr::memmem::find_iter(bytes, b"MPP") {
            if o < next {
                continue;
            }
            if let Some(f) = try_mpp(bytes, o) {
                next = o + f.len;
                out.push(f);
            }
        }
        out
    }
    fn render_png(&self, bytes: &[u8]) -> Option<Vec<u8>> {
        render(&MppHeader::parse(bytes, 0)?, bytes)
    }
}

fn try_mpp(bytes: &[u8], o: usize) -> Option<Finding> {
    if let Some(hdr) = MppHeader::parse(bytes, o) {
        return Some(Finding::new(
            PluginId::new("mpp"),
            RipKind::Picture,
            Offset::new(o),
            hdr.total_len(),
            label(&hdr),
        ));
    }
    // Unsupported mode 4..=7: header-only finding so it still surfaces
    // though undecoded (§4.0, ADR-26); overrun canonical modes fall through.
    let (mode, _, extra_len) = gates(bytes, o)?;
    (mode > 3).then(|| {
        Finding::new(
            PluginId::new("mpp"),
            RipKind::Picture,
            Offset::new(o),
            HEADER_LEN + extra_len,
            format!("MPP mode {mode} picture (unsupported mode, header-only)"),
        )
    })
}

const MODE_DESC: [&str; 4] = [
    "54 colors/line",
    "48 colors/line",
    "56 colors/line, blitter",
    "48+6 colors/line, overscan",
];

fn label(h: &MppHeader) -> String {
    let double = if h.double { ", double-image" } else { "" };
    format!(
        "MPP mode {} {}x{} picture ({}, {}{double})",
        h.mode,
        h.width(),
        h.height(),
        MODE_DESC[h.mode as usize],
        h.kind.name(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Addr;

    fn header(mode: u8, flags: u8, extra: u32) -> Vec<u8> {
        let mut out = b"MPP".to_vec();
        out.extend([mode, flags, 0, 0, 0]);
        out.extend(extra.to_be_bytes());
        out
    }

    #[test]
    fn gates_accept_reject() {
        assert!(gates(&header(2, 0, 0), 0).is_some());
        assert!(gates(&header(2, 1, 0), 0).is_some());
        assert!(gates(&header(2, 3, 0), 0).is_some());
        assert!(gates(&header(2, 4, 0), 0).is_some());
        assert!(gates(&header(5, 0, 0), 0).is_some());

        let mut wrong_magic = header(2, 0, 0);
        wrong_magic[..3].copy_from_slice(b"MPQ");
        assert!(gates(&wrong_magic, 0).is_none());

        assert!(gates(&header(8, 0, 0), 0).is_none()); // mode 8
        assert!(gates(&header(2, 2, 0), 0).is_none()); // extra w/o STE
        assert!(gates(&header(2, 0x08, 0), 0).is_none()); // reserved flag bit
        let mut bad_reserved = header(2, 0, 0);
        bad_reserved[5] = 1;
        assert!(gates(&bad_reserved, 0).is_none());

        let mut ok_extra = header(2, 0, 6);
        ok_extra.extend(b"MPPH");
        assert!(gates(&ok_extra, 0).is_some());
        assert!(gates(&header(2, 0, 5), 0).is_none()); // odd extra_len
        assert!(gates(&header(2, 0, 4098), 0).is_none()); // > MAX_EXTRA

        let mut extra_no_mpph = header(2, 0, 8);
        extra_no_mpph.extend(b"XXXX");
        assert!(gates(&extra_no_mpph, 0).is_none());

        assert!(gates(&header(2, 0, 0)[..11], 0).is_none()); // 11-byte buffer
    }

    #[test]
    fn pinned_sizes() {
        let h = |mode, kind, double| MppHeader {
            mode,
            kind,
            double,
            extra_len: 0,
        };
        assert_eq!(h(0, PaletteKind::St9, false).total_len(), 43494);
        assert_eq!(h(1, PaletteKind::St9, false).total_len(), 42152);
        assert_eq!(h(2, PaletteKind::St9, false).total_len(), 43942);
        assert_eq!(h(3, PaletteKind::St9, false).total_len(), 71538);
        assert_eq!(h(2, PaletteKind::Ste12, false).total_len(), 47972);
        assert_eq!(h(2, PaletteKind::Ste15, false).total_len(), 52002);
        assert_eq!(h(2, PaletteKind::Ste12, true).total_len(), 95932);
        assert_eq!(h(2, PaletteKind::St9, false).palette_bytes(), 12090);
        assert_eq!(h(3, PaletteKind::St9, false).palette_bytes(), 14742);
    }

    #[test]
    fn parse_requires_whole_artifact() {
        let mut buf = header(2, 0, 0);
        buf.resize(43942 - 1, 0);
        assert!(MppHeader::parse(&buf, 0).is_none());
        assert!(try_mpp(&buf, 0).is_none());

        let mut exact = header(2, 0, 0);
        exact.resize(43942, 0);
        let hdr = MppHeader::parse(&exact, 0).unwrap();
        assert_eq!(hdr.total_len(), 43942);
    }

    #[test]
    fn unknown_mode_header_only() {
        let mut buf = vec![0u8; 4096];
        buf[100..112].copy_from_slice(&header(5, 0, 0));
        let mem = MemImage::new(buf, Addr::new(0));
        let findings = Mpp.scan(&mem);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].len, 12);
        assert!(findings[0].label.contains("mode 5"));
        assert!(findings[0].label.contains("unsupported"));

        let mut buf2 = vec![0u8; 4096];
        let mut h = header(5, 0, 8);
        h.extend(b"MPPHHPPM");
        buf2[100..100 + h.len()].copy_from_slice(&h);
        let mem2 = MemImage::new(buf2, Addr::new(0));
        let findings2 = Mpp.scan(&mem2);
        assert_eq!(findings2.len(), 1);
        assert_eq!(findings2[0].len, 20);
    }

    /// Full corruption sweep lives in `tests/mpp.rs::render_truncated_is_none`
    /// (needs a real BitWriter-built file); these two need only the header.
    #[test]
    fn render_png_corrupt_is_none() {
        assert!(Mpp.render_png(&[]).is_none());
        assert!(Mpp.render_png(&header(2, 2, 0)).is_none()); // bad flags
    }
}
