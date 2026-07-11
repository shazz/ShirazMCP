//! MPP palette bitstream + per-scanline change engine (§4.3/§4.4 of P13).
//! Semantics pinned from RECOIL 6.4.5 recoil.c (RECOIL_DecodeMppScreen,
//! MppPaletteStream_Read, BitStream_ReadBit, GetSteInterlacedColor) —
//! cited, not ported. Renders the FIRST palette+bitmap pair only; a
//! double-image file's second (blend) frame is deliberately skipped and the
//! finding label says "double-image" (ADR-26).

use image::RgbaImage;

use super::degas::warn_none;
use super::mpp::{MppHeader, PaletteKind, HEADER_LEN};
use crate::ripper::image::png_bytes;

/// Per-mode x position of the first in-line palette change (§4.4).
const FIRST_CHANGE_X: [usize; 4] = [33, 9, 4, 69];
/// Change index whose value is forced black instead of read from the
/// stream (the right border must be black); 127 = never fires (mode 3).
const FORCED_BLACK: [u32; 4] = [32, 16, 32, 127];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// MSB-first bit reader over the palette section (RECOIL BitStream
/// semantics: bits taken from each byte high-to-low, bytes in order; one
/// continuous stream for the whole frame — no per-line realignment).
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize, // bit position
}

impl BitReader<'_> {
    fn read_bits(&mut self, n: u32) -> Option<u16> {
        let mut v = 0u16;
        for _ in 0..n {
            let byte = *self.data.get(self.pos >> 3)?;
            v = v << 1 | u16::from(byte >> (7 - (self.pos & 7)) & 1);
            self.pos += 1;
        }
        Some(v)
    }
}

/// Decode the first palette+bitmap pair of `bytes` (header at 0, already
/// parsed by the caller) to encoded PNG. None = corrupt input, never panic.
pub(crate) fn render(hdr: &MppHeader, bytes: &[u8]) -> Option<Vec<u8>> {
    let pal_off = HEADER_LEN + hdr.extra_len;
    let mut stream = BitReader {
        data: bytes.get(pal_off..pal_off + hdr.palette_bytes())?,
        pos: 0,
    };
    let bitmap = bytes
        .get(pal_off + hdr.palette_bytes()..)?
        .get(..hdr.bitmap_bytes())?;
    let (w, h, bpl) = (hdr.width(), hdr.height(), hdr.width() / 2);
    let mut img = RgbaImage::new(w as u32, h as u32);
    // ONE palette for the whole frame — state persists across lines
    // (mode 3's slots 0..=5 depend on it, §4.4).
    let mut palette = [BLACK; 16];
    for y in 0..h {
        for slot in palette.iter_mut().skip(hdr.line_start_color()) {
            *slot = read_entry(&mut stream, hdr.kind)?;
        }
        render_line(
            &mut img,
            y,
            &bitmap[y * bpl..(y + 1) * bpl],
            &mut palette,
            &mut stream,
            hdr,
        )?;
    }
    warn_none("mpp", png_bytes(&img))
}

/// One scanline: walk x, rewrite palette slots at the pinned change
/// positions, look each pixel up in the palette AS OF that x (§4.4).
fn render_line(
    img: &mut RgbaImage,
    y: usize,
    line: &[u8],
    palette: &mut [[u8; 4]; 16],
    stream: &mut BitReader<'_>,
    hdr: &MppHeader,
) -> Option<()> {
    let mode = hdr.mode as usize;
    let mut change_x = FIRST_CHANGE_X[mode];
    let mut change_color = 0u32;
    for x in 0..hdr.width() {
        if x == change_x {
            palette[(change_color & 15) as usize] = if change_color == FORCED_BLACK[mode] {
                BLACK
            } else {
                read_entry(stream, hdr.kind)?
            };
            change_x += step(hdr.mode, change_color);
            change_color += 1;
        }
        img.put_pixel(
            x as u32,
            y as u32,
            image::Rgba(palette[pixel_index(line, x)]),
        );
    }
    Some(())
}

