//! P26.md §10.2: live behavioral check for `symbols` + label-name
//! addressing in `disasm`/`breakpoint`, driven through the PUBLIC tool
//! surface — the `tests/analyze_live.rs`/`tests/profile_frame_live.rs`
//! pattern exactly. Needs `DISPLAY`, `/usr/bin/hatari`, and the MJJ tree —
//! `#[ignore]`d; run with `DISPLAY=:0 cargo test --test symbols_live --
//! --ignored --test-threads=1`.
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
use shiraz_mcp::tools::analyze::{DisasmArgs, DisasmSource};
use shiraz_mcp::tools::breakpoint::{BreakpointAction, BreakpointArgs};
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::symbols::{SymListKind, SymbolsAction, SymbolsArgs};
use shiraz_mcp::tools::HatariServer;
use shiraz_mcp::types::Addr;

/// The GEMDOS-HD tree this test boots. The committed default is the
/// placeholder the public-release PII scrub left in every live test; a
/// local checkout points at its own tree via `SHIRAZMCP_TEST_HD`.
fn test_hd() -> String {
    std::env::var("SHIRAZMCP_TEST_HD").unwrap_or_else(|_| "/path/to/projects/MJJ/build".to_string())
}

fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some(test_hd().into()),
        work_dir: format!("/tmp/shiraz-mcp-symbols-live-{name}").into(),
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

fn load_args(path: Option<&str>, text_offset: Option<&str>) -> SymbolsArgs {
    SymbolsArgs {
        action: SymbolsAction::Load,
        path: path.map(str::to_string),
        text_offset: text_offset.map(str::to_string),
        data_offset: None,
        bss_offset: None,
        kind: None,
        find: None,
        limit: None,
        offset: None,
    }
}

fn disasm_args(addr: &str) -> DisasmArgs {
    DisasmArgs {
        addr: addr.to_string(),
        count: Some(1),
        source: Some(DisasmSource::Snapshot),
    }
}

#[tokio::test]
#[ignore = "needs DISPLAY, hatari, and the MJJ tree"]
async fn symbols_load_list_disasm_breakpoint_free_flow() {
    let server = HatariServer::new(mjj_config("full-flow"));
    start(&server).await;
    wait_until_ready(&server).await;

    // Step 2: MJJ has no symbol table (ADR-8) — degrades cleanly.
    let Json(no_syms) = server
        .symbols(Parameters(load_args(None, None)))
        .await
        .unwrap();
    let loaded = no_syms.loaded.as_ref().unwrap();
    assert_eq!(loaded.total, 0);
    assert!(loaded.note.is_some());

    // Step 3: write a tiny nm-style ASCII map.
    let work_dir = "/tmp/shiraz-mcp-symbols-live-full-flow";
    std::fs::create_dir_all(work_dir).unwrap();
    let sym_path = format!("{work_dir}/p26.sym");
    std::fs::write(&sym_path, "# P26 live map\n0 T P26entry\n100 D P26data\n").unwrap();

    // Step 4: explicit offset load.
    let Json(explicit) = server
        .symbols(Parameters(load_args(Some(&sym_path), Some("$70000"))))
        .await
        .unwrap();
    let loaded = explicit.loaded.as_ref().unwrap();
    assert_eq!(loaded.base_source, "explicit");
    assert!(loaded.listable);
    assert_eq!(loaded.total, 2);

    let Json(d) = server
        .disasm(Parameters(disasm_args("P26entry")))
        .await
        .unwrap();
    assert_eq!(d.insns[0].addr, "$070000");

    // Steps 5-9: list (filtered/by-kind), auto-base reload, symbolic
    // breakpoint arm, unresolvable-name error, free.
    symbols_list_and_breakpoint_and_free(&server, &sym_path).await;

    stop(&server).await;
    assert_no_orphan_hatari();
}

async fn symbols_list_and_breakpoint_and_free(server: &HatariServer, sym_path: &str) {
    let Json(list_all) = server
        .symbols(Parameters(SymbolsArgs {
            action: SymbolsAction::List,
            find: Some("p26".to_string()),
            ..load_args(None, None)
        }))
        .await
        .unwrap();
    assert_eq!(list_all.total_matches, Some(2));

    let Json(list_code) = server
        .symbols(Parameters(SymbolsArgs {
            action: SymbolsAction::List,
            kind: Some(SymListKind::Code),
            ..load_args(None, None)
        }))
        .await
        .unwrap();
    assert_eq!(list_code.total_matches, Some(1));

    // Step 6: auto-base reload of the same file.
    let Json(auto) = server
        .symbols(Parameters(load_args(Some(sym_path), None)))
        .await
        .unwrap();
    let loaded = auto.loaded.as_ref().unwrap();
    assert_eq!(loaded.base_source, "basepage");

    // Step 7: symbolic breakpoint arm (timeout expected — no code writes
    // P26data; the assertion is that arming the symbol succeeded).
    let Json(bp) = server
        .breakpoint(Parameters(BreakpointArgs {
            action: BreakpointAction::Set,
            condition: Some("pc = P26data".to_string()),
            timeout_ms: Some(1_500),
            index: None,
        }))
        .await
        .unwrap();
    assert_eq!(bp.fired, Some(false));
    server
        .breakpoint(Parameters(BreakpointArgs {
            action: BreakpointAction::Clear,
            condition: None,
            timeout_ms: None,
            index: None,
        }))
        .await
        .unwrap();

    // Step 8: unresolvable name.
    let err = server
        .disasm(Parameters(disasm_args("NoSuchLabel")))
        .await
        .err()
        .unwrap();
    assert!(err.message.contains("symbols(action=\"load\")"));

    // Step 9: free.
    server
        .symbols(Parameters(SymbolsArgs {
            action: SymbolsAction::Free,
            ..load_args(None, None)
        }))
        .await
        .unwrap();
    let err = server
        .disasm(Parameters(disasm_args("P26entry")))
        .await
        .err()
        .unwrap();
    assert!(!err.message.is_empty());
    let err = server
        .symbols(Parameters(SymbolsArgs {
            action: SymbolsAction::List,
            ..load_args(None, None)
        }))
        .await
        .err()
        .unwrap();
    assert!(err.message.contains("symbols(action=\"load\")"));
}

/// Grep-gate (P26 §2.3): the pager-hazard listing command must never be
/// sent from anywhere in the crate.
#[test]
fn symbols_listing_pager_grep_gate() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    assert!(!files.is_empty(), "src walk found no .rs files");
    for f in files {
        let content = std::fs::read_to_string(&f).unwrap();
        for needle in ["\"symbols name", "\"symbols code", "\"symbols data"] {
            assert!(
                !content.contains(needle),
                "banned pager-hazard string {needle:?} found in {f:?}"
            );
        }
    }
}

fn collect_rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}
