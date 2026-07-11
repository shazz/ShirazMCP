//! P15.md §9.4: live behavioral checks for `cheat_scan`, driven through the
//! PUBLIC tool surface (mirrors `tests/analyze_live.rs`'s session-bootstrap
//! and cleanup shape). Needs `DISPLAY`, `/usr/bin/hatari`, and the MJJ tree
//! — `#[ignore]`d; run with `DISPLAY=:0 cargo test --test cheat_live --
//! --ignored --test-threads=1`.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape/timeout against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::process::Command;
use std::time::Duration;

use rmcp::handler::server::wrapper::{Json, Parameters};

use shiraz_mcp::session::HatariConfig;
use shiraz_mcp::tools::cheat::CheatScanArgs;
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::HatariServer;

/// Per-test work dirs (P1.md §10.4 rationale, reused by every live suite):
/// a panicking test's `HatariProcess` only gets RAII `kill_on_drop`, not the
/// graceful `stop()`, and a following test's fresh spawn reusing the same
/// fifo/log paths can race that teardown.
fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: format!("/tmp/shiraz-mcp-cheat-live-{name}").into(),
        ..HatariConfig::default()
    }
}

fn start_args(memsize_mb: Option<u32>) -> StartArgs {
    StartArgs {
        hatari_path: None,
        tos_path: None,
        harddrive: None,
        machine: None,
        memsize_mb,
    }
}

async fn start(server: &HatariServer) {
    server.start(Parameters(start_args(None))).await.unwrap();
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

/// Let the machine run for `ms` real milliseconds: `cheat_scan` never
/// changes session state (§3.6), so narrowing steps bracket their own
/// resume/pause around it — mirrors run_frames/resume+pause per the tool
/// description, without depending on P14's `run_frames`.
async fn run_for(server: &HatariServer, ms: u64) {
    server.resume().await.unwrap();
    tokio::time::sleep(Duration::from_millis(ms)).await;
    server.pause().await.unwrap();
}

fn cheat_args(action: &str) -> CheatScanArgs {
    CheatScanArgs {
        action: action.to_string(),
        predicate: None,
        value: None,
        width: None,
        start: None,
        end: None,
        limit: None,
        offset: None,
    }
}

async fn cheat(
    server: &HatariServer,
    a: CheatScanArgs,
) -> shiraz_mcp::tools::cheat::CheatScanOutput {
    let Json(out) = server.cheat_scan(Parameters(a)).await.unwrap();
    out
}

/// `Json<CheatScanOutput>` has no `Debug` impl (rmcp's wrapper doesn't
/// derive it), so `Result::unwrap_err` can't be used directly (the
/// `tests/assemble.rs` precedent).
#[allow(clippy::panic, reason = "integration test, not library code")]
fn expect_err<T>(r: Result<T, rmcp::ErrorData>) -> rmcp::ErrorData {
    match r {
        Ok(_) => panic!("expected an error result"),
        Err(e) => e,
    }
}

/// §9.4 item 1 (the headline): narrow the `$400-$500` sysvar region by
/// changed+increased and find `_hz_200` ($4BA, a 200 Hz up-counter — long,
/// increments every 5 ms, so it survives both filters) or its documented
/// fallback `_frclock` ($466). `// verify live`: confirmed live against the
/// MJJ demo — `_hz_200` ($4BA) holds; TOS Timer C stays armed as expected,
/// the `_frclock` fallback was not needed.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn cheat_finds_ticking_sysvars_in_bounded_region() {
    let server = HatariServer::new(mjj_config("cheat-sysvars"));
    start(&server).await;
    server.pause().await.unwrap();

    let new_out = cheat(
        &server,
        CheatScanArgs {
            width: Some(4),
            start: Some("$400".to_string()),
            end: Some("$500".to_string()),
            ..cheat_args("new")
        },
    )
    .await;
    assert!(new_out.count.unwrap_or(0) > 0);
    assert_eq!(new_out.step, Some(0));

    run_for(&server, 300).await;
    let f1 = cheat(
        &server,
        CheatScanArgs {
            predicate: Some("changed".to_string()),
            ..cheat_args("filter")
        },
    )
    .await;
    assert!(f1.count.unwrap() <= new_out.count.unwrap());
    assert!(f1.count.unwrap() > 0);

    run_for(&server, 300).await;
    let f2 = cheat(
        &server,
        CheatScanArgs {
            predicate: Some("increased".to_string()),
            ..cheat_args("filter")
        },
    )
    .await;
    assert!(f2.count.unwrap() >= 1);

    let listed = cheat(
        &server,
        CheatScanArgs {
            limit: Some(64),
            ..cheat_args("list")
        },
    )
    .await;
    for c in &listed.sample {
        assert!(c.value.starts_with('$'));
        assert_eq!(c.value.len(), 9, "expected $ + 8 hex digits: {}", c.value);
    }
    let hit = listed
        .sample
        .iter()
        .find(|c| c.addr == "$0004BA")
        .or_else(|| listed.sample.iter().find(|c| c.addr == "$000466"))
        .expect("neither _hz_200 ($4BA) nor _frclock ($466) survived changed+increased");
    assert!(!hit.annotations.is_empty());

    server.resume().await.unwrap();
    stop(&server).await;
    assert_no_orphan_hatari();
}