/// x-advance AFTER processing change `cc` (RECOIL DecodeMppScreen switch).
fn step(mode: u8, cc: u32) -> usize {
    match mode {
        1 => {
            if cc.is_multiple_of(2) {
                4
            } else {
                16
            }
        }
        2 => 8,
        // modes 0 and 3 share the table, differing only at cc == 15
        _ => match cc {
            15 => {
                if mode == 0 {
                    88
                } else {
                    112
                }
            }
            31 => 12,
            37 => 100,
            _ => 4,
        },
    }
}

/// Standard ST 4-plane word interleave (the same layout image/render.rs
/// decodes, duplicated here for one line at a time): group x/16 is 4
/// consecutive BE words, pixel bit 15-(x%16), plane pl -> index bit pl.
/// In-bounds by construction: line.len() == width/2 == (width/16)*8.
fn pixel_index(line: &[u8], x: usize) -> usize {
    let (base, bit) = (x / 16 * 8, 15 - (x % 16));
    let mut idx = 0usize;
    for pl in 0..4 {
        let w = u16::from_be_bytes([line[base + 2 * pl], line[base + 2 * pl + 1]]);
        idx |= (((w >> bit) & 1) as usize) << pl;
    }
    idx
}

/// One palette entry -> RGBA (§4.3). 9-bit: linear rrrgggbbb, house ST
/// scaling v*255/7 (image/mod.rs decode_word ST branch; RECOIL
/// bit-replicates — <=1/255 apart at v in {2,4,6}, deliberate, ADR-26).
/// 12-bit: three rotated STE hardware nibbles, v4*17 (identical to RECOIL).
/// 15-bit: bits 14..12 are per-channel extra LSBs over the same 12-bit
/// word; (v5<<3)|(v5>>2) — exactly RECOIL GetSteInterlacedColor, already
/// blended, rendered as a single frame.
fn read_entry(s: &mut BitReader<'_>, kind: PaletteKind) -> Option<[u8; 4]> {
    Some(match kind {
        PaletteKind::St9 => {
            let v = s.read_bits(9)?;
            [st8(v >> 6 & 7), st8(v >> 3 & 7), st8(v & 7), 255]
        }
        PaletteKind::Ste12 => {
            let v = s.read_bits(12)?;
            [ste8(v >> 8 & 15), ste8(v >> 4 & 15), ste8(v & 15), 255]
        }
        PaletteKind::Ste15 => {
            let v = s.read_bits(15)?;
            [
                five8(v >> 8 & 15, v >> 14 & 1),
                five8(v >> 4 & 15, v >> 13 & 1),
                five8(v & 15, v >> 12 & 1),
                255,
            ]
        }
    })
}

