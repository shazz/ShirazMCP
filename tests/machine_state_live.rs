//! `machine_state` end-to-end against a real Hatari session (P10.md §9.3).
//! `#[ignore]`d — needs `DISPLAY`, `/usr/bin/hatari`, and the MJJ tree. Run:
//! `DISPLAY=:0 cargo test --test machine_state_live -- --ignored --test-threads=1`.
//!
//! Deliberately drives the TOOL layer (`HatariServer`'s public methods, the
//! same surface an MCP client calls) rather than the library path
//! `tests/integration.rs` already covers — this is the end-to-end proof of
//! ADR-20's pause/gather/resume choreography, including the lock-held
//! session-state bookkeeping around it.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/integration.rs for the same rationale) — a panic on unexpected
// shape/timeout against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::collections::HashSet;
use std::process::Command;
use std::time::Duration;

use base64::Engine;
use shiraz_mcp::session::SessionConfig;
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::machine_state::MachineStateArgs;
use shiraz_mcp::tools::HatariServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;

fn mjj_config() -> SessionConfig {
    SessionConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: "/tmp/shiraz-mcp-itest-machine-state".into(),
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

fn snapshot_args(include_screen: bool, include_raw_dump: bool) -> MachineStateArgs {
    MachineStateArgs {
        include_screen: Some(include_screen),
        include_raw_dump: Some(include_raw_dump),
    }
}

/// The last content block (the JSON block — `compose`'s step 6 order).
fn last_json(result: &CallToolResult) -> serde_json::Value {
    let text = result.content.last().and_then(|b| b.as_text()).unwrap();
    serde_json::from_str(&text.text).unwrap()
}

fn distinct_palette_words(v: &serde_json::Value) -> usize {
    let pal = v["video"]["palette"].as_array().unwrap();
    let raws: HashSet<&str> = pal.iter().map(|e| e["raw"].as_str().unwrap()).collect();
    raws.len()
}

/// True iff the attached image block decodes to at least 2 distinct pixel
/// values — cheap "is this actually a picture, not a blank buffer" gate,
/// same rationale as `tests/integration.rs`'s `is_non_degenerate`.
fn image_block_is_non_degenerate(block: &rmcp::model::ContentBlock) -> bool {
    let Some(img) = block.as_image() else {
        return false;
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&img.data) else {
        return false;
    };
    let Ok(decoded) = image::load_from_memory(&bytes) else {
        return false;
    };
    let distinct: HashSet<[u8; 4]> = decoded.to_rgba8().pixels().map(|p| p.0).collect();
    distinct.len() >= 2
}

/// MJJHACK.PRG is UPX-packed and needs a handful of VBLs to unpack and draw
/// — freezing right after `start()` can catch TOS's own (already plausible-
/// resolution, already plausible-palette) blank desktop, so the gate also
/// requires the rendered screen itself to be non-degenerate (the actual
/// source of `view_screen_matches_live`'s historical flake, P4 §8.4, and
/// `tests/integration.rs`'s `freeze_when_rendered` precedent).
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_for_plausible_snapshot(server: &HatariServer) -> (CallToolResult, serde_json::Value) {
    for _ in 0..50 {
        let result = server
            .machine_state(Parameters(snapshot_args(true, false)))
            .await
            .unwrap();
        let v = last_json(&result);
        let plausible = v["video"]["resolution"] != "invalid"
            && distinct_palette_words(&v) >= 4
            && result
                .content
                .first()
                .is_some_and(image_block_is_non_degenerate);
        if plausible {
            return (result, v);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("machine_state never produced a plausible snapshot within the poll budget");
}

/// The running-session snapshot: blocks, coherence facts, and the rendered
/// PNG's non-degeneracy (§9.3 steps 2-3). No pixel-equality assertion
/// against `screenshot` — MJJ double-buffers, so the *programmed* base this
/// tool reports can be the back buffer (§2's pinned observation).
fn assert_running_snapshot(result: &CallToolResult, v: &serde_json::Value) {
    assert_eq!(result.content.len(), 3, "expected [image, text, text]");
    assert!(result.content[0].as_image().is_some());
    let pretty = &result.content[1].as_text().unwrap().text;
    assert!(pretty.starts_with("=== machine_state — Atari ST snapshot ==="));

    assert_eq!(v["session"], "running");
    assert_eq!(v["momentary_freeze"], true);
    let sync = v["video"]["sync_hz"].as_u64().unwrap();
    assert!(sync == 50 || sync == 60);
    let base = v["video"]["screen_base"].as_str().unwrap();
    assert!(base.starts_with('$') && base.len() >= 7);
    let imask = v["cpu"]["sr"]["imask"].as_u64().unwrap();
    assert!(imask <= 7);

    let png_path = v["video_memory"]["png_path"].as_str().unwrap();
    let png_bytes = std::fs::read(png_path).unwrap();
    assert_eq!(
        &png_bytes[..8],
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
    );
    let decoded = image::load_from_memory(&png_bytes).unwrap();
    let distinct: HashSet<[u8; 4]> = decoded.to_rgba8().pixels().map(|p| p.0).collect();
    assert!(distinct.len() >= 2, "expected a non-degenerate screen");
}

/// Paused-session snapshot: read in place, never thawed (§9.3 step 5).
async fn assert_paused_snapshot(server: &HatariServer) {
    let result = server
        .machine_state(Parameters(snapshot_args(false, true)))
        .await
        .unwrap();
    assert_eq!(result.content.len(), 2, "expected [text, text] (no image)");
    let v = last_json(&result);
    assert_eq!(v["session"], "paused");
    assert_eq!(v["momentary_freeze"], false);
    assert!(v["video_memory"]["png_path"].is_null());

    let raw_path = v["video_memory"]["raw_path"].as_str().unwrap();
    let raw_bytes = std::fs::read(raw_path).unwrap();
    assert_eq!(raw_bytes.len(), 32000);

    let status = server.status().await.unwrap();
    assert_eq!(status.0.state, "paused", "a user's freeze is never thawed");
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn machine_state_snapshot_of_mjj() {
    let server = HatariServer::new(mjj_config());
    server.start(Parameters(start_args())).await.unwrap();

    let (result, v) = wait_for_plausible_snapshot(&server).await;
    assert_running_snapshot(&result, &v);

    // Thaw invariant (the ADR-20 point): the momentary freeze left the
    // session exactly where it found it.
    let status = server.status().await.unwrap();
    assert_eq!(status.0.state, "running");

    server.pause().await.unwrap();
    assert_paused_snapshot(&server).await;

    server.stop().await.unwrap();
    let err = server
        .machine_state(Parameters(snapshot_args(true, false)))
        .await
        .unwrap_err();
    assert!(err.message.contains("session is stopped"));

    assert_no_orphan_hatari();
}
