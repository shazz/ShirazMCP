//! Bulk planar → RGBA decode + PNG encode, split out of `image/mod.rs` for
//! the 200-line budget (same pattern as `types/addr.rs`). Re-exported as
//! `crate::ripper::image::{decode_planar, png_bytes}` — no import-path
//! change for callers.

use std::io::Cursor;

use image::RgbaImage;

use super::{Palette, ScreenRes, SCREEN_BYTES};
use crate::error::{Error, Result};

/// Decode `SCREEN_BYTES` of interleaved planes into RGBA. Uses the FIRST
/// 32000 bytes of `data`; Err (Parse) if shorter — callers pass exact slices.
pub fn decode_planar(data: &[u8], res: ScreenRes, pal: &Palette) -> Result<RgbaImage> {
    let Some(data) = data.get(..SCREEN_BYTES) else {
        return Err(Error::Parse {
            context: "planar decode".to_string(),
            input: format!("{} bytes, need {SCREEN_BYTES}", data.len()),
        });
    };
    let bpl = res.bytes_per_line();
    let mut img = RgbaImage::new(res.width(), res.height());
    let mut indices = Vec::with_capacity(res.width() as usize);
    for y in 0..res.height() {
        indices.clear();
        row_indices(&data[y as usize * bpl..][..bpl], res, &mut indices);
        paint_row(&mut img, y, &indices, pal);
    }
    Ok(img)
}

fn paint_row(img: &mut RgbaImage, y: u32, indices: &[u8], pal: &Palette) {
    for (x, &idx) in indices.iter().enumerate() {
        img.put_pixel(x as u32, y, image::Rgba(pal.color(idx)));
    }
}

/// Decode one interleaved scanline to palette indices, appending to `out`.
/// `line` must be exactly `res.bytes_per_line()` bytes — callers slice.
/// The single planar→index bit-math in the codebase; `decode_planar` and
/// `image::score` both build on it.
pub(crate) fn row_indices(line: &[u8], res: ScreenRes, out: &mut Vec<u8>) {
    let planes = res.planes();
    for group in 0..res.width() as usize / 16 {
        let words = group_words(line, group, planes);
        for p in 0..16 {
            let mut idx = 0u8;
            for (pl, w) in words[..planes].iter().enumerate() {
                idx |= (((w >> (15 - p)) & 1) as u8) << pl;
            }
            out.push(idx);
        }
    }
}

fn group_words(line: &[u8], group: usize, planes: usize) -> [u16; 4] {
    let base = group * planes * 2;
    let mut out = [0u16; 4];
    for (pl, w) in out[..planes].iter_mut().enumerate() {
        *w = u16::from_be_bytes([line[base + 2 * pl], line[base + 2 * pl + 1]]);
    }
    out
}

// (Direct indexing above is provably in-bounds: `line.len() == bpl` and
// `(width/16)*planes*2 == bpl` for all three resolutions; no `.get()` noise.)

/// Encode RGBA → PNG bytes in memory (the `image` crate's PNG encoder).
pub fn png_bytes(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| Error::Parse {
            context: "png encode".to_string(),
            input: e.to_string(),
        })?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 16 distinct ST-range (no nibble bit 3 set) palette words, one per
    /// index — used to assert decoded pixel *identity* (not just presence).
    /// Duplicated from `image/mod.rs`'s test fixture (established
    /// convention for tiny test doubles, not a DRY violation).
    fn counting_pal() -> [u16; 16] {
        let mut w = [0u16; 16];
        for (i, slot) in w.iter_mut().enumerate() {
            let r = (i as u16) & 7;
            let g = (i as u16 >> 3) & 7;
            *slot = (r << 8) | (g << 4);
        }
        w
    }

    #[test]
    fn decode_planar_counting_group() {
        let mut data = vec![0u8; SCREEN_BYTES];
        data[0..2].copy_from_slice(&0xAAAAu16.to_be_bytes());
        data[2..4].copy_from_slice(&0xCCCCu16.to_be_bytes());
        data[4..6].copy_from_slice(&0xF0F0u16.to_be_bytes());
        data[6..8].copy_from_slice(&0xFF00u16.to_be_bytes());
        let pal = Palette::from_words(&counting_pal());
        let img = decode_planar(&data, ScreenRes::Low, &pal).unwrap();
        for p in 0..16u32 {
            let expected = pal.color((15 - p) as u8);
            assert_eq!(*img.get_pixel(p, 0), image::Rgba(expected));
        }
        assert_eq!(*img.get_pixel(16, 0), image::Rgba(pal.color(0)));
        assert_eq!(*img.get_pixel(0, 1), image::Rgba(pal.color(0)));
    }

    #[test]
    fn decode_planar_medium_and_high() {
        let pal = Palette::from_words(&counting_pal());
        let mut med = vec![0u8; SCREEN_BYTES];
        med[0..2].copy_from_slice(&0xAAAAu16.to_be_bytes());
        med[2..4].copy_from_slice(&0x0000u16.to_be_bytes());
        let img = decode_planar(&med, ScreenRes::Med, &pal).unwrap();
        for p in 0..4u32 {
            let expected = if p % 2 == 0 { 1 } else { 0 };
            assert_eq!(*img.get_pixel(p, 0), image::Rgba(pal.color(expected)));
        }

        let mut high = vec![0u8; SCREEN_BYTES];
        high[0..2].copy_from_slice(&0x8000u16.to_be_bytes());
        let img = decode_planar(&high, ScreenRes::High, &pal).unwrap();
        assert_eq!(*img.get_pixel(0, 0), image::Rgba(pal.color(1)));
        assert_eq!(*img.get_pixel(1, 0), image::Rgba(pal.color(0)));
    }

    #[test]
    fn decode_planar_short_input_is_parse_err() {
        let pal = Palette::from_words(&counting_pal());
        let data = vec![0u8; SCREEN_BYTES - 1];
        match decode_planar(&data, ScreenRes::Low, &pal) {
            Err(Error::Parse { context, .. }) => assert_eq!(context, "planar decode"),
            other => panic!("expected Parse err, got {other:?}"),
        }
    }

    #[test]
    fn png_bytes_roundtrip() {
        let pal = Palette::from_words(&counting_pal());
        let data = vec![0u8; SCREEN_BYTES];
        let img = decode_planar(&data, ScreenRes::Low, &pal).unwrap();
        let png = png_bytes(&img).unwrap();
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!(decoded.width(), 320);
        assert_eq!(decoded.height(), 200);
        assert_eq!(decoded.to_rgba8().get_pixel(0, 0).0, pal.color(0));
    }
}