/// STE hardware nibble -> linear 4-bit (the channel LSB sits in bit 3).
fn rot4(n: u16) -> u16 {
    ((n & 7) << 1) | (n >> 3)
}
fn st8(v: u16) -> u8 {
    (v * 255 / 7) as u8
}
fn ste8(n: u16) -> u8 {
    (rot4(n) * 17) as u8
}
fn five8(n: u16, extra: u16) -> u8 {
    let v5 = (rot4(n) << 1) | extra;
    ((v5 << 3) | (v5 >> 2)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_reader_msb_first() {
        let data = [0b1011_0001u8, 0b1000_0000];
        let mut s = BitReader {
            data: &data,
            pos: 0,
        };
        assert_eq!(s.read_bits(3), Some(0b101));
        assert_eq!(s.read_bits(6), Some(0b100011));
        assert_eq!(s.read_bits(7), Some(0));
        assert_eq!(s.read_bits(1), None);
    }

    fn bits_stream(v: u16, n: u32) -> Vec<u8> {
        // Left-align v's low n bits into a byte-padded MSB-first stream.
        let mut buf = vec![0u8; (n as usize).div_ceil(8)];
        for i in 0..n {
            if (v >> (n - 1 - i)) & 1 != 0 {
                buf[(i / 8) as usize] |= 1 << (7 - (i % 8));
            }
        }
        buf
    }

    #[test]
    fn entry_st9() {
        let data = bits_stream(0b111_000_000, 9);
        let mut s = BitReader {
            data: &data,
            pos: 0,
        };
        assert_eq!(read_entry(&mut s, PaletteKind::St9), Some([255, 0, 0, 255]));

        let data = bits_stream(0b000_000_111, 9);
        let mut s = BitReader {
            data: &data,
            pos: 0,
        };
        assert_eq!(read_entry(&mut s, PaletteKind::St9), Some([0, 0, 255, 255]));

        let data = bits_stream(0b010_010_010, 9);
        let mut s = BitReader {
            data: &data,
            pos: 0,
        };
        assert_eq!(
            read_entry(&mut s, PaletteKind::St9),
            Some([72, 72, 72, 255])
        );
    }

    #[test]
    fn entry_ste12() {
        let data = bits_stream(0x888, 12);
        let mut s = BitReader {
            data: &data,
            pos: 0,
        };
        assert_eq!(
            read_entry(&mut s, PaletteKind::Ste12),
            Some([17, 17, 17, 255])
        );

        let data = bits_stream(0x777, 12);
        let mut s = BitReader {
            data: &data,
            pos: 0,
        };
        assert_eq!(
            read_entry(&mut s, PaletteKind::Ste12),
            Some([238, 238, 238, 255])
        );
    }

    #[test]
    fn entry_ste15() {
        let data = bits_stream(0b100_0111_0000_0000, 15);
        let mut s = BitReader {
            data: &data,
            pos: 0,
        };
        assert_eq!(
            read_entry(&mut s, PaletteKind::Ste15),
            Some([239, 0, 0, 255])
        );

        let data = bits_stream(0b111_1111_1111_1111, 15);
        let mut s = BitReader {
            data: &data,
            pos: 0,
        };
        assert_eq!(
            read_entry(&mut s, PaletteKind::Ste15),
            Some([255, 255, 255, 255])
        );

        let data = bits_stream(0, 15);
        let mut s = BitReader {
            data: &data,
            pos: 0,
        };
        assert_eq!(read_entry(&mut s, PaletteKind::Ste15), Some([0, 0, 0, 255]));
    }

    /// Walk the change engine for one mode, counting stream reads and total
    /// changes, without assuming the §4.4 identities — they are asserted.
    fn walk(mode: u8, width: usize) -> (u32, u32) {
        let mut change_x = FIRST_CHANGE_X[mode as usize];
        let mut change_color = 0u32;
        let mut reads = 0u32;
        while change_x < width {
            if change_color != FORCED_BLACK[mode as usize] {
                reads += 1;
            }
            change_x += step(mode, change_color);
            change_color += 1;
        }
        (reads, change_color)
    }

    #[test]
    fn change_engine_consistency() {
        let widths = [320, 320, 320, 416];
        let line_start = [15, 15, 15, 10];
        let entries = [52, 46, 54, 48];
        let total_changes = [38, 32, 40, 38];
        for mode in 0..4u8 {
            let (reads, changes) = walk(mode, widths[mode as usize]);
            assert_eq!(
                line_start[mode as usize] + reads,
                entries[mode as usize],
                "mode {mode}"
            );
            assert_eq!(changes, total_changes[mode as usize], "mode {mode}");
        }

        // mode 2: x = 4 + 8*cc; forced change (cc32) at x=260, last (cc39) at 316.
        assert_eq!(4 + 8 * 32, 260);
        assert_eq!(4 + 8 * 39, 316);
    }

    #[test]
    fn pixel_index_planes() {
        let mut line = [0u8; 160];
        line[0..2].copy_from_slice(&0xAAAAu16.to_be_bytes());
        line[2..4].copy_from_slice(&0xCCCCu16.to_be_bytes());
        line[4..6].copy_from_slice(&0xF0F0u16.to_be_bytes());
        line[6..8].copy_from_slice(&0xFF00u16.to_be_bytes());
        for p in 0..16 {
            assert_eq!(pixel_index(&line, p), 15 - p);
        }
        assert_eq!(pixel_index(&line, 16), 0);

        let mut line3 = [0u8; 208];
        // group 25 starts at byte 25*8 = 200; plane-0 word 0x8000.
        line3[200..202].copy_from_slice(&0x8000u16.to_be_bytes());
        assert_eq!(pixel_index(&line3, 400), 1);
        assert_eq!(pixel_index(&line3, 401), 0);
    }
}
