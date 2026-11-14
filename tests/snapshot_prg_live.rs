//! `save_prg` mode="snapshot" end-to-end against a real Hatari session
//! (P22.md §10.8): freeze → emit a self-restoring `.PRG` → reboot from a
//! staged `AUTO/` folder → verify the resumed instant. `#[ignore]`d — needs
//! `DISPLAY`, `/usr/bin/hatari`, and the MJJ tree. Single-Hatari discipline:
//! must not run while another agent drives the emulator. Run:
//! `DISPLAY=:0 cargo test --test snapshot_prg_live -- --ignored --nocapture`

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape/timeout against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use rmcp::handler::server::wrapper::{Json, Parameters};

use shiraz_mcp::ripper::prg_file;
use shiraz_mcp::session::HatariConfig;
use shiraz_mcp::tools::breakpoint::{BreakpointAction, BreakpointArgs};
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::mem::ReadMemoryArgs;
use shiraz_mcp::tools::save_prg::{SavePrgArgs, SavePrgOutput};
use shiraz_mcp::tools::HatariServer;
use shiraz_mcp::types::Addr;

const MJJ_BUILD: &str = "/path/to/projects/MJJ/build";
const RAM_LEN: u32 = 1 << 20;

fn no_args() -> StartArgs {
    StartArgs {
        hatari_path: None,
        tos_path: None,
        harddrive: None,
        machine: None,
        memsize_mb: None,
        ..Default::default()
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

#[allow(clippy::panic, reason = "integration test, not library code")]
fn expect_err<T>(r: Result<T, rmcp::ErrorData>) -> rmcp::ErrorData {
    match r {
        Ok(_) => panic!("expected an error result"),
        Err(e) => e,
    }
}

#[allow(clippy::panic, reason = "integration test, not library code")]
fn expect_snapshot(out: SavePrgOutput) -> shiraz_mcp::tools::save_prg::SnapshotOutput {
    match out {
        SavePrgOutput::Snapshot(s) => s,
        SavePrgOutput::Repatch(_) => panic!("expected the Snapshot variant"),
    }
}

#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_until_ready(server: &HatariServer) {
    for _ in 0..50 {
        let Json(r) = server.registers().await.unwrap();
        if Addr::parse(&r.pc).is_ok_and(|pc| pc.raw() < RAM_LEN) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("program never became ready (PC in RAM) within the poll budget");
}

/// Stage a GEMDOS drive whose sole `AUTO/` entry is the snapshot `.PRG`
/// (the MJJ tree itself is never written, ADR-8 precedent).
fn stage_snapshot_harddrive(work_dir: &Path, snap: &Path) -> PathBuf {
    let hd = work_dir.join("hd_boot");
    let _ = std::fs::remove_dir_all(&hd);
    let auto = hd.join("AUTO");
    std::fs::create_dir_all(&auto).unwrap();
    std::fs::copy(snap, auto.join("SNAP.PRG")).unwrap();
    hd
}

/// Local hex decode (no `hex` crate dependency; matches the shape of the
/// project's own internal `mem::scan::decode_hex`, not visible outside
/// `tools::mem`).
fn decode_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

async fn read_palette(server: &HatariServer) -> Vec<u8> {
    let Json(out) = server
        .read_memory(Parameters(ReadMemoryArgs {
            addr: "$FF8240".to_string(),
            count: Some(32),
        }))
        .await
        .unwrap();
    decode_hex(&out.bytes_hex)
}

/// The captured facts a resumed snapshot must reproduce (§10.8 step 2).
struct Frozen {
    pc: String,
    palette: Vec<u8>,
}

/// Freeze mid-animation and retry if the PC lands in shared TOS/GEMDOS ROM
/// (>= `RAM_LEN`): the resume-verification below arms a one-shot
/// `pc = frozen.pc` breakpoint as a race-free stand-in for "the exact
/// resume instant", which only holds when that address can't otherwise
/// recur — true for the frozen program's own RAM, not for ROM routines
/// that legitimately re-enter the same addresses many times during a
/// fresh boot.
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn freeze_and_capture(server: &HatariServer) -> Frozen {
    wait_until_ready(server).await;
    tokio::time::sleep(Duration::from_secs(5)).await; // let the demo animate
    server.pause().await.unwrap();
    let mut regs = server.registers().await.unwrap().0;
    for _ in 0..10 {
        if Addr::parse(&regs.pc).is_ok_and(|pc| pc.raw() < RAM_LEN) {
            break;
        }
        server.resume().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        server.pause().await.unwrap();
        regs = server.registers().await.unwrap().0;
    }
    assert!(
        Addr::parse(&regs.pc).is_ok_and(|pc| pc.raw() < RAM_LEN),
        "never landed a freeze with PC in RAM after 10 retries"
    );
    let palette = read_palette(server).await;
    Frozen {
        pc: regs.pc,
        palette,
    }
}

async fn take_snapshot(
    server: &HatariServer,
    out_path: &Path,
) -> shiraz_mcp::tools::save_prg::SnapshotOutput {
    let Json(out) = server
        .save_prg(Parameters(SavePrgArgs {
            out_path: out_path.display().to_string(),
            source_prg: None,
            mode: Some("snapshot".to_string()),
            regions: None,
        }))
        .await
        .unwrap();
    let snap = expect_snapshot(out);
    assert!(snap.payload_len > 0);
    assert!(snap.packed_pct < 100);
    assert!(out_path.is_file());
    prg_file::read_prg_file(out_path).unwrap();
    snap
}

/// Race-free stand-in for "the CPU has just resumed at the frozen
/// instant": arm a one-shot `pc = frozen_pc` breakpoint and wait for it to
/// fire. `breakpoint` pauses the CPU exactly there -- before Hatari's own
/// VBL-driven screen/palette redraw can run even once -- unlike polling
/// `registers()` and racing a separate `pause()` call afterward, which
/// leaves a window (bounded by the poll interval) for real frames, and any
/// palette-cycle effect driven by them, to run first. Only valid when
/// `frozen_pc` is an address that can't otherwise recur during a fresh
/// boot (`freeze_and_capture` guarantees this by retrying out of ROM).
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_for_resume_at(server: &HatariServer, frozen_pc: &str) {
    let Json(bp) = server
        .breakpoint(Parameters(BreakpointArgs {
            action: BreakpointAction::Set,
            condition: Some(format!("pc = {frozen_pc}")),
            timeout_ms: Some(30_000),
            index: None,
        }))
        .await
        .unwrap();
    assert_eq!(
        bp.fired,
        Some(true),
        "snapshot never resumed at the frozen PC within the timeout"
    );
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn snapshot_resumes_frozen_scene() {
    let work_dir: PathBuf = "/tmp/shiraz-mcp-itest-snapshot-prg".into();
    let server = HatariServer::new(HatariConfig {
        harddrive: Some(MJJ_BUILD.into()),
        work_dir: work_dir.join("mcp1"),
        ..HatariConfig::default()
    });
    server.start(Parameters(no_args())).await.unwrap();
    let frozen = freeze_and_capture(&server).await;

    let snap_path = work_dir.join("SNAP.PRG");
    let snap = take_snapshot(&server, &snap_path).await;
    assert_eq!(
        snap.entry_pc, frozen.pc,
        "facts.entry_pc must echo the frozen PC"
    );

    server.stop().await.unwrap();
    assert_no_orphan_hatari();

    // Reboot from a staged drive whose sole AUTO/ program is the snapshot.
    let hd = stage_snapshot_harddrive(&work_dir, &snap_path);
    let server2 = HatariServer::new(HatariConfig {
        harddrive: Some(hd),
        work_dir: work_dir.join("mcp2"),
        ..HatariConfig::default()
    });
    server2.start(Parameters(no_args())).await.unwrap();
    wait_for_resume_at(&server2, &frozen.pc).await;

    // The breakpoint left the session paused at the exact frozen PC, so
    // this palette read is race-free -- no post-resume VBL has run yet.
    let palette = read_palette(&server2).await;
    assert_eq!(
        palette, frozen.palette,
        "restored palette must match the frozen one"
    );
    println!("frozen PC was {}", frozen.pc);

    // Let a couple of frames actually render before screenshotting: at the
    // exact resume instant Hatari hasn't redrawn the SDL surface from the
    // restored VRAM yet (that only happens once the CPU runs a VBL).
    server2.resume().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    server2.pause().await.unwrap();

    let shot = server2.screenshot().await.unwrap();
    if let Some(text) = shot.content.get(1).and_then(|b| b.as_text()) {
        println!("resumed-scene screenshot: {}", text.text);
    }

    server2.stop().await.unwrap();
    assert_no_orphan_hatari();
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn snapshot_rejects_running_session() {
    let work_dir: PathBuf = "/tmp/shiraz-mcp-itest-snapshot-prg-reject".into();
    let server = HatariServer::new(HatariConfig {
        harddrive: Some(MJJ_BUILD.into()),
        work_dir: work_dir.clone(),
        ..HatariConfig::default()
    });
    server.start(Parameters(no_args())).await.unwrap();
    wait_until_ready(&server).await;

    let err = expect_err(
        server
            .save_prg(Parameters(SavePrgArgs {
                out_path: work_dir.join("should-not-exist.prg").display().to_string(),
                source_prg: None,
                mode: Some("snapshot".to_string()),
                regions: None,
            }))
            .await,
    );
    assert!(err.message.contains("paused"));

    server.stop().await.unwrap();
    assert_no_orphan_hatari();
}
