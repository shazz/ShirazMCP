//! Wire structs + pure validation/mapping for the P11 audio tools — split
//! from audio.rs for the 200-line budget, same pattern as view_wire.rs. No
//! session/emulator access.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audio::spectrogram::{SpecImage, HOP, N_FFT};
use crate::audio::wav::WavInfo;

/// Bounds for `seconds` (both tools). 0.5 s ≈ 21 STFT frames (enough to see
/// anything); 30 s caps the session-lock hold and the WAV at ~5.3 MB.
pub const SECONDS_MIN: f64 = 0.5;
pub const SECONDS_MAX: f64 = 30.0;

pub(crate) const SILENT_PEAK: f32 = 1e-4;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordAudioArgs {
    /// Real-time seconds to record (0.5 ..= 30). The emulated machine runs
    /// at normal speed for exactly this window; the call blocks for its
    /// duration.
    pub seconds: f64,
    /// Absolute output path ending in ".wav", no whitespace. Default:
    /// "<work_dir>/audio-NNNN.wav".
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SpectrogramArgs {
    /// Record this many real-time seconds first (0.5 ..= 30). Exactly one
    /// of `seconds` / `wav_path` must be given.
    pub seconds: Option<f64>,
    /// Analyze an existing WAV instead of recording (e.g. a previous
    /// record_audio output). Works in any session state, even stopped.
    pub wav_path: Option<String>,
    /// Frequency-axis ceiling in Hz. Default 8000; clamped to
    /// 500 ..= sample_rate/2.
    pub fmax_hz: Option<u32>,
    /// Also render a min-max amplitude waveform PNG. Default false.
    pub include_waveform: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RecordAudioOutput {
    /// On-disk WAV — a path handle, never bytes (ADR-5).
    pub wav_path: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    /// Actual audio duration from the header (≈ requested seconds).
    pub duration_s: f64,
    /// Max |sample| after mono mix, 0..=1.
    pub peak: f32,
    /// peak < 1e-4 — the "is anything playing at all?" fast answer.
    pub silent: bool,
    /// True iff fast-forward was suspended for the window and restored.
    pub fast_forward_restored: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SpectrogramOutput {
    pub wav_path: String,
    pub sample_rate: u32,
    pub duration_s: f64,
    pub n_fft: usize,
    pub hop: usize,
    /// Effective (clamped) ceiling actually rendered.
    pub fmax_hz: u32,
    pub frames: usize,
    pub bins: usize,
    pub width: u32,
    pub height: u32,
    /// Time axis: image columns per second = rate/hop * scale_x.
    pub px_per_second: f64,
    /// Frequency axis: Hz per image row = (rate/n_fft) / scale_y. Top row =
    /// fmax_hz, bottom row = 0 Hz.
    pub hz_per_row: f64,
    pub peak: f32,
    pub silent: bool,
    /// Saved PNGs (the spectrogram is also returned inline).
    pub spectrogram_png: String,
    pub waveform_png: Option<String>,
}

/// seconds in `SECONDS_MIN..=SECONDS_MAX` (NaN fails the range test and is
/// therefore rejected too).
pub(crate) fn validate_seconds(s: f64) -> Result<f64, String> {
    if s.is_nan() || !(SECONDS_MIN..=SECONDS_MAX).contains(&s) {
        return Err(format!(
            "seconds must be {SECONDS_MIN}..={SECONDS_MAX}, got {s}"
        ));
    }
    Ok(s)
}

/// Output-path rule (the string goes over the control fifo, which splits on
/// whitespace — §2.2): absolute, ends with ".wav" (ASCII, case-sensitive),
/// contains no whitespace/control chars.
pub(crate) fn validate_wav_out(path: &str) -> Result<PathBuf, String> {
    if !path.starts_with('/') {
        return Err(format!("wav path must be absolute: {path:?}"));
    }
    if !path.ends_with(".wav") {
        return Err(format!("wav path must end in \".wav\": {path:?}"));
    }
    if path.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!(
            "wav path must not contain whitespace/control characters: {path:?}"
        ));
    }
    Ok(PathBuf::from(path))
}

/// Exactly one of seconds/wav_path.
pub(crate) enum SpecSource {
    Record(f64),
    Wav(PathBuf),
}

pub(crate) fn validate_source(a: &SpectrogramArgs) -> Result<SpecSource, String> {
    match (a.seconds, a.wav_path.as_deref()) {
        (Some(s), None) => Ok(SpecSource::Record(validate_seconds(s)?)),
        (None, Some(p)) => {
            let path = PathBuf::from(p);
            if !path.is_absolute() {
                return Err(format!("wav_path must be absolute: {p:?}"));
            }
            Ok(SpecSource::Wav(path))
        }
        (Some(_), Some(_)) => Err("give exactly one of seconds/wav_path, not both".to_string()),
        (None, None) => Err("give exactly one of seconds/wav_path".to_string()),
    }
}

/// `SpecImage` + wav facts + paths → `SpectrogramOutput` (pure arithmetic).
pub(crate) fn build_spec_output(
    wav_path: &std::path::Path,
    info: &WavInfo,
    spec: &SpecImage,
    spectrogram_png: String,
    waveform_png: Option<String>,
) -> SpectrogramOutput {
    let px_per_second = f64::from(info.sample_rate) / HOP as f64 * f64::from(spec.scale_x);
    let hz_per_row = f64::from(info.sample_rate) / N_FFT as f64 / f64::from(spec.scale_y);
    SpectrogramOutput {
        wav_path: wav_path.display().to_string(),
        sample_rate: info.sample_rate,
        duration_s: info.duration_s,
        n_fft: N_FFT,
        hop: HOP,
        fmax_hz: spec.fmax_hz,
        frames: spec.frames,
        bins: spec.bins,
        width: spec.width,
        height: spec.height,
        px_per_second,
        hz_per_row,
        peak: info.peak,
        silent: info.peak < SILENT_PEAK,
        spectrogram_png,
        waveform_png,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_bounds() {
        assert_eq!(validate_seconds(0.5), Ok(0.5));
        assert_eq!(validate_seconds(30.0), Ok(30.0));
        assert!(validate_seconds(0.49).is_err());
        assert!(validate_seconds(30.01).is_err());
        assert!(validate_seconds(f64::NAN).is_err());
    }

    #[test]
    fn wav_out_rules() {
        assert!(validate_wav_out("/tmp/x.wav").is_ok());
        assert!(validate_wav_out("relative.wav").is_err());
        assert!(validate_wav_out("/tmp/x.ym").is_err());
        assert!(validate_wav_out("/tmp/has space.wav").is_err());
        assert!(validate_wav_out("/tmp/x.WAV").is_err());
        assert!(validate_wav_out("/tmp/a\nb.wav").is_err());
    }

    fn args(seconds: Option<f64>, wav_path: Option<&str>) -> SpectrogramArgs {
        SpectrogramArgs {
            seconds,
            wav_path: wav_path.map(str::to_string),
            fmax_hz: None,
            include_waveform: None,
        }
    }

    #[test]
    fn source_xor() {
        assert!(matches!(
            validate_source(&args(Some(2.0), None)),
            Ok(SpecSource::Record(s)) if s == 2.0
        ));
        assert!(matches!(
            validate_source(&args(None, Some("/tmp/x.wav"))),
            Ok(SpecSource::Wav(_))
        ));
        assert!(validate_source(&args(Some(2.0), Some("/tmp/x.wav"))).is_err());
        assert!(validate_source(&args(None, None)).is_err());
        assert!(validate_source(&args(None, Some("rel.wav"))).is_err());
    }

    #[test]
    fn spec_output_axis_math() {
        let wav_info = WavInfo {
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
            duration_s: 1.0,
            peak: 0.5,
        };
        let spec = SpecImage {
            png: vec![],
            width: 664,
            height: 372,
            frames: 83,
            bins: 372,
            scale_x: 8,
            scale_y: 1,
            fmax_hz: 8000,
        };
        let out = build_spec_output(
            std::path::Path::new("/tmp/x.wav"),
            &wav_info,
            &spec,
            "spec.png".to_string(),
            None,
        );
        assert!((out.px_per_second - 44100.0 / 512.0 * 8.0).abs() < 1e-6);
        assert!((out.hz_per_row - 44100.0 / 2048.0).abs() < 1e-6);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"n_fft\":2048"));
    }

    #[test]
    fn args_defaults() {
        let a: SpectrogramArgs = serde_json::from_str(r#"{"seconds":2}"#).unwrap();
        assert_eq!(a.fmax_hz, None); // documented default 8000 applied by the tool
        assert_eq!(a.include_waveform, None); // documented default false applied by the tool
    }
}
