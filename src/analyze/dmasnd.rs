//! Pure STE DMA-sound register decode (P20) — the $FF8900 block →
//! play/loop, frame [start,end), mono/stereo and sample rate. Semantics
//! pinned from hatari/src/dmaSnd.c + the hardware memory map (docs/plan/
//! P20.md §2); no I/O, no emulator access, the same purity contract as
//! ym.rs.

use crate::error::{Error, Result};

/// Base/length of the one debugger block read: $FF8900 ..= $FF8921
/// (34 bytes). The whole range is mapped without bus errors on the STE
/// (ioMemTabSTE.c) and debugger reads dispatch no handlers (§2.2/§2.3).
pub const DMASND_BLOCK_ADDR: u32 = 0x00FF_8900;
pub const DMASND_BLOCK_LEN: usize = 0x22;

/// $FF8921 bits 1-0 → replay rate (dmaSnd.c `DmaSndSampleRates`).
pub const DMA_RATES_HZ: [u32; 4] = [6258, 12517, 25033, 50066];

/// Decoded DMA-sound state. Addresses are 24-bit with the low bit forced
/// even, exactly like Hatari's own latch (§2.1). `frame_counter` is
/// informational only — a debugger read returns the last value the
/// *program* latched, not the live DMA position (§2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmaSndState {
    pub control: u8,      // $FF8901 raw
    pub play: bool,       // control bit 0 (may be stale after a one-shot, §2.2)
    pub loop_on: bool,    // control bit 1
    pub frame_start: u32, // $FF8903/05/07, & !1
    pub frame_counter: u32,
    pub frame_end: u32, // $FF890F/11/13, & !1 (exclusive)
    pub mode: u8,       // $FF8921 raw (Hatari masks writes to 0x8F)
    pub mono: bool,     // mode bit 7 set = mono, clear = stereo
    pub rate_hz: u32,   // DMA_RATES_HZ[mode & 3]
}

impl DmaSndState {
    /// 1 (mono) or 2 (stereo) — the WAV channel count.
    pub fn channels(&self) -> u16 {
        if self.mono {
            1
        } else {
            2
        }
    }
}

/// Why the `[frame_start, frame_end)` buffer can(not) be ripped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferCheck {
    Ok {
        len: usize,
    },
    /// start == end: no buffer programmed (also the reset state, all-zero).
    Empty,
    /// end < start: registers mid-update or garbage.
    Inverted,
    /// end beyond ST RAM — unrippable via savebin.
    OutOfRam {
        end: u32,
        ram: usize,
    },
}

/// Block index = bus address − $FF8900 (all data bytes are at odd offsets).
fn addr24(block: &[u8], hi: usize, mid: usize, lo: usize) -> u32 {
    (u32::from(block[hi]) << 16) | (u32::from(block[mid]) << 8) | u32::from(block[lo] & !1)
}

/// Decode the 34-byte $FF8900 block. Err(Parse{context:"dmasnd block"})
/// iff `block.len() < DMASND_BLOCK_LEN` (extra bytes are ignored —
/// read_bytes already truncates).
pub fn decode(block: &[u8]) -> Result<DmaSndState> {
    if block.len() < DMASND_BLOCK_LEN {
        return Err(Error::Parse {
            context: "dmasnd block".to_string(),
            input: format!("{} bytes, need >= {DMASND_BLOCK_LEN}", block.len()),
        });
    }
    let control = block[0x01];
    let mode = block[0x21];
    Ok(DmaSndState {
        control,
        play: control & 1 != 0,
        loop_on: control & 2 != 0,
        frame_start: addr24(block, 0x03, 0x05, 0x07),
        frame_counter: addr24(block, 0x09, 0x0B, 0x0D),
        frame_end: addr24(block, 0x0F, 0x11, 0x13),
        mode,
        mono: mode & 0x80 != 0,
        rate_hz: DMA_RATES_HZ[usize::from(mode & 3)],
    })
}

