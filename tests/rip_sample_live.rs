//! `rip_sample` end-to-end against a real Hatari session (P20.md §11.6).
//! `#[ignore]`d — needs `DISPLAY`, `/usr/bin/hatari`, and the DHS "Uhane"
//! demo tree. Run:
//! `DISPLAY=:0 cargo test --test rip_sample_live -- --ignored --test-threads=1`.
//!
//! Uhane's hdd has no `AUTO/` folder (§2.5), so this stages a copy with
//! `uhane.prg` renamed into `AUTO/UHANE.PRG` — the ADR-32 staged-copy
//! precedent (`tests/save_prg_live.rs`). Deliberately drives the TOOL layer
//! (`HatariServer`'s public methods), same precedent as
//! `tests/ym_state_live.rs`/`tests/machine_state_live.rs`.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/ym_state_live.rs for the same rationale) — a panic on unexpected
// shape/timeout against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use rmcp::handler::server::wrapper::{Json, Parameters};

use shiraz_mcp::session::SessionConfig;
use shiraz_mcp::tools::audio::SpectrogramArgs;
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::rip_sample::{RipSampleArgs, RipSampleOutput};
use shiraz_mcp::tools::HatariServer;

const UHANE_HDD: &str = "/path/to/projects/STEIsNotDead/assets/dhs/hdd";

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
        machine: Some("ste".to_string()),
        memsize_mb: Some(4),
    }
}

/// §2.5: the shipped hdd has no `AUTO/` folder, so TOS will not auto-run
/// `uhane.prg`. Stage a tree with it renamed into `AUTO/UHANE.PRG` +
/// `music1`/`music2` at the drive root (the demo GEMDOS-loads them
/// relative to the root) — the source tree is never written (ADR-8/32).
fn stage_uhane_harddrive(work_dir: &Path) -> PathBuf {
    let hd = work_dir.join("hd");
    let auto = hd.join("AUTO");
    std::fs::create_dir_all(&auto).unwrap();
    std::fs::copy(
        Path::new(UHANE_HDD).join("uhane.prg"),
        auto.join("UHANE.PRG"),
    )
    .unwrap();
    for name in ["music1", "music2"] {
        std::fs::copy(Path::new(UHANE_HDD).join(name), hd.join(name)).unwrap();
    }
    hd
}

fn rip_args(force: bool) -> RipSampleArgs {
    RipSampleArgs {
        out_path: None,
        force: Some(force),
    }
}

/// §11.6 step 1: poll until the DMA sound engine is playing — the demo
/// loads its data files first. Panics past the budget.
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_for_playing(server: &HatariServer) -> RipSampleOutput {
    for _ in 0..120 {
        let Json(out) = server
            .rip_sample(Parameters(rip_args(false)))
            .await
            .unwrap();
        if out.status == "playing" {
            return out;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("rip_sample never reported \"playing\" within the poll budget");
}

/// §11.6 step 2: shape assertions on the accepted snapshot.
fn assert_playing_snapshot(out: &RipSampleOutput) {
    assert!(
        [6258, 12517, 25033, 50066].contains(&out.rate_hz),
        "unexpected rate_hz {}",
        out.rate_hz
    );
    assert!(out.len_bytes > 0, "len_bytes should be > 0");
    assert_eq!(out.len_bytes % 2, 0, "len_bytes should be even (§2.1)");
    let wav_path = out.wav_path.as_ref().unwrap();
    assert!(Path::new(wav_path).exists(), "wav file not on disk");
    assert_eq!(out.silent, Some(false));
    assert!(out.duration_s > 0.0);
    assert!(out.momentary_freeze, "session was running, freeze expected");
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the DHS Uhane demo tree"]
async fn rip_sample_snapshot_of_uhane() {
    let work_dir: PathBuf = "/tmp/shiraz-mcp-itest-rip-sample".into();
    let hd = stage_uhane_harddrive(&work_dir);
    let server = HatariServer::new(SessionConfig {
        harddrive: Some(hd),
        work_dir: work_dir.join("mcp"),
        ..SessionConfig::default()
    });
    server.start(Parameters(start_args())).await.unwrap();

    let first = wait_for_playing(&server).await;
    assert_playing_snapshot(&first);

    // §11.6 step 3: rip twice in a row — the freeze/thaw restored the
    // machine, so a second call still answers.
    let Json(second) = server
        .rip_sample(Parameters(rip_args(false)))
        .await
        .unwrap();
    assert_eq!(second.status, "playing");

    // §11.6 step 4: the ripped WAV composes with spectrogram (the §3.4
    // compose guarantee, end-to-end).
    server
        .spectrogram(Parameters(SpectrogramArgs {
            seconds: None,
            wav_path: first.wav_path.clone(),
            fmax_hz: None,
            include_waveform: None,
        }))
        .await
        .unwrap();

    server.stop().await.unwrap();
    assert_no_orphan_hatari();
}
