//! STFT spectrogram: Hann window → rustfft → dB → magma LUT → PNG.
//! Parameters pinned to the trusted STEIsNotDead pipeline (ADR-23 / P11
//! §7.4) — do not retune without updating the ADR.

use rustfft::num_complex::Complex;

use crate::error::{Error, Result};

use super::{magma, png_encode, AudioBuf};

/// STFT parameters matching STEIsNotDead eyes.py's specgram call
/// (NFFT=2048, noverlap=1536 ⇒ hop 512, Hanning) — ADR-23.
pub const N_FFT: usize = 2048;
pub const HOP: usize = 512;
/// Pinned dB color range: `[max-80, max]` → LUT index 0..=255.
pub const DB_RANGE: f32 = 80.0;
/// Default display ceiling (spectrize.py convention).
pub const FMAX_DEFAULT_HZ: u32 = 8_000;
/// Minimum output raster before integer upscaling (§7.4 scaling rule).
const MIN_W: usize = 640;
const MIN_H: usize = 320;

/// The rendered spectrogram + the axis facts the wire layer reports.
#[derive(Debug)]
pub struct SpecImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub frames: usize,
    pub bins: usize,
    pub scale_x: u32,
    pub scale_y: u32,
    /// Effective (clamped) ceiling actually rendered.
    pub fmax_hz: u32,
}

/// Symmetric Hann, numpy `np.hanning(N)` exactly:
/// `w[i] = 0.5 - 0.5*cos(2*pi*i / (N-1))`.
fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32;
            0.5 - 0.5 * phase.cos()
        })
        .collect()
}

/// Magnitude STFT: frames x (N_FFT/2 + 1). Frame f covers samples
/// `[f*HOP, f*HOP + N_FFT)`; frame count = `(len - N_FFT)/HOP + 1`.
fn stft_mag(samples: &[f32]) -> Result<Vec<Vec<f32>>> {
    if samples.len() < N_FFT {
        return Err(Error::Parse {
            context: "stft input".to_string(),
            input: format!("{} samples, need >= {N_FFT}", samples.len()),
        });
    }
    let win = hann(N_FFT);
    let mut planner = rustfft::FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);
    let frames = (samples.len() - N_FFT) / HOP + 1;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        out.push(one_frame(&fft, &samples[f * HOP..], &win));
    }
    Ok(out)
}

fn one_frame(fft: &std::sync::Arc<dyn rustfft::Fft<f32>>, tail: &[f32], win: &[f32]) -> Vec<f32> {
    let mut buf: Vec<Complex<f32>> = (0..N_FFT)
        .map(|i| Complex {
            re: tail[i] * win[i],
            im: 0.0,
        })
        .collect();
    fft.process(&mut buf);
    buf[..=N_FFT / 2]
        .iter()
        .map(|c| (c.re * c.re + c.im * c.im).sqrt())
        .collect()
}

/// mag relative to `max` → LUT index. 0 dB → 255, ≤ -80 dB → 0, linear
/// between. `max <= 0` (silence) → 0.
fn db_index(mag: f32, max: f32) -> u8 {
    if max <= 0.0 {
        return 0;
    }
    let db = (20.0 * (mag / max + 1e-12).log10()).clamp(-DB_RANGE, 0.0);
    ((db + DB_RANGE) / DB_RANGE * 255.0).round() as u8
}

/// Bins kept for a given `fmax_hz` (already clamped), rows 0..=fmax.
fn keep_bins(fmax_hz: u32, rate: u32) -> usize {
    let full = N_FFT / 2 + 1;
    let scaled = (u64::from(fmax_hz) * N_FFT as u64 / u64::from(rate)) as usize + 1;
    full.min(scaled)
}

/// The whole pipeline. `fmax_hz` is clamped to `500 ..= rate/2` before use.
pub fn render(audio: &AudioBuf, fmax_hz: u32) -> Result<SpecImage> {
    let fmax = fmax_hz.max(500).min(audio.rate / 2);
    let mag = stft_mag(&audio.samples)?;
    let bins = keep_bins(fmax, audio.rate);
    let max = mag
        .iter()
        .flat_map(|f| f[..bins].iter().copied())
        .fold(0f32, f32::max);
    let frames = mag.len();
    let scale_x = MIN_W.div_ceil(frames).max(1);
    let scale_y = MIN_H.div_ceil(bins).max(1);
    let png = png_encode(&paint(&mag, bins, max, scale_x, scale_y))?;
    Ok(SpecImage {
        png,
        width: (frames * scale_x) as u32,
        height: (bins * scale_y) as u32,
        frames,
        bins,
        scale_x: scale_x as u32,
        scale_y: scale_y as u32,
        fmax_hz: fmax,
    })
}

