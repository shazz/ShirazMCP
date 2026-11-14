//! `ym_state` end-to-end against a real Hatari session (P17.md §10.5).
//! `#[ignore]`d — needs `DISPLAY`, `/usr/bin/hatari`, and the MJJ tree. Run:
//! `DISPLAY=:0 cargo test --test ym_state_live -- --ignored --test-threads=1`.
//!
//! Deliberately drives the TOOL layer (`HatariServer`'s public methods),
//! same precedent as `tests/machine_state_live.rs`/`tests/audio_live.rs`:
//! this is the end-to-end proof that `info ym` is a single, non-intrusive
//! round-trip (ADR-33) — the running program's audio and PSG state are
//! never disturbed by reading them.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/machine_state_live.rs for the same rationale) — a panic on
// unexpected shape/timeout against a real Hatari session is the correct
// failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::process::Command;
use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use shiraz_mcp::session::SessionConfig;
use shiraz_mcp::tools::audio::RecordAudioArgs;
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::HatariServer;

fn mjj_config() -> SessionConfig {
    SessionConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: "/tmp/shiraz-mcp-itest-ym-state".into(),
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
        ..Default::default()
    }
}

/// The last content block (the JSON block — `[pretty, json]` order, §8).
fn last_json(result: &CallToolResult) -> serde_json::Value {
    let text = result.content.last().and_then(|b| b.as_text()).unwrap();
    serde_json::from_str(&text.text).unwrap()
}

fn any_audible(v: &serde_json::Value) -> bool {
    v["channels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["audible"] == true)
}

/// Music starts seconds after boot (`audio_live` precedent) — poll until a
/// snapshot shows at least one audible channel. Panics past the budget.
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_for_audible_snapshot(server: &HatariServer) -> (CallToolResult, serde_json::Value) {
    for _ in 0..50 {
        let result = server.ym_state().await.unwrap();
        let v = last_json(&result);
        if any_audible(&v) {
            return (result, v);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("ym_state never produced an audible snapshot within the poll budget");
}

/// §10.5 step 3: shape assertions on the accepted snapshot.
fn assert_running_snapshot(result: &CallToolResult, v: &serde_json::Value) {
    assert_eq!(result.content.len(), 2, "expected [text, text]");
    let pretty = &result.content[0].as_text().unwrap().text;
    assert!(pretty.starts_with("=== ym_state — YM2149 PSG snapshot ==="));
    assert_eq!(v["session"], "running");

    let channels = v["channels"].as_array().unwrap();
    assert_eq!(channels.len(), 3);
    let names: Vec<&str> = channels
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["A", "B", "C"]);
    for c in channels {
        assert!(c["period"].as_u64().unwrap() <= 4095);
        assert!(c["volume"].as_u64().unwrap() <= 15);
        if c["audible"] == true {
            assert!(c["freq_hz"].as_f64().unwrap() > 0.0);
        }
    }
    assert!(v["noise"]["period"].as_u64().unwrap() <= 31);

    let regs = v["registers_raw"].as_array().unwrap();
    assert_eq!(regs.len(), 14);
    for r in regs {
        let s = r.as_str().unwrap();
        assert!(s.starts_with('$') && s.len() == 3, "{s}");
    }
}

/// §10.5 step 4: the music keeps advancing after our reads — poll until the
/// raw registers differ from the accepted snapshot's. Panics past the budget.
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn assert_registers_drift(server: &HatariServer, baseline: &serde_json::Value) {
    for _ in 0..10 {
        let v = last_json(&server.ym_state().await.unwrap());
        if v["registers_raw"] != baseline["registers_raw"] {
            return;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    panic!("PSG registers never drifted after ym_state reads — read may be corrupting state");
}

/// §10.5 step 6: a frozen chip is stable — two paused reads must be
/// byte-identical, and this also pins that the read has no self-side-effect.
async fn assert_paused_stable(server: &HatariServer) {
    let first = last_json(&server.ym_state().await.unwrap());
    let second = last_json(&server.ym_state().await.unwrap());
    assert_eq!(first["session"], "paused");
    assert_eq!(second["session"], "paused");
    assert_eq!(first["registers_raw"], second["registers_raw"]);
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn ym_state_snapshot_of_mjj() {
    let server = HatariServer::new(mjj_config());
    server.start(Parameters(start_args())).await.unwrap();

    let (result, v) = wait_for_audible_snapshot(&server).await;
    assert_running_snapshot(&result, &v);

    // Non-corruption oracle 1: the read never pauses/pokes anything, so the
    // music keeps advancing — the raw registers must drift on their own.
    assert_registers_drift(&server, &v).await;

    // Non-corruption oracle 2: the chip still sings after N register reads —
    // a real recording is the strongest form of "audio continues normally".
    let recorded = server
        .record_audio(Parameters(RecordAudioArgs {
            seconds: 1.0,
            path: None,
        }))
        .await
        .unwrap()
        .0;
    assert!(!recorded.silent, "PSG went silent after ym_state reads");

    server.pause().await.unwrap();
    assert_paused_stable(&server).await;
    let status = server.status().await.unwrap();
    assert_eq!(status.0.state, "paused");

    server.stop().await.unwrap();
    let err = server.ym_state().await.unwrap_err();
    assert!(err.message.contains("session is stopped"));

    assert_no_orphan_hatari();
}