/// §9.4 item 2: whole-RAM narrowing (default width=2) is monotone
/// non-increasing across successive `changed` filters, `prev_count` echoes
/// the prior step's `count`, and `step` advances by one per filter.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn cheat_whole_ram_narrowing_monotone() {
    let server = HatariServer::new(mjj_config("cheat-whole-ram"));
    start(&server).await;
    server.pause().await.unwrap();

    let new_out = cheat(&server, cheat_args("new")).await;
    let c0 = new_out.count.unwrap();

    run_for(&server, 200).await;
    let f1 = cheat(
        &server,
        CheatScanArgs {
            predicate: Some("changed".to_string()),
            ..cheat_args("filter")
        },
    )
    .await;
    assert_eq!(f1.prev_count, Some(c0));
    assert!(f1.count.unwrap() <= c0);

    run_for(&server, 200).await;
    let f2 = cheat(
        &server,
        CheatScanArgs {
            predicate: Some("changed".to_string()),
            ..cheat_args("filter")
        },
    )
    .await;
    assert_eq!(f2.prev_count, f1.count);
    assert!(f2.count.unwrap() <= f1.count.unwrap());
    assert!(
        f2.count.unwrap() > 0,
        "the screen buffer alone guarantees survivors"
    );
    assert_eq!(f2.step, Some(2));

    server.resume().await.unwrap();
    stop(&server).await;
    assert_no_orphan_hatari();
}

/// §9.4 item 3: `list`/`reset` are pure over the store — `list` still
/// answers after `stop`, `reset` reports whether a search existed to clear.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn cheat_list_and_reset_without_session() {
    let server = HatariServer::new(mjj_config("cheat-list-reset"));
    start(&server).await;
    server.pause().await.unwrap();

    cheat(
        &server,
        CheatScanArgs {
            start: Some("$400".to_string()),
            end: Some("$500".to_string()),
            ..cheat_args("new")
        },
    )
    .await;
    run_for(&server, 200).await;
    cheat(
        &server,
        CheatScanArgs {
            predicate: Some("changed".to_string()),
            ..cheat_args("filter")
        },
    )
    .await;

    server.resume().await.unwrap();
    stop(&server).await;

    let listed = cheat(&server, cheat_args("list")).await;
    assert_eq!(listed.action, "list");

    let reset1 = cheat(&server, cheat_args("reset")).await;
    assert_eq!(reset1.cleared, Some(true));

    let err = expect_err(server.cheat_scan(Parameters(cheat_args("list"))).await);
    assert!(err.message.contains("no cheat search in memory"));

    let reset2 = cheat(&server, cheat_args("reset")).await;
    assert_eq!(reset2.cleared, Some(false));

    assert_no_orphan_hatari();
}

/// §9.4 item 4: a `filter` against a session restarted with a different
/// memsize is rejected — the stored snapshot's geometry no longer matches.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn cheat_filter_rejects_stale_geometry() {
    let server = HatariServer::new(mjj_config("cheat-geometry"));
    server.start(Parameters(start_args(Some(1)))).await.unwrap();
    server.pause().await.unwrap();
    cheat(&server, cheat_args("new")).await;
    server.resume().await.unwrap();
    stop(&server).await;

    server.start(Parameters(start_args(Some(4)))).await.unwrap();
    server.pause().await.unwrap();

    let err = expect_err(
        server
            .cheat_scan(Parameters(CheatScanArgs {
                predicate: Some("changed".to_string()),
                ..cheat_args("filter")
            }))
            .await,
    );
    assert!(err.message.contains("snapshot geometry changed"));

    server.resume().await.unwrap();
    stop(&server).await;
    assert_no_orphan_hatari();
}
