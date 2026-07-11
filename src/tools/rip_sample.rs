//! P20 rip_sample tool: momentary-freeze read of the STE DMA sound
//! registers ($FF8900 block) + ranged savebin of [frame_start, frame_end) +
//! signed-8→PCM16 WAV (audio::wav). Choreography-not-capability (the
//! machine_state pattern, ADR-20); the wire layer is rip_sample_wire.rs.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::rip_sample_wire;
use super::{audio_wire, to_mcp, wrong_state, HatariServer};
use crate::analyze::dmasnd::{self, DmaSndState, DMASND_BLOCK_ADDR, DMASND_BLOCK_LEN};
use crate::debug::{self, DebugChannel};
use crate::session::{memory, AnySession, HatariProcess, Running, Session};
use crate::types::{Addr, MemImage};

// `rip_sample_wire` is `pub(crate)` (same as audio_wire/machine_wire), but
// `RipSampleArgs`/`RipSampleOutput` are the input/output types of this `pub`
// tool fn and the live integration test (tests/rip_sample_live.rs, an
// external crate) needs to name them — same re-export pattern as
// `machine_state.rs`'s `MachineStateArgs` / `audio.rs`'s `RecordAudioArgs`.
pub use super::rip_sample_wire::{RipSampleArgs, RipSampleOutput};

/// Everything read under the freeze. `pcm` is Some iff plan.extract fired.
pub(crate) struct RawRip {
    pub(crate) state: DmaSndState,
    pub(crate) plan: rip_sample_wire::RipPlan,
    pub(crate) pcm: Option<MemImage>,
}

/// Testability seam (machine_state::read_video precedent): one `m $ff8900
/// 34` through the sentinel-synced channel, decoded pure.
async fn read_dma_block(chan: &mut impl DebugChannel) -> crate::error::Result<DmaSndState> {
    let block = debug::read_bytes(chan, Addr::new(DMASND_BLOCK_ADDR), DMASND_BLOCK_LEN).await?;
    dmasnd::decode(&block)
}

async fn gather(proc: &mut HatariProcess, force: bool) -> crate::error::Result<RawRip> {
    let state = read_dma_block(proc).await?;
    let plan = rip_sample_wire::plan(
        &proc.config().machine,
        &state,
        proc.config().ram_len(),
        force,
    );
    let pcm = match plan.extract {
        Some((start, len)) => Some(memory::acquire_range(proc, Addr::new(start), len).await?),
        None => None,
    };
    Ok(RawRip { state, plan, pcm })
}

/// Momentary freeze: pause -> gather -> resume (ADR-20: gather error wins).
async fn freeze_gather_thaw(
    sess: Session<Running>,
    force: bool,
) -> (AnySession, Result<RawRip, McpError>) {
    let mut paused = match sess.pause().await {
        Ok(p) => p,
        Err((s, e)) => return (AnySession::Running(s), Err(to_mcp(e))),
    };
    let raw = gather(paused.process_mut(), force).await.map_err(to_mcp);
    match paused.resume().await {
        Ok(r) => (AnySession::Running(r), raw),
        Err((p, e)) => (AnySession::Paused(p), raw.and(Err(to_mcp(e)))),
    }
}

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = rip_sample_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "rip_sample",
        description = "Rip the 8-bit PCM sample the STE DMA sound engine is \
playing: reads the $FF8900 frame registers (which point directly at the \
buffer in RAM), dumps [frame_start, frame_end), and writes it as a 16-bit \
WAV — returns the path plus rate (6258/12517/25033/50066 Hz), mono/stereo, \
length and a peak/silent verdict, never audio bytes. A running session is \
momentarily frozen so registers and buffer come from one instant; paused \
sessions are read in place. If DMA sound is off or the machine is not an \
STE, returns status \"none\"/\"stopped\" with a reason (not an error); \
force=true rips a valid-but-stopped buffer. Covers what identify_music/\
ym_state cannot: DMA digi-music is not the YM chip. Compose with \
spectrogram(wav_path=…) to see the sample."
    )]
    pub async fn rip_sample(
        &self,
        Parameters(a): Parameters<RipSampleArgs>,
    ) -> Result<Json<RipSampleOutput>, McpError> {
        let force = a.force.unwrap_or(false);
        let (work_dir, session, momentary, raw) = self.take_rip(force).await?;
        let raw = raw?;
        let wav = match &raw.pcm {
            Some(img) => Some(
                self.write_sample(img.bytes(), &raw.state, a.out_path.as_deref(), &work_dir)
                    .await?,
            ),
            None => None,
        };
        Ok(Json(rip_sample_wire::build_output(
            session, momentary, &raw.state, &raw.plan, wav,
        )))
    }
}

