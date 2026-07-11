//! SNDH detector (§4.1). Finds `SNDH` blobs anywhere in a buffer; header
//! facts parsed by `sndh_header`. Length is the pinned 64 KiB cap heuristic
//! (ADR-16) — the format records no total length.

use memchr::memmem;

use crate::ripper::plugins::sndh_header::{parse_header, SndhInfo};
use crate::ripper::{Finding, RipKind, RipPlugin};
use crate::types::{MemImage, Offset, PluginId};

/// SNDH records no total length; rip a capped window (§4.1, ADR-16).
pub const SNDH_RIP_CAP: usize = 65536;

pub struct Sndh;

impl RipPlugin for Sndh {
    fn id(&self) -> PluginId {
        PluginId::new("sndh")
    }
    fn kind(&self) -> RipKind {
        RipKind::Music
    }
    fn describe(&self) -> &'static str {
        "SNDH music blobs (init/exit/play entries + TITL/COMM/RIPP/subtune/timer tags)"
    }
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        let bytes = mem.bytes();
        memmem::find_iter(bytes, b"SNDH")
            .filter_map(|m| m.checked_sub(12))
            .filter_map(|o| {
                let info = parse_header(bytes, o)?;
                let len = (bytes.len() - o).min(SNDH_RIP_CAP);
                Some(Finding::new(
                    self.id(),
                    RipKind::Music,
                    Offset::new(o),
                    len,
                    label(&info),
                ))
            })
            .collect()
    }
    // no render_png: music does not render (trait default None).
}

/// Pinned label — human-readable; structured facts come from re-running
/// `parse_header` over the finding bytes (tools/music.rs), NOT from parsing
/// this string.
fn label(info: &SndhInfo) -> String {
    format!(
        "SNDH '{}' by {} ({} subtune(s), {} {}Hz)",
        info.title.as_deref().unwrap_or("untitled"),
        info.composer.as_deref().unwrap_or("unknown"),
        info.subtunes,
        info.replay.as_str(),
        info.freq_hz
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Addr;

    /// Same builder as sndh_header.rs's tests / tests/music.rs §7.1 — kept
    /// byte-identical across the three copies (self-checked below).
    fn build_sndh() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x60, 0x00, 0x00, 0x42]);
        out.extend_from_slice(&[0x60, 0x00, 0x00, 0x40]);
        out.extend_from_slice(&[0x60, 0x00, 0x00, 0x3E]);
        out.extend_from_slice(b"SNDH");
        out.extend_from_slice(b"TITLTest Tune\0");
        out.extend_from_slice(b"COMMClaude\0");
        out.extend_from_slice(b"RIPPShirazMCP\0");
        out.extend_from_slice(b"##02");
        out.extend_from_slice(b"TC50\0");
        out.extend_from_slice(b"HDNS");
        assert_eq!(out.len(), 68);
        out.extend_from_slice(&[0x4E, 0x75, 0x4E, 0x75]);
        out.extend_from_slice(&[
            0x41, 0xF8, 0x88, 0x00, // lea.l   $8800.w, a0
            0x11, 0xFC, 0x00, 0x0B, 0x88, 0x00, // move.b  #$b, $8800.w  (select R11)
            0x48, 0xF8, 0x00, 0x0F, 0x88, 0x00, // movem.l d0-d3, $8800.w (hippel-movem-w)
            0x4E, 0x75, // rts
        ]);
        out
    }

    #[test]
    fn detects_blob_mid_buffer() {
        let mut buf = vec![0u8; 4096];
        let sndh = build_sndh();
        buf[40..40 + sndh.len()].copy_from_slice(&sndh);
        let mem = MemImage::new(buf, Addr::new(0));
        let findings = Sndh.scan(&mem);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].offset, Offset::new(40));
        assert_eq!(findings[0].len, (4096 - 40).min(SNDH_RIP_CAP));
        assert_eq!(
            findings[0].label,
            "SNDH 'Test Tune' by Claude (2 subtune(s), timer-c 50Hz)"
        );
    }

    #[test]
    fn two_adjacent_blobs_both_found() {
        let mut buf = vec![0u8; 65536];
        let sndh = build_sndh();
        buf[0..sndh.len()].copy_from_slice(&sndh);
        buf[4096..4096 + sndh.len()].copy_from_slice(&sndh);
        let mem = MemImage::new(buf, Addr::new(0));
        let findings = Sndh.scan(&mem);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].offset, Offset::new(0));
        assert_eq!(findings[1].offset, Offset::new(4096));
    }

    #[test]
    fn magic_before_offset_12_skipped() {
        let mut buf = vec![0u8; 64];
        buf[3..7].copy_from_slice(b"SNDH");
        let mem = MemImage::new(buf, Addr::new(0));
        assert!(Sndh.scan(&mem).is_empty());
    }

    #[test]
    fn plain_text_sndh_rejected() {
        let mut buf = vec![0x20u8; 64]; // spaces
        buf[20..24].copy_from_slice(b"SNDH");
        let mem = MemImage::new(buf, Addr::new(0));
        assert!(Sndh.scan(&mem).is_empty());
    }
}
