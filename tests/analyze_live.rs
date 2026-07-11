//! P2.md §10.4: live behavioral checks for `disasm`/`profile`/`breakpoint`/
//! `state`, driven through the PUBLIC tool surface (`HatariServer` +
//! direct method calls, like `tests/integration.rs`'s `Session`/
//! `HatariProcess` calls). Needs `DISPLAY`, `/usr/bin/hatari`, and the MJJ
//! tree — `#[ignore]`d; run with `DISPLAY=:0 cargo test --test
//! analyze_live -- --ignored --test-threads=1`.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape/timeout against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::path::{Path, PathBuf};
use std::process::Command;

use rmcp::handler::server::wrapper::{Json, Parameters};

use shiraz_mcp::session::HatariConfig;
use shiraz_mcp::tools::analyze::{DisasmArgs, DisasmOutput, DisasmSource};
use shiraz_mcp::tools::analyze_profile::ProfileArgs;
use shiraz_mcp::tools::breakpoint::{BreakpointAction, BreakpointArgs, StateAction, StateArgs};
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::HatariServer;
use shiraz_mcp::types::Addr;

/// Per-test work dirs (P1.md §10.4 rationale, unchanged for P2): a panicking
/// test's `HatariProcess` only gets RAII `kill_on_drop`, not the graceful
/// `stop()`, and a following test's fresh spawn reusing the same fifo/log
/// paths can race that teardown.
fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: format!("/tmp/shiraz-mcp-analyze-live-{name}").into(),
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

/// RAM size for `mjj_config` (`HatariConfig::default().memsize_mb` == 1).
const RAM_LEN: u32 = 1 << 20;

/// Bounded poll (not the boot gate) for the GEMDOS HD auto-run to actually
/// reach its own RAM-resident code — mirrors `tests/integration.rs`'s
/// `wait_until_ready`, minus the `symbols prg` check (no `symbols` tool
/// exists at the MCP surface; PC-in-RAM alone is a reliable readiness
/// signal for the MJJ demo).
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

async fn disasm(
    server: &HatariServer,
    addr: &str,
    count: u32,
    source: DisasmSource,
) -> DisasmOutput {
    let Json(out) = server
        .disasm(Parameters(DisasmArgs {
            addr: addr.to_string(),
            count: Some(count),
            source: Some(source),
        }))
        .await
        .unwrap();
    out
}

fn bp_args(
    action: BreakpointAction,
    condition: Option<&str>,
    timeout_ms: Option<u64>,
) -> BreakpointArgs {
    BreakpointArgs {
        action,
        condition: condition.map(str::to_string),
        timeout_ms,
        index: None,
    }
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn disasm_snapshot_matches_live_at_pc() {
    let server = HatariServer::new(mjj_config("disasm-snapshot-live"));
    start(&server).await;
    wait_until_ready(&server).await;
    server.pause().await.unwrap();

    let Json(regs) = server.registers().await.unwrap();
    let snap = disasm(&server, &regs.pc, 8, DisasmSource::Snapshot).await;
    let live = disasm(&server, &regs.pc, 8, DisasmSource::Live).await;

    assert_eq!(snap.insns.len(), 8);
    assert_eq!(live.insns.len(), 8);
    for (s, l) in snap.insns.iter().zip(&live.insns) {
        assert_eq!(s.addr, l.addr, "snapshot/live addr mismatch");
    }
    assert_ne!(snap.insns[0].mnemonic, "dc.w");

    stop(&server).await;
    assert_no_orphan_hatari();
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn breakpoint_set_fires_and_pauses() {
    let server = HatariServer::new(mjj_config("bp-set-fires"));
    start(&server).await;

    let Json(out) = server
        .breakpoint(Parameters(bp_args(
            BreakpointAction::Set,
            Some("pc > 0"),
            Some(10_000),
        )))
        .await
        .unwrap();
    assert_eq!(out.fired, Some(true));
    assert_eq!(out.state, "paused");

    let Json(status) = server.status().await.unwrap();
    assert_eq!(status.state, "paused");

    server.resume().await.unwrap();
    stop(&server).await;
    assert_no_orphan_hatari();
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn breakpoint_set_timeout_leaves_running_and_disarmed() {
    let server = HatariServer::new(mjj_config("bp-set-timeout"));
    start(&server).await;

    let Json(out) = server
        .breakpoint(Parameters(bp_args(
            BreakpointAction::Set,
            Some("pc = $1"),
            Some(2_000),
        )))
        .await
        .unwrap();
    assert_eq!(out.fired, Some(false));
    assert_eq!(out.state, "running");

    let Json(list) = server
        .breakpoint(Parameters(bp_args(BreakpointAction::List, None, None)))
        .await
        .unwrap();
    assert!(list.breakpoints.unwrap().is_empty());

    stop(&server).await;
    assert_no_orphan_hatari();
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn profile_demo_loop() {
    let server = HatariServer::new(mjj_config("profile-demo-loop"));
    start(&server).await;
    wait_until_ready(&server).await;
    server.pause().await.unwrap();

    let Json(regs) = server.registers().await.unwrap();
    let d = disasm(&server, &regs.pc, 4, DisasmSource::Snapshot).await;
    let to = d.insns[1].addr.clone();

    server.resume().await.unwrap();

    let Json(p) = server
        .profile(Parameters(ProfileArgs {
            from: regs.pc.clone(),
            to,
            timeout_ms: Some(20_000),
            region: None,
        }))
        .await
        .unwrap();
    assert!(p.measured_cycles.unwrap_or(0) > 0);
    assert!(p.static_cycles > 0);
    assert!(p.static_cycles_contended >= p.static_cycles);
    assert!(p.static_bus_cycles <= p.static_cycles);
    assert_eq!(p.region, "visible");
    assert!(p.vbl_fraction > 0.0 && p.vbl_fraction < 1.0);
    assert_eq!(p.state, "paused");
    assert!(Path::new(&p.profile_path).exists());

    stop(&server).await;
    assert_no_orphan_hatari();
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn state_save_load_roundtrip() {
    let server = HatariServer::new(mjj_config("state-save-load"));
    start(&server).await;

    let Json(saved) = server
        .state(Parameters(StateArgs {
            action: StateAction::Save,
            path: None,
        }))
        .await
        .unwrap();
    let meta = std::fs::metadata(&saved.path).unwrap();
    assert!(
        meta.len() > 100 * 1024,
        "snapshot too small: {} bytes",
        meta.len()
    );

    let Json(loaded) = server
        .state(Parameters(StateArgs {
            action: StateAction::Load,
            path: Some(saved.path.clone()),
        }))
        .await
        .unwrap();
    assert_eq!(loaded.state, "running");

    server.registers().await.unwrap();

    let Json(list) = server
        .breakpoint(Parameters(bp_args(BreakpointAction::List, None, None)))
        .await
        .unwrap();
    assert!(
        list.breakpoints.unwrap().is_empty(),
        "restore must clear breakpoints"
    );

    stop(&server).await;
    assert_no_orphan_hatari();
}

/// P2.md §10.4 item 6 (plain test, not `#[ignore]`d): the ADR-11 ban on
/// `hatari-stop` must hold for the new P2 control-plane code too.
#[test]
fn hatari_stop_grep_gate() {
    let mut files = Vec::new();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    collect_rs_files(&src, &mut files);
    assert!(
        !files.is_empty(),
        "src walk found no .rs files — test is broken"
    );
    for f in files {
        let content = std::fs::read_to_string(&f).unwrap();
        assert!(
            !content.contains("hatari-stop"),
            "banned string found in {f:?}"
        );
    }
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}