/// Pure gate for the extraction plan (§9.3). Order of checks: Empty
/// (start == end) → Inverted (end < start) → OutOfRam (end as usize >
/// ram_len) → Ok { len: (end - start) as usize }.
pub fn check_buffer(s: &DmaSndState, ram_len: usize) -> BufferCheck {
    if s.frame_start == s.frame_end {
        return BufferCheck::Empty;
    }
    if s.frame_end < s.frame_start {
        return BufferCheck::Inverted;
    }
    if s.frame_end as usize > ram_len {
        return BufferCheck::OutOfRam {
            end: s.frame_end,
            ram: ram_len,
        };
    }
    BufferCheck::Ok {
        len: (s.frame_end - s.frame_start) as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pinned 34-byte block (§11.1): control $03 (play+loop) at 0x01;
    /// start $03/$00/$01 (odd lo → masks to $030000); counter $03/$40/$00
    /// → $034000; end $03/$A0/$00 → $03A000; mode $81 at 0x21 (mono, rate
    /// index 1); every other byte 0.
    fn pinned_block() -> [u8; DMASND_BLOCK_LEN] {
        let mut b = [0u8; DMASND_BLOCK_LEN];
        b[0x01] = 0x03;
        b[0x03] = 0x03;
        b[0x05] = 0x00;
        b[0x07] = 0x01;
        b[0x09] = 0x03;
        b[0x0B] = 0x40;
        b[0x0D] = 0x00;
        b[0x0F] = 0x03;
        b[0x11] = 0xA0;
        b[0x13] = 0x00;
        b[0x21] = 0x81;
        b
    }

    #[test]
    fn decode_pinned_block() {
        let s = decode(&pinned_block()).unwrap();
        assert!(s.play);
        assert!(s.loop_on);
        assert_eq!(s.frame_start, 0x030000);
        assert_eq!(s.frame_counter, 0x034000);
        assert_eq!(s.frame_end, 0x03A000);
        assert!(s.mono);
        assert_eq!(s.rate_hz, 12517);
        assert_eq!(s.control, 0x03);
        assert_eq!(s.mode, 0x81);
        assert_eq!(s.channels(), 1);
    }

    #[test]
    fn decode_all_rates() {
        let cases: [(u8, bool, u32); 4] = [
            (0x00, false, 6258),
            (0x01, false, 12517),
            (0x02, false, 25033),
            (0x83, true, 50066),
        ];
        for (mode, mono, rate) in cases {
            let mut b = pinned_block();
            b[0x21] = mode;
            let s = decode(&b).unwrap();
            assert_eq!(s.mono, mono, "mode {mode:#04x}");
            assert_eq!(s.rate_hz, rate, "mode {mode:#04x}");
            assert_eq!(s.channels(), if mono { 1 } else { 2 });
        }
    }

    // P20.md §11.1 names the changed lo byte `$A1`, but $A1 & !1 == $A0 (not
    // $00), which would shift the visible hex digits (0x03A0A0), not
    // reproduce the documented 0x03A000. $01 is the value that actually
    // masks away to $00 — the same odd-lo demonstration
    // `decode_pinned_block` already relies on for `frame_start` — and is
    // the only value consistent with the spec's own asserted result.
    #[test]
    fn decode_masks_end_low_bit() {
        let mut b = pinned_block();
        b[0x13] = 0x01;
        let s = decode(&b).unwrap();
        assert_eq!(s.frame_end, 0x03A000);
    }

    #[test]
    fn decode_rejects_short() {
        let short = &pinned_block()[..DMASND_BLOCK_LEN - 1];
        let err = decode(short).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "dmasnd block"));
    }

    fn state_with(start: u32, end: u32) -> DmaSndState {
        DmaSndState {
            control: 0,
            play: false,
            loop_on: false,
            frame_start: start,
            frame_counter: 0,
            frame_end: end,
            mode: 0,
            mono: false,
            rate_hz: DMA_RATES_HZ[0],
        }
    }

    #[test]
    fn check_buffer_ok() {
        let s = state_with(0x030000, 0x03A000);
        assert_eq!(
            check_buffer(&s, 4 * 1024 * 1024),
            BufferCheck::Ok { len: 0xA000 }
        );
    }

    #[test]
    fn check_buffer_empty() {
        let s = state_with(0, 0);
        assert_eq!(check_buffer(&s, 4 * 1024 * 1024), BufferCheck::Empty);
    }

    #[test]
    fn check_buffer_inverted() {
        let s = state_with(0x030000, 0x020000);
        assert_eq!(check_buffer(&s, 4 * 1024 * 1024), BufferCheck::Inverted);
    }

    #[test]
    fn check_buffer_out_of_ram() {
        let s = state_with(0x100000, 0x120000);
        assert_eq!(
            check_buffer(&s, 1024 * 1024),
            BufferCheck::OutOfRam {
                end: 0x120000,
                ram: 1024 * 1024
            }
        );
        assert_eq!(
            check_buffer(&s, 4 * 1024 * 1024),
            BufferCheck::Ok { len: 0x20000 }
        );
    }
}
