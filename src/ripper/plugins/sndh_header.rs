//! SNDH header/tag parsing (§4.1). Authority: skill `atari-st-ym-players`;
//! Atari_ST_Sources/C/Toyoyo/stvn/src/sndh.c — cited for semantics
//! (substring tag scan, timer priority, 50 Hz default), not ported.
//! Pure over byte slices; split from sndh.rs for the 200-line budget.

use memchr::memmem;

/// Tag-block scan window past the header magic (§4.1).
pub const TAG_SCAN_MAX: usize = 2048;

/// How the tune expects to be driven (SNDH timer tags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replay {
    TimerA,
    TimerB,
    TimerC,
    TimerD,
    Vbl,
}

impl Replay {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TimerA => "timer-a",
            Self::TimerB => "timer-b",
            Self::TimerC => "timer-c",
            Self::TimerD => "timer-d",
            Self::Vbl => "vbl",
        }
    }
}

/// Parsed SNDH header facts. Offsets are relative to the header base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SndhInfo {
    pub title: Option<String>,
    pub composer: Option<String>,
    pub ripper: Option<String>,
    pub converter: Option<String>,
    pub subtunes: u16,
    pub replay: Replay,
    pub freq_hz: u16,
    pub init: u32,
    pub exit: Option<u32>,
    pub play: u32,
    pub header_len: Option<usize>,
}

/// Timer tags in the pinned priority order (§4.1): first match wins.
/// `sndh.c` spells the VBL tag "V!"; the SNDH spec spells it "!V" —
/// both are accepted.
const TIMERS: [(&[u8], Replay); 6] = [
    (b"TC", Replay::TimerC),
    (b"TA", Replay::TimerA),
    (b"TB", Replay::TimerB),
    (b"TD", Replay::TimerD),
    (b"!V", Replay::Vbl),
    (b"V!", Replay::Vbl),
];

/// Parse an SNDH header whose magic sits at `o + 12`. None = not SNDH
/// (bad magic, invalid init/play branches) — never a panic.
pub fn parse_header(bytes: &[u8], o: usize) -> Option<SndhInfo> {
    if bytes.get(o + 12..o + 16)? != b"SNDH" {
        return None;
    }
    let init = entry_offset(bytes, o, 0)?;
    let play = entry_offset(bytes, o, 2)?;
    let exit = entry_offset(bytes, o, 1);
    let (tags, header_len) = tag_region(bytes, o);

    let (replay, freq_hz) = TIMERS
        .iter()
        .find_map(|(tag, r)| digit_tag(tags, tag).map(|f| (*r, f)))
        .unwrap_or((Replay::Vbl, 50));

    Some(SndhInfo {
        title: cstr_tag(tags, b"TITL"),
        composer: cstr_tag(tags, b"COMM"),
        ripper: cstr_tag(tags, b"RIPP"),
        converter: cstr_tag(tags, b"CONV"),
        subtunes: digit_tag(tags, b"##").unwrap_or(1),
        replay,
        freq_hz,
        init,
        exit,
        play,
        header_len,
    })
}

/// Tag-block slice + `header_len` (§4.1): bounded by `TAG_SCAN_MAX`, clipped
/// at "HDNS" when found (degrades to the full window otherwise, never an
/// error).
fn tag_region(bytes: &[u8], o: usize) -> (&[u8], Option<usize>) {
    let end = (o + 16 + TAG_SCAN_MAX).min(bytes.len());
    let region = &bytes[o + 16..end];
    let hdns = memmem::find(region, b"HDNS");
    let tags = &region[..hdns.unwrap_or(region.len())];
    let header_len = hdns.map(|p| {
        let l = 16 + p + 4;
        l + (l & 1)
    });
    (tags, header_len)
}

/// Decode entry slot `slot` (0=init, 1=exit, 2=play) per §4.1.
/// None = slot word is not bra.w/bra.s, or target invalid.
fn entry_offset(bytes: &[u8], o: usize, slot: usize) -> Option<u32> {
    let w = be16(bytes, o + slot * 4)?;
    let t: i32 = if w == 0x6000 {
        slot as i32 * 4 + 2 + i32::from(be16(bytes, o + slot * 4 + 2)? as i16)
    } else if w >> 8 == 0x60 && !matches!(w & 0xFF, 0x00 | 0xFF) {
        slot as i32 * 4 + 2 + i32::from((w & 0xFF) as u8 as i8)
    } else {
        return None;
    };
    (t >= 12 && t % 2 == 0 && (t as usize) < bytes.len() - o).then_some(t as u32)
}

