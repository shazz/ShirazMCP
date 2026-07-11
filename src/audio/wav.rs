//! WAV reader (hound): RIFF PCM16 → channel-averaged mono f32 + format
//! facts. Matches Hatari's `soundout` recorder output (P11 §2.4: integer
//! PCM, 16-bit, rate/channels read from the header, never assumed).

use std::path::Path;

use crate::error::{Error, Result};

use super::AudioBuf;

/// Format + level facts read from a WAV (wire-facing, plain data).
#[derive(Debug, Clone, PartialEq)]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub duration_s: f64,
    pub peak: f32,
}

/// Read a PCM WAV → channel-averaged mono f32 in [-1, 1] + facts. Matches
/// STEIsNotDead `wavio.read_wav` semantics (mean of channels, scale by
/// 2^(bits-1)). Only integer PCM 16-bit is accepted — that is what Hatari
/// writes (§2.4); anything else is `Error::Parse`.
pub fn read_mono(path: &Path) -> Result<(AudioBuf, WavInfo)> {
    let mut reader = hound::WavReader::open(path).map_err(|e| Error::Parse {
        context: "wav read".to_string(),
        input: e.to_string(),
    })?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        return Err(Error::Parse {
            context: "wav format".to_string(),
            input: format!("{spec:?}"),
        });
    }
    let raw = collect_samples(&mut reader)?;
    let (samples, peak) = mix_mono(&raw, spec.channels as usize);
    let duration_s = samples.len() as f64 / f64::from(spec.sample_rate);
    let buf = AudioBuf {
        samples,
        rate: spec.sample_rate,
    };
    let info = WavInfo {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        bits_per_sample: spec.bits_per_sample,
        duration_s,
        peak,
    };
    Ok((buf, info))
}

fn collect_samples<R: std::io::Read>(reader: &mut hound::WavReader<R>) -> Result<Vec<i16>> {
    reader
        .samples::<i16>()
        .collect::<std::result::Result<Vec<i16>, _>>()
        .map_err(|e| Error::Parse {
            context: "wav samples".to_string(),
            input: e.to_string(),
        })
}

/// Mono mix: mean of each frame's channels, scaled to [-1, 1]. A trailing
/// partial frame (malformed file) is dropped silently (`chunks_exact`).
/// Returns `(mono, peak)`.
fn mix_mono(raw: &[i16], channels: usize) -> (Vec<f32>, f32) {
    let mut peak = 0f32;
    let samples = raw
        .chunks_exact(channels.max(1))
        .map(|frame| {
            let sum: f32 = frame.iter().map(|&v| f32::from(v) / 32768.0).sum();
            let mixed = sum / frame.len() as f32;
            peak = peak.max(mixed.abs());
            mixed
        })
        .collect();
    (samples, peak)
}

/// Write signed 8-bit PCM as a 16-bit integer PCM WAV: each byte is
/// reinterpreted as i8 and upscaled losslessly (`i16::from(s as i8) * 256`)
/// so the file is readable back by `read_mono` (PCM16-only) and thus by the
/// spectrogram tool (P20 §3.4). `channels` must be 1 or 2; stereo input is
/// STE interleave (even byte = left, odd = right — memory_map.txt), written
/// as L,R frames. A trailing odd byte with channels == 2 is rejected (STE
/// frames are words; the hardware cannot produce one).
pub fn write_pcm8(path: &Path, pcm: &[u8], channels: u16, rate: u32) -> Result<()> {
    if !(channels == 1 || channels == 2) || rate == 0 {
        return Err(Error::Parse {
            context: "wav write format".to_string(),
            input: format!("channels {channels}, rate {rate}"),
        });
    }
    if channels == 2 && !pcm.len().is_multiple_of(2) {
        return Err(Error::Parse {
            context: "wav write frames".to_string(),
            input: format!("{} bytes is odd for stereo", pcm.len()),
        });
    }
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(wav_write_err)?;
    for &b in pcm {
        w.write_sample(i16::from(b as i8) * 256)
            .map_err(wav_write_err)?;
    }
    w.finalize().map_err(wav_write_err)
}

