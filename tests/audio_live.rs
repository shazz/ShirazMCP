//! `record_audio`/`spectrogram` end-to-end against a real Hatari session
//! (P11.md §11.6). `#[ignore]`d — needs `DISPLAY`, `/usr/bin/hatari`, and
//! the MJJ tree. Run:
//! `DISPLAY=:0 cargo test --test audio_live -- --ignored --test-threads=1`.
//!
//! Drives the TOOL layer (`HatariServer`'s public methods) end-to-end,
//! same precedent as `tests/machine_state_live.rs`.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/machine_state_live.rs for the same rationale) — a panic on
// unexpected shape/timeout against a real Hatari session is the correct
// failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::collections::HashSet;
use std::process::Command;

use shiraz_mcp::session::SessionConfig;
use shiraz_mcp::tools::audio::{RecordAudioArgs, RecordAudioOutput, SpectrogramArgs};
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::HatariServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::ErrorData as McpError;

fn mjj_config() -> SessionConfig {
    SessionConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: "/tmp/shiraz-mcp-itest-audio".into(),
        ..SessionConfig::default()
    }
}

fn assert_no_orphan_hatari() {
    let status = Command::new("pgrep")
        .args(["-x", "hatari"])
        .status()
        .unwrap();
    assert!(
        !status.success(),
        "an orphan hatari process survived stop()"
    );
}

fn start_args() -> StartArgs {
    StartArgs {
        hatari_path: None,
        tos_path: None,
        harddrive: None,
        machine: None,
        memsize_mb: None,
    }
}

fn record_args(seconds: f64, path: Option<&str>) -> RecordAudioArgs {
    RecordAudioArgs {
        seconds,
        path: path.map(str::to_string),
    }
}

fn spec_args(
    seconds: Option<f64>,
    wav_path: Option<&str>,
    include_waveform: bool,
) -> SpectrogramArgs {
    SpectrogramArgs {
        seconds,
        wav_path: wav_path.map(str::to_string),
        fmax_hz: None,
        include_waveform: Some(include_waveform),
    }
}

fn last_json(result: &CallToolResult) -> serde_json::Value {
    let text = result.content.last().and_then(|b| b.as_text()).unwrap();
    serde_json::from_str(&text.text).unwrap()
}

/// `rmcp::Json<RecordAudioOutput>` isn't `Debug`, so `Result::unwrap_err`
/// (which requires the `Ok` side to be `Debug`) doesn't apply — this
/// extracts the error by hand instead.
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn expect_record_audio_err(server: &HatariServer, seconds: f64) -> McpError {
    match server
        .record_audio(Parameters(record_args(seconds, None)))
        .await
    {
        Ok(_) => panic!("expected record_audio to fail"),
        Err(e) => e,
    }
}

/// The MJJ demo needs seconds of fast-forwarded boot before music plays —
/// poll `record_audio` (1s each) until a non-silent capture lands (§11.6
/// step 2). Panics if the budget is exhausted.
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_for_music(server: &HatariServer) -> RecordAudioOutput {
    for _ in 0..15 {
        let out = server
            .record_audio(Parameters(record_args(1.0, None)))
            .await
            .unwrap()
            .0;
        if !out.silent && out.peak > 0.01 {
            return out;
        }
    }
    panic!("record_audio never captured non-silent MJJ audio within the poll budget");
}

fn assert_recording(out: &RecordAudioOutput) {
    assert_eq!(out.sample_rate, 44100);
    assert_eq!(out.channels, 2);
    assert_eq!(out.bits_per_sample, 16);
    assert!((0.8..=1.6).contains(&out.duration_s), "{}", out.duration_s);
    assert!(out.fast_forward_restored);
    let bytes = std::fs::read(&out.wav_path).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
}

fn distinct_pixels(png: &[u8]) -> usize {
    let decoded = image::load_from_memory(png).unwrap().to_rgb8();
    let set: HashSet<[u8; 3]> = decoded.pixels().map(|p| p.0).collect();
    set.len()
}

async fn assert_spectrogram(server: &HatariServer) {
    let result = server
        .spectrogram(Parameters(spec_args(Some(2.0), None, true)))
        .await
        .unwrap();
    assert_eq!(result.content.len(), 3, "expected [image, image, text]");
    assert!(result.content[0].as_image().is_some());
    assert!(result.content[1].as_image().is_some());
    let v = last_json(&result);
    let frames = v["frames"].as_u64().unwrap() as usize;
    assert!((140..=210).contains(&frames), "{frames}");
    assert_eq!(v["bins"], 372);
    assert_eq!(v["fmax_hz"], 8000);
    assert_eq!(v["n_fft"], 2048);
    assert_eq!(v["hop"], 512);
    let scale_x = 640_usize.div_ceil(frames).max(1);
    assert_eq!(v["width"], (frames * scale_x) as u64);
    assert_eq!(v["silent"], false);

    let spec_png = std::fs::read(v["spectrogram_png"].as_str().unwrap()).unwrap();
    let wave_png = std::fs::read(v["waveform_png"].as_str().unwrap()).unwrap();
    let spec_dims = image::load_from_memory(&spec_png)
        .unwrap()
        .to_rgb8()
        .dimensions();
    assert_eq!(
        spec_dims,
        (
            v["width"].as_u64().unwrap() as u32,
            v["height"].as_u64().unwrap() as u32
        )
    );
    assert_eq!(
        image::load_from_memory(&wave_png)
            .unwrap()
            .to_rgb8()
            .dimensions(),
        (1200, 300)
    );
    assert!(
        distinct_pixels(&spec_png) >= 8,
        "spectrogram looks degenerate"
    );
}

/// §11.6 step 5: every fast-forward suspend was paired with a restore.
fn assert_fast_forward_paired(work_dir: &str) {
    let log = std::fs::read_to_string(format!("{work_dir}/hatari.log")).unwrap();
    let offs = log.matches("- 'off'").count();
    let ons = log.matches("- 'on'").count();
    assert_eq!(offs, ons);
    assert!(offs >= 1);
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn record_and_see_mjj_music() {
    let server = HatariServer::new(mjj_config());
    server.start(Parameters(start_args())).await.unwrap();

    let recorded = wait_for_music(&server).await;
    assert_recording(&recorded);

    assert_spectrogram(&server).await;

    assert_fast_forward_paired("/tmp/shiraz-mcp-itest-audio");
    assert_eq!(server.status().await.unwrap().0.state, "running");

    server.pause().await.unwrap();
    let err = expect_record_audio_err(&server, 1.0).await;
    assert!(err.message.contains("session is paused"));
    server.resume().await.unwrap();

    server.stop().await.unwrap();
    let err = expect_record_audio_err(&server, 1.0).await;
    assert!(err.message.contains("session is stopped"));

    let reanalyze = server
        .spectrogram(Parameters(spec_args(None, Some(&recorded.wav_path), false)))
        .await
        .unwrap();
    assert_eq!(reanalyze.content.len(), 2, "expected [image, text]");

    assert_no_orphan_hatari();
}