type RipTaken = (PathBuf, &'static str, bool, Result<RawRip, McpError>);

impl HatariServer {
    /// Lock held across the whole choreography (see `grab_screen`, view.rs).
    async fn take_rip(&self, force: bool) -> Result<RipTaken, McpError> {
        let mut guard = self.session.lock().await;
        let config = guard.config().clone();
        let work_dir = config.work_dir.clone();
        let (back, session, momentary, raw) =
            match std::mem::replace(&mut *guard, AnySession::new(config)) {
                AnySession::Stopped(s) => {
                    let name = s.state_name();
                    *guard = AnySession::Stopped(s);
                    return Err(wrong_state(name, "running or paused"));
                }
                AnySession::Paused(mut s) => {
                    let raw = gather(s.process_mut(), force).await.map_err(to_mcp);
                    (AnySession::Paused(s), "paused", false, raw)
                }
                AnySession::Running(s) => {
                    let (back, raw) = freeze_gather_thaw(s, force).await;
                    (back, "running", true, raw)
                }
            };
        *guard = back;
        Ok((work_dir, session, momentary, raw))
    }
}

impl HatariServer {
    /// Resolve the out path (validate_wav_out, or work_dir/sample-NNNN.wav
    /// from audio_seq), write via audio::wav::write_pcm8, read the facts
    /// back with wav::read_mono (record_audio precedent — the read-back IS
    /// the write verification). Sync hound I/O on a ≤4 MB buffer is
    /// milliseconds (the Capstone/read_mono precedent); the session lock is
    /// already released.
    async fn write_sample(
        &self,
        pcm: &[u8],
        state: &DmaSndState,
        out_path: Option<&str>,
        work_dir: &Path,
    ) -> Result<rip_sample_wire::WavFacts, McpError> {
        let path = match out_path {
            Some(p) => {
                let path = audio_wire::validate_wav_out(p)
                    .map_err(|m| McpError::invalid_params(m, None))?;
                check_parent_dir(&path).await?;
                path
            }
            None => {
                let n = self.audio_seq.fetch_add(1, Ordering::Relaxed) + 1;
                work_dir.join(format!("sample-{n:04}.wav"))
            }
        };
        crate::audio::wav::write_pcm8(&path, pcm, state.channels(), state.rate_hz)
            .map_err(to_mcp)?;
        let (_, info) = crate::audio::wav::read_mono(&path).map_err(to_mcp)?;
        Ok(rip_sample_wire::WavFacts {
            path: path.display().to_string(),
            peak: info.peak,
            silent: info.peak < audio_wire::SILENT_PEAK,
        })
    }
}

/// P11 §10.1 rule: a user-supplied path's parent must already exist.
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        responses: Vec<String>,
        sent: Vec<String>,
    }

    impl DebugChannel for Fake {
        async fn debug(&mut self, cmd: &str) -> crate::error::Result<String> {
            self.sent.push(cmd.to_string());
            Ok(self.responses.remove(0))
        }
    }

    /// The §11.1 pinned block as a realistic `m` dump (three 16-byte lines).
    fn fixture_dump() -> String {
        "00FF8900: 00 03 00 03 00 00 00 01 00 03 00 40 00 00 00 03   ................\n\
00FF8910: 00 A0 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................\n\
00FF8920: 00 81 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................"
            .to_string()
    }

    #[tokio::test]
    async fn read_dma_block_command_and_decode() {
        let mut fake = Fake {
            responses: vec![fixture_dump()],
            sent: vec![],
        };
        let s = read_dma_block(&mut fake).await.unwrap();
        assert_eq!(fake.sent, vec!["m $ff8900 34".to_string()]);
        assert!(s.play);
        assert!(s.loop_on);
        assert_eq!(s.frame_start, 0x030000);
        assert_eq!(s.frame_counter, 0x034000);
        assert_eq!(s.frame_end, 0x03A000);
        assert!(s.mono);
        assert_eq!(s.rate_hz, 12517);
    }
}
