//! P25.md §10.3: live behavioral check for `profile_frame`, driven through
//! the PUBLIC tool surface — the `tests/analyze_live.rs` pattern exactly.
//! Needs `DISPLAY`, `/usr/bin/hatari`, and the MJJ tree — `#[ignore]`d; run
//! with `DISPLAY=:0 cargo test --test profile_frame_live -- --ignored
//! --test-threads=1`.
//!
//! Do NOT run this against a session another agent/process may be using —
//! single Hatari instance, single fifo (`/tmp/hatari_fifo`).

// [clippy::unwrap_used] this whole binary is test code (see
// tests/analyze_live.rs for the same rationale) — a panic on unexpected
// shape/timeout against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::path::Path;
use std::process::Command;

use rmcp::handler::server::wrapper::{Json, Parameters};

use shiraz_mcp::session::HatariConfig;
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::profile_frame::ProfileFrameArgs;
use shiraz_mcp::tools::HatariServer;
use shiraz_mcp::types::Addr;

/// The GEMDOS-HD tree this test boots. The committed default is the
/// placeholder the public-release PII scrub left in every live test; a local
/// checkout points at its own tree via `SHIRAZMCP_TEST_HD` (`SHIRAZMCP_TOS`
/// is already read by `HatariConfig::default`). Without both, Hatari exits
/// before creating its fifo and the test fails as "hatari fifo creation".
fn test_hd() -> String {
    std::env::var("SHIRAZMCP_TEST_HD").unwrap_or_else(|_| "/path/to/projects/MJJ/build".to_string())
}

fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some(test_hd().into()),
        work_dir: format!("/tmp/shiraz-mcp-profile-frame-live-{name}").into(),
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

const RAM_LEN: u32 = 1 << 20;

#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_until_ready(server: &HatariServer) {
    for _ in 0..50 {
        let Json(r) = server.registers().await.unwrap();
        if let Ok(pc) = Addr::parse(&r.pc) {
            if pc.raw() < RAM_LEN {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("MJJHACK.PRG never became ready (PC in RAM) within the poll budget");
}

fn frame_args(frames: u32, from: Option<&str>, to: Option<&str>) -> ProfileFrameArgs {
    ProfileFrameArgs {
        frames,
        from: from.map(str::to_string),
        to: to.map(str::to_string),
        timeout_ms: None,
    }
}

#[tokio::test]
#[ignore = "needs DISPLAY, hatari, and the MJJ tree"]
async fn profile_frame_measures_whole_frames() {
    let server = HatariServer::new(mjj_config("measures-whole-frames"));
    start(&server).await;
    wait_until_ready(&server).await;

    // No extent: total_* only, and THIS is the machine-check of the §1.2
    // reset semantics — if the buffer did not reset per continue, frame 2
    // would read ~320k and frame 3 ~480k.
    let Json(totals_only) = server
        .profile_frame(Parameters(frame_args(3, None, None)))
        .await
        .unwrap();
    assert_eq!(totals_only.per_frame_total_cycles.len(), 3);
    for &c in &totals_only.per_frame_total_cycles {
        assert!(
            (155_000..=165_256).contains(&c),
            "frame total {c} outside the expected ~160256 band \
             (reset-on-continue semantics did not hold)"
        );
    }
    assert_eq!(totals_only.cost_basis, "total");
    assert!(totals_only.range_cycles_min.is_none());
    assert_eq!(totals_only.state, "paused");
    assert!(Path::new(&totals_only.worst_frame_path).exists());
    assert!(Path::new(&totals_only.profile_dir).exists());

    // 1 MiB RAM extent (TOS ROM at $E00000+ excluded).
    let Json(with_extent) = server
        .profile_frame(Parameters(frame_args(3, Some("$0"), Some("$100000"))))
        .await
        .unwrap();
    assert_eq!(with_extent.cost_basis, "range");
    let ranges = with_extent.per_frame_range_cycles.clone().unwrap();
    assert_eq!(ranges.len(), 3);
    for (r, t) in ranges.iter().zip(&with_extent.per_frame_total_cycles) {
        assert!(r <= t, "range cycles {r} exceeded total cycles {t}");
    }
    assert!(with_extent.range_cycles_max.unwrap() <= with_extent.total_cycles_max);
    assert!(with_extent.headroom_pct.is_finite());

    stop(&server).await;
    assert_no_orphan_hatari();
}