/// Nearest-neighbor upscale + paint. Column x = frame x (time left→right);
/// row y = bin `bins - 1 - (y / scale_y)` (0 Hz at the bottom, fmax at top).
fn paint(
    mag: &[Vec<f32>],
    bins: usize,
    max: f32,
    scale_x: usize,
    scale_y: usize,
) -> image::RgbImage {
    let frames = mag.len();
    let (width, height) = (frames * scale_x, bins * scale_y);
    let mut img = image::RgbImage::new(width as u32, height as u32);
    for y in 0..height {
        let bin = bins - 1 - (y / scale_y);
        for x in 0..width {
            let frame = (x / scale_x).min(frames - 1);
            let idx = db_index(mag[frame][bin], max);
            img.put_pixel(x as u32, y as u32, image::Rgb(magma::MAGMA[idx as usize]));
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Amplitude 0.5, frequency exactly `k * rate / N_FFT` Hz (bin-centered).
    fn sine(rate: u32, k: usize, secs: f64) -> AudioBuf {
        let n = (rate as f64 * secs) as usize;
        let freq = k as f64 * rate as f64 / N_FFT as f64;
        let samples = (0..n)
            .map(|i| {
                (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin()) as f32
            })
            .collect();
        AudioBuf { samples, rate }
    }

    #[test]
    fn hann_shape() {
        let w = hann(2048);
        assert_eq!(w[0], 0.0);
        assert!(w[2047].abs() < 1e-5);
        assert!(w[1023] > 0.999);
        assert!((w[1023] - w[1024]).abs() < 1e-5);
    }

    #[test]
    fn stft_frame_count() {
        assert_eq!(stft_mag(&vec![0.0; 44100]).unwrap().len(), 83);
        for f in &stft_mag(&vec![0.0; 44100]).unwrap() {
            assert_eq!(f.len(), 1025);
        }
        assert_eq!(stft_mag(&vec![0.0; 132_153]).unwrap().len(), 255);
    }

    #[test]
    fn stft_rejects_short() {
        let err = stft_mag(&vec![0.0; 2047]).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "stft input"));
    }

    #[test]
    fn stft_sine_peak_bin() {
        let audio = sine(44100, 48, 1.0);
        let mag = stft_mag(&audio.samples).unwrap();
        let frame0 = &mag[0];
        let (argmax, &peak) = frame0
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert_eq!(argmax, 48);
        let db = |b: usize| 20.0 * (frame0[b] / peak).log10();
        assert!((-7.0..=-5.0).contains(&db(47)));
        assert!((-7.0..=-5.0).contains(&db(49)));
        assert!(db(46) < -60.0);
        assert!(db(50) < -60.0);
    }

    #[test]
    fn db_index_pins() {
        assert_eq!(db_index(1.0, 1.0), 255);
        assert_eq!(db_index(0.1, 1.0), 191);
        assert_eq!(db_index(1e-5, 1.0), 0);
        assert_eq!(db_index(0.5, 0.0), 0);
    }

    #[test]
    fn render_dims_and_scale() {
        let spec = render(&sine(44100, 48, 1.0), 8000).unwrap();
        assert_eq!(spec.frames, 83);
        assert_eq!(spec.bins, 372);
        assert_eq!(spec.scale_x, 8);
        assert_eq!(spec.scale_y, 1);
        assert_eq!(spec.width, 664);
        assert_eq!(spec.height, 372);
        assert_eq!(spec.fmax_hz, 8000);
        assert_eq!(
            &spec.png[..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        );
    }

    #[test]
    fn render_sine_row_is_hot() {
        let spec = render(&sine(44100, 48, 1.0), 8000).unwrap();
        let decoded = image::load_from_memory(&spec.png).unwrap().to_rgb8();
        assert_eq!(
            decoded.get_pixel(0, spec.height - 1 - 48),
            &image::Rgb([252, 253, 191])
        );
        assert_eq!(decoded.get_pixel(0, 0), &image::Rgb([0, 0, 4]));
    }

    #[test]
    fn render_clamps_fmax() {
        let spec = render(&sine(44100, 48, 1.0), 99_000).unwrap();
        assert_eq!(spec.fmax_hz, 22050);
        assert_eq!(spec.bins, 1025);
    }

    #[test]
    fn render_silence() {
        let audio = AudioBuf {
            samples: vec![0.0; 44100],
            rate: 44100,
        };
        let spec = render(&audio, 8000).unwrap();
        let decoded = image::load_from_memory(&spec.png).unwrap().to_rgb8();
        assert!(decoded.pixels().all(|p| *p == image::Rgb([0, 0, 4])));
    }
}
