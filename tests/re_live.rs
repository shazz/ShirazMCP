//! P14.md §9.4: live behavioral checks for `watch`/`xrefs`/`run_frames`,
//! driven through the PUBLIC tool surface — mirrors `tests/analyze_live.rs`'s
//! session-bootstrap + cleanup shape. Needs `DISPLAY`, `/usr/bin/hatari`, and
//! the MJJ tree — `#[ignore]`d; run with `DISPLAY=:0 cargo test --test
//! re_live -- --ignored --test-threads=1`.
//!
//! §9.4 item 5 (`watch_catches_planted_writer`, the redirect-a-loop
//! variant) is intentionally NOT implemented: the spec's own fallback rule
//! is "rely on test 1" when a real demo-code candidate fires within its
//! budget, and PC redirection into a planted loop is explicitly flagged
//! "stop and ask rust-architect instead of improvising" — not a call for
//! this test file to make unilaterally. `watch_fires_on_changing_address`
//! (below) already exercises the headline claim end-to-end: a write-watch
//! on live demo memory catches the writing PC.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape/timeout against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::process::Command;

use rmcp::handler::server::wrapper::{Json, Parameters};

use shiraz_mcp::session::HatariConfig;
use shiraz_mcp::tools::assemble::PatchArgs;
use shiraz_mcp::tools::breakpoint::{BreakpointAction, BreakpointArgs};
use shiraz_mcp::tools::frames::RunFramesArgs;
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::watch::{WatchArgs, WatchOn, WatchOutput};
use shiraz_mcp::tools::xrefs::XrefsArgs;
use shiraz_mcp::tools::HatariServer;
use shiraz_mcp::types::Addr;

/// Per-test work dirs (P1.md §10.4 rationale, reused by every live suite):
/// a panicking test's `HatariProcess` only gets RAII `kill_on_drop`, not the
/// graceful `stop()`, and a following test's fresh spawn reusing the same
/// fifo/log paths can race that teardown.
fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: format!("/tmp/shiraz-mcp-re-live-{name}").into(),
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
        ..Default::default()
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

fn watch_args(addr: &str, len: u32, timeout_ms: u64) -> WatchArgs {
    WatchArgs {
        addr: addr.to_string(),
        on: Some(WatchOn::Write),
        len: Some(len),
        timeout_ms: Some(timeout_ms),
    }
}

fn bp_args(action: BreakpointAction) -> BreakpointArgs {
    BreakpointArgs {
        action,
        condition: None,
        timeout_ms: None,
        index: None,
    }
}

/// §9.4 item 1 + item 6 (piggybacked): one of the known-live candidate
/// addresses (the MJJ scroller cursor / TOS clock ticks) must catch a
/// write, reporting a nonzero PC and a changed value; the transcript also
/// closes the §3.2 `// verify live` wire-format assertions.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn watch_fires_on_changing_address() {
    let server = HatariServer::new(mjj_config("watch-fires"));
    start(&server).await;

    let candidates = ["$470E8", "$4BA", "$466", "$462"];
    let mut fired: Option<WatchOutput> = None;
    for addr in candidates {
        let _ = server.resume().await; // best-effort re-arm between attempts
        let Json(out) = server
            .watch(Parameters(watch_args(addr, 4, 5_000)))
            .await
            .unwrap();
        if out.fired {
            fired = Some(out);
            break;
        }
    }

    let out = fired.expect("no watch candidate fired within its budget");
    assert_eq!(out.ours, Some(true));
    assert_eq!(out.state, "paused");
    let pc = Addr::parse(out.pc.as_deref().unwrap()).unwrap();
    assert_ne!(pc.raw(), 0);
    assert_ne!(out.new_value, Some(out.old_value.clone()));
    println!("watch writer backscan: {:?}", out.writer);

    // §3.2 `// verify live` closure: confirmed live — the tracking value
    // line ("  $<addr> = $<new>") prints BEFORE the match-report header
    // (BreakCond_UpdateTracked before BreakCond_MatchBreakPoints, as
    // pinned), and the normalized expression is dot-spaced exactly as the
    // tokenizer predicts. ("-> Track value changes..." is ARM-echo text —
    // a separate exchange `bp::Armed` never retains — not fire-block text;
    // the earlier `// verify live` note conflated the two.)
    let output = out.output.expect("fired watch must carry the entry block");
    assert!(
        output.contains("condition(s) matched"),
        "missing match-report header: {output}"
    );
    assert!(
        output.contains("Removed CPU breakpoint"),
        "missing auto-removal line: {output}"
    );
    assert!(
        output.contains(") . l ! (") && output.contains(":once"),
        "missing normalized tracking expression: {output}"
    );

    server.resume().await.unwrap();
    stop(&server).await;
    assert_no_orphan_hatari();
}

