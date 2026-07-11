//! P27 §4.6: live behavioral checks for A2 (broken-pipe = dead Hatari,
//! `status` self-heals) and the A1 noise-filter regression, driven through
//! the PUBLIC tool surface (mirrors `tests/re_live.rs`'s session-bootstrap
//! and cleanup shape). Needs `DISPLAY`, `/usr/bin/hatari`, and the MJJ
//! tree. `#[ignore]`d; run with
//! `DISPLAY=:0 cargo test --test control_recover_live -- --ignored --test-threads=1`.
//!
//! Do NOT run this against a session another agent/process may be using —
//! single Hatari instance, single fifo (`/tmp/hatari_fifo`).

// [clippy::unwrap_used, clippy::expect_used] this whole binary is test code
// (see tests/analyze_live.rs for the same rationale) — a panic on
// unexpected shape/timeout against a real Hatari session is the correct
// failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]
#![allow(clippy::expect_used, reason = "integration test, not library code")]

use std::process::Command;
use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;

use shiraz_mcp::session::HatariConfig;
use shiraz_mcp::tools::frames::RunFramesArgs;
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::mem::ReadMemoryArgs;
use shiraz_mcp::tools::HatariServer;

/// The GEMDOS-HD tree this test boots — same placeholder/override
/// convention as `tests/profile_frame_live.rs` (P25).
fn test_hd() -> String {
    std::env::var("SHIRAZMCP_TEST_HD").unwrap_or_else(|_| "/path/to/projects/MJJ/build".to_string())
}

fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some(test_hd().into()),
        work_dir: format!("/tmp/shiraz-mcp-control-recover-live-{name}").into(),
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

/// Returns the pid so the test can `kill -9` it directly — the whole point
/// of this suite is exercising death the FSM did not itself cause.
async fn start(server: &HatariServer) -> u32 {
    let out = server.start(Parameters(no_args())).await.unwrap();
    out.0.pid.expect("start must report a pid")
}

fn kill9(pid: u32) {
    let status = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success(), "kill -9 {pid} failed to run");
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

fn read_pc_args() -> ReadMemoryArgs {
    ReadMemoryArgs {
        addr: "$0".to_string(),
        count: Some(4),
    }
}

/// §4.6 item 1: a plain `kill -9` between two Running-state tool calls —
/// the fifo write end is still open (Hatari's read end only closes on
/// process exit, §1.4), so the NEXT control-plane write is what surfaces
/// EPIPE; `status` must then self-heal instead of lying `running`/`paused`.
#[tokio::test]
#[ignore = "needs DISPLAY, hatari, and the MJJ tree"]
async fn kill_while_running_is_detected_and_self_heals() {
    let server = HatariServer::new(mjj_config("kill-while-running"));
    let pid = start(&server).await;
    kill9(pid);
    // Give the kernel a moment to finish tearing the process down so the
    // fifo read end is actually gone before we probe (not a sentinel poll —
    // there is nothing Hatari-side left to answer one).
    tokio::time::sleep(Duration::from_millis(300)).await;

    let err = server
        .read_memory(Parameters(read_pc_args()))
        .await
        .err()
        .expect("read_memory against a killed hatari must fail");
    assert!(
        err.message.contains("hatari exited"),
        "expected a ProcessExited-shaped error, got: {}",
        err.message
    );

    let status = server.status().await.unwrap();
    assert_eq!(status.0.state, "stopped");
    assert!(
        status.0.note.is_some(),
        "status must explain the self-heal in `note`"
    );

    // The session is genuinely stopped now — `start` must succeed again.
    let _ = start(&server).await;
    server.stop().await.unwrap();
    assert_no_orphan_hatari();
}

/// §4.6 item 2: same death, but discovered from the Paused/stdin plane
/// (exercises `classify_stdin_epipe` rather than `recover_fifo_epipe`).
#[tokio::test]
#[ignore = "needs DISPLAY, hatari, and the MJJ tree"]
async fn kill_while_paused_is_detected_via_stdin_plane() {
    let server = HatariServer::new(mjj_config("kill-while-paused"));
    let pid = start(&server).await;
    server.pause().await.unwrap();
    kill9(pid);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let err = server
        .read_memory(Parameters(read_pc_args()))
        .await
        .err()
        .expect("read_memory against a killed, paused hatari must fail");
    assert!(
        err.message.contains("hatari exited"),
        "expected a ProcessExited-shaped error via the stdin plane, got: {}",
        err.message
    );

    let status = server.status().await.unwrap();
    assert_eq!(status.0.state, "stopped");
    assert!(status.0.note.is_some());

    let _ = start(&server).await;
    server.stop().await.unwrap();
    assert_no_orphan_hatari();
}

/// A1 regression (P27 §2): a 50-frame run must never leak Hatari's idle-path
/// EAGAIN spam into the tool's returned `output`.
#[tokio::test]
#[ignore = "needs DISPLAY, hatari, and the MJJ tree"]
async fn run_frames_output_has_no_fifo_noise() {
    let server = HatariServer::new(mjj_config("run-frames-noise"));
    let _ = start(&server).await;

    let out = server
        .run_frames(Parameters(RunFramesArgs {
            frames: 50,
            timeout_ms: None,
        }))
        .await
        .unwrap();
    if let Some(output) = &out.0.output {
        assert!(
            !output.contains("command FIFO read error"),
            "run_frames output leaked Hatari's EAGAIN noise: {output}"
        );
    }

    server.stop().await.unwrap();
    assert_no_orphan_hatari();
}
