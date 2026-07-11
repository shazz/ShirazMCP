//! P27 §3.7: live behavioral checks for `send_key`, driven through the
//! PUBLIC tool surface — mirrors `tests/re_live.rs`'s session-bootstrap and
//! cleanup shape. Needs `DISPLAY`, `/usr/bin/hatari`, and the MJJ tree —
//! `#[ignore]`d; run with
//! `DISPLAY=:0 cargo test --test send_key_live -- --ignored --test-threads=1`.
//!
//! Do NOT run this against a session another agent/process may be using —
//! single Hatari instance, single fifo (`/tmp/hatari_fifo`).

// [clippy::unwrap_used] this whole binary is test code (see
// tests/analyze_live.rs for the same rationale) — a panic on unexpected
// shape/timeout against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::process::Command;

use rmcp::handler::server::wrapper::Parameters;

use shiraz_mcp::session::HatariConfig;
use shiraz_mcp::tools::input::SendKeyArgs;
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::HatariServer;

/// The GEMDOS-HD tree this test boots — same placeholder/override
/// convention as `tests/profile_frame_live.rs` (P25).
fn test_hd() -> String {
    std::env::var("SHIRAZMCP_TEST_HD").unwrap_or_else(|_| "/path/to/projects/MJJ/build".to_string())
}

fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some(test_hd().into()),
        work_dir: format!("/tmp/shiraz-mcp-send-key-live-{name}").into(),
        ..HatariConfig::default()
    }
}

fn no_args() -> StartArgs {
    StartArgs {
        hatari_path: None,
        tos_path: None,
        harddrive: None,
        machine: None,
        memsize_mb: None,
    }
}

async fn start(server: &HatariServer) {
    server.start(Parameters(no_args())).await.unwrap();
}

async fn stop(server: &HatariServer) {
    server.stop().await.unwrap();
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

fn key_args(keys: &[&str], action: Option<&str>, frames: Option<u32>) -> SendKeyArgs {
    SendKeyArgs {
        keys: keys.iter().map(|s| s.to_string()).collect(),
        action: action.map(str::to_string),
        frames,
        timeout_ms: None,
    }
}

/// The headless-capture composition: freeze, queue "space", run exactly 2
/// VBL frames, end paused — the primary `send_key` use case (P27 §3.1).
#[tokio::test]
#[ignore = "needs DISPLAY, hatari, and the MJJ tree"]
async fn send_key_with_frames_composes_headless_capture() {
    let server = HatariServer::new(mjj_config("with-frames"));
    start(&server).await;

    let out = server
        .send_key(Parameters(key_args(&["space"], None, Some(2))))
        .await
        .unwrap();
    assert_eq!(out.0.frames_completed, Some(true));
    assert_eq!(out.0.state, "paused");
    assert_eq!(out.0.sent, vec!["hatari-event keypress 57".to_string()]);

    stop(&server).await;
    assert_no_orphan_hatari();
}

/// Paused + no `frames`: events queue in the pipe (no confirm is possible,
/// P27 §3.5 step 5) and a later `resume` must still succeed — the queued
/// event replays harmlessly.
#[tokio::test]
#[ignore = "needs DISPLAY, hatari, and the MJJ tree"]
async fn send_key_while_paused_queues_and_resume_still_succeeds() {
    let server = HatariServer::new(mjj_config("paused-queues"));
    start(&server).await;
    server.pause().await.unwrap();

    let out = server
        .send_key(Parameters(key_args(&["space"], None, None)))
        .await
        .unwrap();
    assert!(out.0.queued);
    assert_eq!(out.0.state, "paused");

    server.resume().await.unwrap();

    stop(&server).await;
    assert_no_orphan_hatari();
}

/// A down/up round-trip on a Running session — the state-polling workaround
/// the tool description recommends for press+release in one main-loop
/// slice being missed.
#[tokio::test]
#[ignore = "needs DISPLAY, hatari, and the MJJ tree"]
async fn send_key_down_then_up_round_trip() {
    let server = HatariServer::new(mjj_config("down-up"));
    start(&server).await;

    let down = server
        .send_key(Parameters(key_args(&["57"], Some("down"), None)))
        .await
        .unwrap();
    assert!(!down.0.queued);
    assert_eq!(down.0.state, "running");

    let up = server
        .send_key(Parameters(key_args(&["57"], Some("up"), None)))
        .await
        .unwrap();
    assert!(!up.0.queued);
    assert_eq!(up.0.state, "running");

    stop(&server).await;
    assert_no_orphan_hatari();
}
