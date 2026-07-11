//! Artifact materialization for `spectrogram` (P11): writes the PNG(s),
//! builds the wire output, and assembles the MCP content blocks. Split from
//! audio.rs for the 200-line budget — same grouping as machine_artifacts.rs
//! (P10.md §7.5 precedent).

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use base64::Engine;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::ErrorData as McpError;

use super::audio_wire::{build_spec_output, SpectrogramOutput};
use super::{to_mcp, HatariServer};
use crate::audio::spectrogram::SpecImage;
use crate::audio::wav::WavInfo;
use crate::audio::AudioBuf;
use crate::error::Error;

impl HatariServer {
    /// Write the spectrogram (+ optional waveform) artifacts and assemble
    /// the `[image, image?, text]` result. `work_dir` is read fresh from the
    /// session config — valid in any state (ADR-3's `AnySession::config`).
    pub(crate) async fn compose_spectrogram(
        &self,
        wav_path: PathBuf,
        info: WavInfo,
        spec: SpecImage,
        buf: &AudioBuf,
        include_waveform: bool,
    ) -> Result<CallToolResult, McpError> {
        let work_dir = self.session.lock().await.config().work_dir.clone();
        let n = self.audio_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let wave_png = include_waveform
            .then(|| crate::audio::waveform::render(&buf.samples))
            .transpose()
            .map_err(to_mcp)?;
        let (spec_path, wave_path) =
            write_artifacts(&work_dir, n, &spec.png, wave_png.as_deref()).await?;
        let out = build_spec_output(&wav_path, &info, &spec, spec_path, wave_path);
        spec_result(&spec.png, wave_png.as_deref(), &out)
    }
}

/// Writes the spectrogram PNG (always) and the waveform PNG (iff present);
/// returns their string paths for `SpectrogramOutput`.
async fn write_artifacts(
    work_dir: &Path,
    n: u64,
    spec_png: &[u8],
    wave_png: Option<&[u8]>,
) -> Result<(String, Option<String>), McpError> {
    let spec_path = work_dir.join(format!("spectrogram-{n:04}.png"));
    write_bytes(&spec_path, spec_png).await?;
    let wave_path = match wave_png {
        Some(bytes) => {
            let p = work_dir.join(format!("waveform-{n:04}.png"));
            write_bytes(&p, bytes).await?;
            Some(p.display().to_string())
        }
        None => None,
    };
    Ok((spec_path.display().to_string(), wave_path))
}

async fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), McpError> {
    tokio::fs::write(path, bytes)
        .await
        .map_err(|e| to_mcp(Error::Io(e)))
}

fn spec_result(
    spec_png: &[u8],
    wave_png: Option<&[u8]>,
    out: &SpectrogramOutput,
) -> Result<CallToolResult, McpError> {
    let mut blocks = vec![ContentBlock::image(b64(spec_png), "image/png")];
    if let Some(w) = wave_png {
        blocks.push(ContentBlock::image(b64(w), "image/png"));
    }
    let json = serde_json::to_string(out).map_err(|e| {
        McpError::internal_error(format!("serialize spectrogram output: {e}"), None)
    })?;
    blocks.push(ContentBlock::text(json));
    Ok(CallToolResult::success(blocks))
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