/// §9.4 item 2: a watch on untouched scratch RAM times out, leaving the
/// session Running with nothing left armed.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn watch_timeout_leaves_running_and_disarmed() {
    let server = HatariServer::new(mjj_config("watch-timeout"));
    start(&server).await;

    let Json(out) = server
        .watch(Parameters(watch_args("$78BFE", 2, 2_000)))
        .await
        .unwrap();
    assert!(!out.fired);
    assert_eq!(out.state, "running");

    let Json(list) = server
        .breakpoint(Parameters(bp_args(BreakpointAction::List)))
        .await
        .unwrap();
    assert!(list.breakpoints.unwrap().is_empty());

    stop(&server).await;
    assert_no_orphan_hatari();
}

/// §9.4 item 3: `run_frames` advances the VBL counter by exactly N, and
/// chains cleanly from an already-Paused session.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn run_frames_advances_exactly_n() {
    let server = HatariServer::new(mjj_config("run-frames"));
    start(&server).await;

    let Json(out) = server
        .run_frames(Parameters(RunFramesArgs {
            frames: 3,
            timeout_ms: None,
        }))
        .await
        .unwrap();
    assert!(out.completed, "run_frames(3) did not complete: {out:?}");
    assert_eq!(out.frames_run, Some(3));
    assert_eq!(out.vbl_after, Some(out.vbl_before + 3));
    assert_eq!(out.state, "paused");

    let Json(out2) = server
        .run_frames(Parameters(RunFramesArgs {
            frames: 1,
            timeout_ms: None,
        }))
        .await
        .unwrap();
    assert!(
        out2.completed,
        "chained run_frames(1) did not complete: {out2:?}"
    );
    assert_eq!(out2.vbl_after, Some(out.vbl_after.unwrap() + 1));

    stop(&server).await;
    assert_no_orphan_hatari();
}

/// §9.4 item 4: plant a known abs.l reference and a PC-relative `lea` into
/// scratch RAM via `patch`, then confirm `xrefs` finds both.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn xrefs_finds_planted_reference() {
    let server = HatariServer::new(mjj_config("xrefs-plant"));
    start(&server).await;
    server.pause().await.unwrap();

    // `.l` forces absolute-long addressing — vasm otherwise auto-optimizes
    // a bare numeric destination within pc-relative range (displacement
    // $FE here) to a d16(pc) move, confirmed live (the P14.md §9.4 item 4
    // `// verify live` guess was wrong: it is NOT abs.l without the
    // suffix — amended here).
    let Json(patch_out) = server
        .patch(Parameters(PatchArgs {
            addr: "$78800".to_string(),
            source: "\tmove.l $78900.l,d0\n\tlea data(pc),a0\n\trts\ndata:\tdc.l 0\n".to_string(),
        }))
        .await
        .unwrap();
    assert!(patch_out.verified, "patch write must verify: {patch_out:?}");
    assert!(
        patch_out.readback.len() >= 3,
        "expected at least 3 decoded instructions: {patch_out:?}"
    );
    let rts = &patch_out.readback[2];
    assert_eq!(rts.mnemonic, "rts");
    let rts_addr = Addr::parse(&rts.addr).unwrap();
    let data_addr = Addr::new(rts_addr.raw() + (rts.bytes_hex.len() / 2) as u32);

    let Json(abs_xr) = server
        .xrefs(Parameters(XrefsArgs {
            addr: "$78900".to_string(),
            target_len: None,
            from: Some("$78800".to_string()),
            len: Some(64),
            limit: None,
        }))
        .await
        .unwrap();
    assert!(
        abs_xr
            .hits
            .iter()
            .any(|h| h.addr == "$078800" && h.mode == "abs.l" && h.access == "read"),
        "abs.l reference not found: {abs_xr:?}"
    );

    let Json(pc_xr) = server
        .xrefs(Parameters(XrefsArgs {
            addr: data_addr.to_string(),
            target_len: None,
            from: Some("$78800".to_string()),
            len: Some(64),
            limit: None,
        }))
        .await
        .unwrap();
    assert!(
        pc_xr
            .hits
            .iter()
            .any(|h| h.mode == "pc" && h.access == "ea"),
        "pc-relative lea reference not found: {pc_xr:?}"
    );

    server.resume().await.unwrap();
    stop(&server).await;
    assert_no_orphan_hatari();
}