fn be16(bytes: &[u8], o: usize) -> Option<u16> {
    let b = bytes.get(o..o + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

/// First `tag` occurrence in the tag block → the NUL-terminated string after.
fn cstr_tag(tags: &[u8], tag: &[u8]) -> Option<String> {
    let p = memmem::find(tags, tag)?;
    read_cstr(&tags[p + tag.len()..])
}

/// First `tag` occurrence followed by an ASCII digit → parsed decimal.
/// The digit gate stops timer tags matching inside title text (§4.1).
fn digit_tag(tags: &[u8], tag: &[u8]) -> Option<u16> {
    memmem::find_iter(tags, tag)
        .find(|&p| tags.get(p + tag.len()).is_some_and(u8::is_ascii_digit))
        .map(|p| parse_decimal(&tags[p + tag.len()..]))
}

/// Leading ASCII digits, decimal, saturating at u16::MAX.
fn parse_decimal(bytes: &[u8]) -> u16 {
    bytes
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .take(5)
        .fold(0u16, |v, b| {
            v.saturating_mul(10).saturating_add(u16::from(b - b'0'))
        })
}

/// ASCII value up to the NUL; NUL must appear within 128 bytes (garbage
/// guard) and the value must be non-empty. Non-printables become '?'.
fn read_cstr(bytes: &[u8]) -> Option<String> {
    let window = &bytes[..bytes.len().min(128)];
    let end = window.iter().position(|&b| b == 0)?;
    (end > 0).then(|| {
        window[..end]
            .iter()
            .map(|&b| {
                if (0x20..=0x7E).contains(&b) {
                    b as char
                } else {
                    '?'
                }
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same layout as tests/music.rs's `build_sndh` (§7.1) — kept identical
    /// so the two builders stay pinned together.
    fn build_sndh() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x60, 0x00, 0x00, 0x42]); // bra.w +66 -> 68
        out.extend_from_slice(&[0x60, 0x00, 0x00, 0x40]); // bra.w +64 -> 70
        out.extend_from_slice(&[0x60, 0x00, 0x00, 0x3E]); // bra.w +62 -> 72
        out.extend_from_slice(b"SNDH");
        out.extend_from_slice(b"TITLTest Tune\0");
        out.extend_from_slice(b"COMMClaude\0");
        out.extend_from_slice(b"RIPPShirazMCP\0");
        out.extend_from_slice(b"##02");
        out.extend_from_slice(b"TC50\0");
        out.extend_from_slice(b"HDNS");
        assert_eq!(out.len(), 68);
        out.extend_from_slice(&[0x4E, 0x75, 0x4E, 0x75]); // init: rts / exit: rts
        out.extend_from_slice(&[
            0x41, 0xF8, 0x88, 0x00, // lea.l   $8800.w, a0
            0x11, 0xFC, 0x00, 0x0B, 0x88, 0x00, // move.b  #$b, $8800.w  (select R11)
            0x48, 0xF8, 0x00, 0x0F, 0x88, 0x00, // movem.l d0-d3, $8800.w (hippel-movem-w)
            0x4E, 0x75, // rts
        ]);
        out
    }

    #[test]
    fn golden_full_tags() {
        let info = parse_header(&build_sndh(), 0).unwrap();
        assert_eq!(info.title.as_deref(), Some("Test Tune"));
        assert_eq!(info.composer.as_deref(), Some("Claude"));
        assert_eq!(info.ripper.as_deref(), Some("ShirazMCP"));
        assert_eq!(info.converter, None);
        assert_eq!(info.subtunes, 2);
        assert_eq!(info.replay, Replay::TimerC);
        assert_eq!(info.freq_hz, 50);
        assert_eq!(info.init, 68);
        assert_eq!(info.exit, Some(70));
        assert_eq!(info.play, 72);
        assert_eq!(info.header_len, Some(68));
    }

    #[test]
    fn golden_minimal() {
        let mut bytes = vec![0x60, 0x00, 0x00, 0x0A]; // init: bra.w -> 12
        bytes.extend_from_slice(&[0x60, 0x00, 0x00, 0x08]); // exit: bra.w -> 12
        bytes.extend_from_slice(&[0x60, 0x00, 0x00, 0x06]); // play: bra.w -> 12
        bytes.extend_from_slice(b"SNDH");
        bytes.extend_from_slice(b"HDNS");
        assert_eq!(bytes.len(), 20);
        bytes.extend_from_slice(&[0x4E, 0x75, 0x4E, 0x75, 0x4E, 0x75, 0x4E, 0x75]);
        let info = parse_header(&bytes, 0).unwrap();
        assert_eq!(info.title, None);
        assert_eq!(info.composer, None);
        assert_eq!(info.ripper, None);
        assert_eq!(info.converter, None);
        assert_eq!(info.subtunes, 1);
        assert_eq!(info.replay, Replay::Vbl);
        assert_eq!(info.freq_hz, 50);
        assert_eq!(info.header_len, Some(20));
    }

    #[test]
    fn timer_priority_tc_beats_ta() {
        // "TA60\0" appears before "TC50\0" in the tag block; priority order
        // (§4.1: TC, TA, TB, TD, !V, V!) picks TC regardless of position.
        let mut bytes = vec![0x60, 0x00, 0x00, 0x1A];
        bytes.extend_from_slice(&[0x60, 0x00, 0x00, 0x18]);
        bytes.extend_from_slice(&[0x60, 0x00, 0x00, 0x16]);
        bytes.extend_from_slice(b"SNDH");
        bytes.extend_from_slice(b"TA60\0");
        bytes.extend_from_slice(b"TC50\0");
        bytes.extend_from_slice(b"HDNS");
        let l = bytes.len();
        bytes.resize(l + (l & 1), 0);
        bytes.extend_from_slice(&[0x4E, 0x75, 0x4E, 0x75, 0x4E, 0x75, 0x4E, 0x75]);
        let info = parse_header(&bytes, 0).unwrap();
        assert_eq!(info.replay, Replay::TimerC);
        assert_eq!(info.freq_hz, 50);
    }

    #[test]
    fn vbl_both_spellings() {
        for tag in [&b"!V50\0"[..], &b"V!50\0"[..]] {
            let mut bytes = vec![0x60, 0x00, 0x00, 0x0A];
            bytes.extend_from_slice(&[0x60, 0x00, 0x00, 0x08]);
            bytes.extend_from_slice(&[0x60, 0x00, 0x00, 0x06]);
            bytes.extend_from_slice(b"SNDH");
            bytes.extend_from_slice(tag);
            bytes.extend_from_slice(b"HDNS");
            let l = bytes.len();
            bytes.resize(l + (l & 1), 0);
            bytes.extend_from_slice(&[0x4E, 0x75, 0x4E, 0x75, 0x4E, 0x75, 0x4E, 0x75]);
            let info = parse_header(&bytes, 0).unwrap();
            assert_eq!(info.replay, Replay::Vbl);
            assert_eq!(info.freq_hz, 50);
        }
    }

    #[test]
    fn timer_digit_gate() {
        let mut bytes = vec![0x60, 0x00, 0x00, 0x0A];
        bytes.extend_from_slice(&[0x60, 0x00, 0x00, 0x08]);
        bytes.extend_from_slice(&[0x60, 0x00, 0x00, 0x06]);
        bytes.extend_from_slice(b"SNDH");
        bytes.extend_from_slice(b"TITLTAXI\0");
        bytes.extend_from_slice(b"HDNS");
        let l = bytes.len();
        bytes.resize(l + (l & 1), 0);
        bytes.extend_from_slice(&[0x4E, 0x75, 0x4E, 0x75, 0x4E, 0x75, 0x4E, 0x75]);
        let info = parse_header(&bytes, 0).unwrap();
        assert_eq!(info.replay, Replay::Vbl);
        assert_eq!(info.freq_hz, 50);
    }

    #[test]
    fn missing_hdns_degrades() {
        let mut bytes = build_sndh();
        // Zero the "HDNS" terminator (at offset 64..68).
        bytes[64..68].copy_from_slice(&[0, 0, 0, 0]);
        let info = parse_header(&bytes, 0).unwrap();
        assert_eq!(info.header_len, None);
        assert_eq!(info.title.as_deref(), Some("Test Tune"));
    }

    #[test]
    fn bra_s_entries_accepted() {
        // bra.s targets = 68/70/72; disp8 = target - (slot*4 + 2).
        let mut bytes = vec![0x60, (68i32 - 2) as u8, 0, 0];
        bytes.extend_from_slice(&[0x60, (70i32 - 6) as u8, 0, 0]);
        bytes.extend_from_slice(&[0x60, (72i32 - 10) as u8, 0, 0]);
        bytes.extend_from_slice(b"SNDH");
        bytes.extend_from_slice(b"HDNS");
        bytes.resize(80, 0); // pad well past the play target (72)
        let info = parse_header(&bytes, 0).unwrap();
        assert_eq!(info.init, 68);
        assert_eq!(info.play, 72);
    }

    #[test]
    fn reject_magic_without_branches() {
        let mut bytes = vec![0u8; 32];
        bytes[12..16].copy_from_slice(b"SNDH");
        assert_eq!(parse_header(&bytes, 0), None);
    }

    #[test]
    fn reject_odd_or_backward_target() {
        // slot 0 disp making target 13 (odd): 13 - 2 = 11 = 0x000B
        let mut bytes = vec![0x60, 0x00, 0x00, 0x0B];
        bytes.extend_from_slice(&[0x60, 0x00, 0x00, 0x08]);
        bytes.extend_from_slice(&[0x60, 0x00, 0x00, 0x06]);
        bytes.extend_from_slice(b"SNDH");
        bytes.extend_from_slice(&[0u8; 16]);
        assert_eq!(parse_header(&bytes, 0), None);

        // negative target: disp = -100
        let mut bytes2 = vec![0x60, 0x00];
        bytes2.extend_from_slice(&(-100i16).to_be_bytes());
        bytes2.extend_from_slice(&[0x60, 0x00, 0x00, 0x08]);
        bytes2.extend_from_slice(&[0x60, 0x00, 0x00, 0x06]);
        bytes2.extend_from_slice(b"SNDH");
        bytes2.extend_from_slice(&[0u8; 16]);
        assert_eq!(parse_header(&bytes2, 0), None);
    }

    #[test]
    fn read_cstr_bounds() {
        assert_eq!(read_cstr(&[0x41; 200]), None); // no NUL within 128 bytes
        assert_eq!(read_cstr(&[0]), None); // empty value
        assert_eq!(read_cstr(&[0x01, 0]), Some("?".to_string()));
    }

    #[test]
    fn parse_decimal_saturates() {
        assert_eq!(parse_decimal(b"99999"), 65535);
    }
}