fn wav_write_err(e: hound::Error) -> Error {
    Error::Parse {
        context: "wav write".to_string(),
        input: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("p11-wav-{}-{name}", std::process::id()))
    }

    fn write_wav(path: &Path, channels: u16, sample_rate: u32, samples: &[i16]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for &s in samples {
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn read_mono_stereo_mix() {
        let path = fixture_path("stereo.wav");
        write_wav(
            &path,
            2,
            44100,
            &[0, 0, 16384, 0, -16384, -16384, 32767, 32767],
        );
        let (buf, info) = read_mono(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(info.sample_rate, 44100);
        assert_eq!(info.channels, 2);
        assert_eq!(info.bits_per_sample, 16);
        let expected = [0.0f32, 0.25, -0.5, 0.999_969_5];
        for (got, want) in buf.samples.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-4, "{got} vs {want}");
        }
        assert!((info.peak - 0.999_969_5).abs() < 1e-4);
        assert!((info.duration_s - 4.0 / 44100.0).abs() < 1e-9);
    }

    #[test]
    fn read_mono_mono_passthrough() {
        let path = fixture_path("mono.wav");
        write_wav(&path, 1, 44100, &[0, -32768]);
        let (buf, info) = read_mono(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert!((buf.samples[0] - 0.0).abs() < 1e-4);
        assert!((buf.samples[1] - (-1.0)).abs() < 1e-4);
        assert!((info.peak - 1.0).abs() < 1e-4);
    }

    #[test]
    fn read_mono_rejects_missing() {
        let err = read_mono(Path::new("/nonexistent/p11-missing.wav")).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "wav read"));
    }

    #[test]
    fn read_mono_rejects_float_format() {
        let path = fixture_path("float.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44100,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        writer.write_sample(0.5f32).unwrap();
        writer.finalize().unwrap();

        let err = read_mono(&path).unwrap_err();
        std::fs::remove_file(&path).ok();
        assert!(matches!(err, Error::Parse { context, .. } if context == "wav format"));
    }

    #[test]
    fn write_pcm8_mono_roundtrip() {
        let path = fixture_path("pcm8-mono.wav");
        write_pcm8(&path, &[0x00, 0x7F, 0x80, 0xFF], 1, 12517).unwrap();
        let (buf, info) = read_mono(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(info.sample_rate, 12517);
        assert_eq!(info.channels, 1);
        assert_eq!(info.bits_per_sample, 16);
        let expected = [0.0f32, 0.992_187_5, -1.0, -0.007_812_5];
        for (got, want) in buf.samples.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-6, "{got} vs {want}");
        }
        assert!((info.peak - 1.0).abs() < 1e-6);
    }

    #[test]
    fn write_pcm8_stereo_interleave() {
        let path = fixture_path("pcm8-stereo.wav");
        let pcm = [0x40u8, 0xC0, 0x40, 0xC0, 0x40, 0xC0, 0x40, 0xC0];
        write_pcm8(&path, &pcm, 2, 25033).unwrap();
        let (buf, info) = read_mono(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 25033);
        for &s in &buf.samples {
            assert!(s.abs() < 1e-6, "expected L/R to cancel, got {s}");
        }
        assert!((info.duration_s - 4.0 / 25033.0).abs() < 1e-6);
    }

    #[test]
    fn write_pcm8_rejects_odd_stereo() {
        let path = fixture_path("pcm8-odd.wav");
        let err = write_pcm8(&path, &[0x00, 0x01, 0x02], 2, 12517).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "wav write frames"));
    }

    #[test]
    fn write_pcm8_rejects_bad_format() {
        let path = fixture_path("pcm8-bad.wav");
        let err = write_pcm8(&path, &[0x00], 3, 12517).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "wav write format"));
        let err = write_pcm8(&path, &[0x00], 1, 0).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "wav write format"));
    }
}
