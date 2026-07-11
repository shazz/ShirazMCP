//! Wire structs + pure video-register decode for the machine_state tool
//! (P10) — split from machine_state.rs for the 200-line budget, same
//! pattern as view_wire.rs. No session/emulator access. The text pretty-
//! printer lives in `machine_pretty.rs` (further split, same budget).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::debugger::RegistersOutput;
use crate::debug;
use crate::error::{Error, Result};
use crate::ripper::image::{is_ste, palette_words, Palette, ScreenRes};
use crate::types::Addr;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MachineStateArgs {
    /// Render the screen buffer as an inline PNG (also saved). Default true.
    pub include_screen: Option<bool>,
    /// Also write the raw 32000-byte dump to disk (path only). Default false.
    pub include_raw_dump: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MachineStateOutput {
    /// "running" | "paused" — state at call time AND after return.
    pub session: String,
    /// True iff the session was Running and was frozen/thawed for coherence.
    pub momentary_freeze: bool,
    /// Reused verbatim from the `registers` tool (tools::debugger).
    pub cpu: RegistersOutput,
    pub video: VideoOutput,
    /// Present iff include_screen or include_raw_dump was in effect.
    pub video_memory: Option<VideoMemoryOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct VideoOutput {
    /// "low" | "medium" | "high" | "invalid" ($FF8260 bits 1-0 == 3).
    pub resolution: String,
    /// "320x200" | "640x200" | "640x400"; None when resolution is invalid.
    pub dimensions: Option<String>,
    /// 50 | 60 ($FF820A bit 1).
    pub sync_hz: u8,
    /// Assembled 24-bit screen base, Addr display form, e.g. "$032200".
    pub screen_base: String,
    /// "st" | "ste" (ripper::image::is_ste over the 16 words).
    pub palette_type: String,
    /// Exactly 16 entries, hardware order.
    pub palette: Vec<PaletteEntry>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PaletteEntry {
    /// Raw register word, "$0777" (4 uppercase hex digits, zero-padded).
    pub raw: String,
    /// Decoded sRGB, "#RRGGBB" (ST/STE-aware via ripper::image::Palette).
    pub rgb: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct VideoMemoryOutput {
    /// Bytes acquired (32000, or 0 when skipped — see note).
    pub len: usize,
    /// Saved rendered PNG (also returned inline), when include_screen.
    pub png_path: Option<String>,
    /// Saved raw dump, when include_raw_dump.
    pub raw_path: Option<String>,
    /// Why the buffer was not acquired. None on success.
    pub note: Option<String>,
}

/// Decoded video-hardware state (pre-wire). `res == None` iff $FF8260 read 3
/// (invariant, machine_state.rs's `gather`: `screen` only when `res.is_some()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoRegs {
    pub base: Addr,
    pub sync_hz: u8,
    pub res: Option<ScreenRes>,
    pub words: [u16; 16],
}

/// `blk` = the 13 bytes at $FFFF8201 (idx 0 hi, 2 mid, 9 sync, 12 low);
/// `res_byte` = the byte at $FFFF8260; `pal` = the 32 bytes at $FFFF8240.
pub(crate) fn video_from_bytes(blk: &[u8], res_byte: u8, pal: &[u8]) -> Result<VideoRegs> {
    if blk.len() < 13 {
        return Err(Error::Parse {
            context: "video base registers".to_string(),
            input: format!("{} bytes, need 13", blk.len()),
        });
    }
    let base = (u32::from(blk[0]) << 16) | (u32::from(blk[2]) << 8) | u32::from(blk[12]);
    let words = palette_words(pal).ok_or_else(|| Error::Parse {
        context: "palette registers".to_string(),
        input: format!("{} bytes, need 32", pal.len()),
    })?;
    // $FF820A bit 1: 1 -> 50 Hz, 0 -> 60 Hz (bit 0 = external sync, ignored).
    Ok(VideoRegs {
        base: Addr::new(base),
        sync_hz: if blk[9] & 0x02 != 0 { 50 } else { 60 },
        res: ScreenRes::from_word(u16::from(res_byte & 3)),
        words,
    })
}

/// `mem` = the wire-ready video-memory summary, pre-digested by compose so
/// this stays pure; None iff neither screen nor dump was requested.
pub(crate) fn build_output(
    session: &str,
    momentary_freeze: bool,
    regs: debug::Registers,
    video: &VideoRegs,
    mem: Option<VideoMemoryOutput>,
) -> MachineStateOutput {
    MachineStateOutput {
        session: session.to_string(),
        momentary_freeze,
        cpu: super::debugger::regs_to_output(regs),
        video: video_output(video),
        video_memory: mem,
    }
}

fn video_output(v: &VideoRegs) -> VideoOutput {
    let pal = Palette::from_words(&v.words);
    let rgb = |c: [u8; 4]| format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
    VideoOutput {
        resolution: v.res.map_or("invalid", ScreenRes::name).to_string(),
        dimensions: v.res.map(|r| format!("{}x{}", r.width(), r.height())),
        sync_hz: v.sync_hz,
        screen_base: v.base.to_string(),
        palette_type: if is_ste(&v.words) { "ste" } else { "st" }.to_string(),
        palette: (0..16u8)
            .map(|i| PaletteEntry {
                raw: format!("${:04X}", v.words[i as usize]),
                rgb: rgb(pal.color(i)),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_video() -> VideoRegs {
        VideoRegs {
            base: Addr::new(0x032200),
            sync_hz: 50,
            res: Some(ScreenRes::Low),
            words: [
                0x0000, 0x0777, 0x0555, 0x0002, 0x0004, 0x0006, 0x0017, 0x0027, 0x0047, 0x0057,
                0x0103, 0x0204, 0x0305, 0x0407, 0x0427, 0x0001,
            ],
        }
    }

    fn probe_palette_bytes() -> Vec<u8> {
        probe_video()
            .words
            .iter()
            .flat_map(|w| w.to_be_bytes())
            .collect()
    }

    fn probe_blk() -> [u8; 13] {
        let mut b = [0u8; 13];
        b[0] = 0x03;
        b[2] = 0x22;
        b[9] = 0x02;
        b
    }

    #[test]
    fn video_from_bytes_probe_pin() {
        let v = video_from_bytes(&probe_blk(), 0, &probe_palette_bytes()).unwrap();
        assert_eq!(v.base, Addr::new(0x032200));
        assert_eq!(v.sync_hz, 50);
        assert_eq!(v.res, Some(ScreenRes::Low));
        assert_eq!(v.words[1], 0x0777);
        assert_eq!(v.words[15], 0x0001);
    }

    #[test]
    fn video_from_bytes_60hz_and_invalid_res() {
        let mut blk = probe_blk();
        blk[9] = 0x00;
        let v = video_from_bytes(&blk, 3, &probe_palette_bytes()).unwrap();
        assert_eq!(v.sync_hz, 60);
        assert_eq!(v.res, None);
    }

    #[test]
    fn video_from_bytes_short_block() {
        let err = video_from_bytes(&probe_blk()[..12], 0, &probe_palette_bytes()).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "video base registers"));
    }

    #[test]
    fn video_from_bytes_short_palette() {
        let pal = &probe_palette_bytes()[..31];
        let err = video_from_bytes(&probe_blk(), 0, pal).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "palette registers"));
    }

    #[test]
    fn palette_entries_st_and_ste() {
        let out = video_output(&probe_video());
        assert_eq!(out.palette_type, "st");
        assert_eq!(out.palette[1].raw, "$0777");
        assert_eq!(out.palette[1].rgb, "#FFFFFF");
        assert_eq!(out.palette[0].rgb, "#000000");

        let mut ste = probe_video();
        ste.words[6] = 0x0888;
        let out = video_output(&ste);
        assert_eq!(out.palette_type, "ste");
        assert_eq!(out.palette[6].raw, "$0888");
        assert_eq!(out.palette[6].rgb, "#111111");
    }

    #[test]
    fn dimensions_none_when_invalid() {
        let mut v = probe_video();
        v.res = None;
        let out = video_output(&v);
        assert_eq!(out.resolution, "invalid");
        assert_eq!(out.dimensions, None);
    }
}
