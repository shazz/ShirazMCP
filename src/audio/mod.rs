//! Pure audio DSP for the P11 tools: WAV decode, STFT spectrogram, waveform
//! render. Emulator-free by construction (ADR-23) — this module must never
//! import session/tools/debug/ripper.

pub mod magma;
pub mod spectrogram;
pub mod wav;
pub mod waveform;

/// Mono audio in [-1, 1] at `rate` Hz — the unit every DSP fn consumes.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuf {
    pub samples: Vec<f32>,
    pub rate: u32,
}

/// Encode an RGB image as PNG bytes. Local twin of
/// `ripper::image::render::png_bytes` (which takes Rgba) — duplicated
/// rather than imported so `audio` stays a leaf with no `ripper` edge.
pub(crate) fn png_encode(img: &image::RgbImage) -> crate::error::Result<Vec<u8>> {
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| crate::error::Error::Parse {
            context: "png encode".to_string(),
            input: e.to_string(),
        })?;
    Ok(buf)
}
