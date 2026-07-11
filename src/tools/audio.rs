//! P11 audio tools: `record_audio` (Hatari native WAV recorder, real-time
//! window — `session::record`) and `spectrogram` (pure DSP over the WAV —
//! `crate::audio`). Thin adapters; the wire layer is `audio_wire.rs`, the
//! artifact/content-block assembly is `audio_artifacts.rs`.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::audio_wire::{
    validate_seconds, validate_source, validate_wav_out, SpecSource, SILENT_PEAK,
};
use super::{to_mcp, wrong_state, HatariServer};
use crate::audio::spectrogram::FMAX_DEFAULT_HZ;
use crate::session::AnySession;

// `audio_wire` is `pub(crate)` (same as rip_wire/view_wire/machine_wire), but
// these are the input/output types of `pub` tool fns and the live
// integration test (tests/audio_live.rs, an external crate) needs to name
// them (construct args, read output fields) — same re-export pattern as
// `machine_state.rs`'s `MachineStateArgs`.
pub use super::audio_wire::{RecordAudioArgs, RecordAudioOutput, SpectrogramArgs};

const MAX_WAV_BYTES: u64 = 64 * 1024 * 1024;

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = audio_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "record_audio",
        description = "Record the emulated Atari ST's sound output to a \
WAV file using Hatari's native recorder. Fast-forward is suspended so the \
capture is real-time: recording N seconds blocks for N real seconds (max \
30). Returns a path handle plus format facts and a peak/silent verdict — \
never audio bytes. Requires a running (not paused) session. Use \
spectrogram to see the recording."
    )]
    pub async fn record_audio(
        &self,
        Parameters(a): Parameters<RecordAudioArgs>,
    ) -> Result<Json<RecordAudioOutput>, McpError> {
        let (wav_path, restored) = self.record_to(a.seconds, a.path.as_deref()).await?;
        let (buf, info) = crate::audio::wav::read_mono(&wav_path).map_err(to_mcp)?;
        drop(buf);
        Ok(Json(RecordAudioOutput {
            wav_path: wav_path.display().to_string(),
            sample_rate: info.sample_rate,
            channels: info.channels,
            bits_per_sample: info.bits_per_sample,
            duration_s: info.duration_s,
            peak: info.peak,
            silent: info.peak < SILENT_PEAK,
            fast_forward_restored: restored,
        }))
    }

    #[tool(
        name = "spectrogram",
        description = "See the music: STFT spectrogram of the ST's sound \
output, returned inline as a PNG (magma colormap; time left-to-right, \
0 Hz at the bottom, fmax_hz at the top, 80 dB color range) plus a JSON \
with the axis mapping (px_per_second, hz_per_row) and a peak/silent \
verdict. Give seconds (0.5-30) to record first — real-time, blocks for \
that long, requires a running session — or wav_path to analyze an \
existing recording (works in any session state). include_waveform=true \
adds an amplitude-envelope PNG. Ideal for before/after comparison around \
a patch to a sound routine."
    )]
    pub async fn spectrogram(
        &self,
        Parameters(a): Parameters<SpectrogramArgs>,
    ) -> Result<CallToolResult, McpError> {
        let source = validate_source(&a).map_err(|m| McpError::invalid_params(m, None))?;
        let include_waveform = a.include_waveform.unwrap_or(false);
        let wav_path = self.resolve_wav(source).await?;
        let (buf, info) = crate::audio::wav::read_mono(&wav_path).map_err(to_mcp)?;
        let fmax = a.fmax_hz.unwrap_or(FMAX_DEFAULT_HZ);
        let spec = crate::audio::spectrogram::render(&buf, fmax).map_err(to_mcp)?;
        self.compose_spectrogram(wav_path, info, spec, &buf, include_waveform)
            .await
    }
}

impl HatariServer {
    /// Validate + resolve the path, then drive `session::record::record_wav`
    /// under the session lock. Requires Running (a frozen CPU produces no
    /// audio and does not service the fifo). Lock is held for the whole
    /// window — deliberate, §3.2/ADR-23.
    async fn record_to(
        &self,
        seconds: f64,
        path_arg: Option<&str>,
    ) -> Result<(PathBuf, bool), McpError> {
        let seconds = validate_seconds(seconds).map_err(|m| McpError::invalid_params(m, None))?;
        let mut guard = self.session.lock().await;
        let wav_path = match path_arg {
            Some(p) => {
                let path = validate_wav_out(p).map_err(|m| McpError::invalid_params(m, None))?;
                check_parent_dir(&path).await?;
                path
            }
            None => {
                let n = self.audio_seq.fetch_add(1, Ordering::Relaxed) + 1;
                guard.config().work_dir.join(format!("audio-{n:04}.wav"))
            }
        };
        let proc = match &mut *guard {
            AnySession::Running(s) => s.process_mut(),
            other => return Err(wrong_state(other.state_name(), "running")),
        };
        let restored = crate::session::record::record_wav(proc, seconds, &wav_path)
            .await
            .map_err(to_mcp)?;
        Ok((wav_path, restored))
    }

    /// `SpecSource::Record` drives the emulator (via `record_to`);
    /// `SpecSource::Wav` only stats the file — no session access, works in
    /// any state including Stopped.
    async fn resolve_wav(&self, source: SpecSource) -> Result<PathBuf, McpError> {
        match source {
            SpecSource::Record(s) => Ok(self.record_to(s, None).await?.0),
            SpecSource::Wav(p) => {
                let meta = tokio::fs::metadata(&p).await.map_err(|_| {
                    McpError::invalid_params(
                        format!("wav_path does not exist: {}", p.display()),
                        None,
                    )
                })?;
                if meta.len() > MAX_WAV_BYTES {
                    return Err(McpError::invalid_params(
                        "wav too large (> 64 MiB)".to_string(),
                        None,
                    ));
                }
                Ok(p)
            }
        }
    }
}

async fn check_parent_dir(path: &Path) -> Result<(), McpError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    tokio::fs::metadata(parent).await.map_err(|_| {
        McpError::invalid_params(
            format!("parent directory does not exist: {}", parent.display()),
            None,
        )
    })?;
    Ok(())
}
