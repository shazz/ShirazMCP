//! Amplitude-envelope PNG: matches the *purpose* (not the matplotlib
//! pixels) of `eyes.py waveform_png` — min-max envelope on white, same
//! blue accent color.

use crate::error::{Error, Result};

use super::png_encode;

pub const WAVE_W: u32 = 1200;
pub const WAVE_H: u32 = 300;
const WAVE_RGB: [u8; 3] = [0x22, 0x88, 0xCC]; // eyes.py "#2288cc"

/// Min-max envelope render, `WAVE_W x WAVE_H`, white background. Column c
/// covers samples `[c*n/W, (c+1)*n/W)` (at least one sample: short input
/// repeats columns).
pub fn render(samples: &[f32]) -> Result<Vec<u8>> {
    if samples.is_empty() {
        return Err(Error::Parse {
            context: "waveform input".to_string(),
            input: "0 samples".to_string(),
        });
    }
    let mut img = image::RgbImage::from_pixel(WAVE_W, WAVE_H, image::Rgb([255, 255, 255]));
    let n = samples.len();
    for c in 0..WAVE_W as usize {
        let (lo, hi) = column_extremes(samples, c, n);
        paint_column(&mut img, c as u32, lo, hi);
    }
    png_encode(&img)
}

/// Column sample range, exactly `start = c*n / W`, `end = max((c+1)*n / W,
/// start + 1)` (usize math) — every column non-empty for any `n >= 1`.
fn column_extremes(samples: &[f32], c: usize, n: usize) -> (f32, f32) {
    let w = WAVE_W as usize;
    let start = c * n / w;
    let end = ((c + 1) * n / w).max(start + 1);
    let slice = &samples[start..end];
    let lo = slice.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = slice.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    (lo, hi)
}

/// `y(v) = round((1 - clamp(v,-1,1)) * 0.5 * (H-1))`.
fn y_of(v: f32) -> u32 {
    let clamped = v.clamp(-1.0, 1.0);
    ((1.0 - clamped) * 0.5 * (WAVE_H - 1) as f32).round() as u32
}

fn paint_column(img: &mut image::RgbImage, x: u32, lo: f32, hi: f32) {
    let (top, bottom) = (y_of(hi), y_of(lo)); // higher amplitude -> smaller y
    for y in top..=bottom {
        img.put_pixel(x, y, image::Rgb(WAVE_RGB));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_dims_and_extremes() {
        let png = render(&vec![1.0; 4800]).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (WAVE_W, WAVE_H));
        assert_eq!(decoded.get_pixel(0, 0), &image::Rgb(WAVE_RGB));
        assert_eq!(decoded.get_pixel(0, 299), &image::Rgb([255, 255, 255]));
    }

    #[test]
    fn waveform_rejects_empty() {
        let err = render(&[]).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "waveform input"));
    }
}
