//! P8.md §10: vasm-gated goldens (self-skip without vasm, not `#[ignore]`d)
//! plus the `#[ignore]`d live `patch` round-trip.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape/timeout is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::path::PathBuf;
use std::process::Command;

use rmcp::handler::server::wrapper::{Json, Parameters};

use shiraz_mcp::assemble::source::prepare_source;
use shiraz_mcp::assemble::{assemble, DEFAULT_VASM};
use shiraz_mcp::session::HatariConfig;
use shiraz_mcp::tools::assemble::{AssembleArgs, PatchArgs};
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::mem::{ReadMemoryArgs, ReadMemoryOutput};
use shiraz_mcp::tools::HatariServer;
use shiraz_mcp::types::Addr;

/// vasm presence gate: the pinned default or the env override. Tests that
/// drive the real assembler SELF-SKIP (with an eprintln) when absent, so
/// `cargo test` stays green on machines without the MJJ toolchain — the
/// pure §9 tests still cover arg-building/parse/temp logic there.
fn vasm() -> Option<PathBuf> {
    let p = std::env::var("SHIRAZMCP_VASM")
        .map(PathBuf::from)
        .unwrap_or_else(|_| DEFAULT_VASM.into());
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("skipping: vasm not found at {}", p.display());
        None
    }
}

#[tokio::test]
async fn golden_move_w_palette() {
    let Some(vasm) = vasm() else { return };
    let prepared = prepare_source("\tmove.w #$777,$ffff8240\n", Some(Addr::new(0x70000))).unwrap();
    let r = assemble(&vasm, &prepared, false).await.unwrap();
    assert!(r.ok, "errors: {:?}", r.errors);
    assert_eq!(r.bytes, vec![0x31, 0xFC, 0x07, 0x77, 0x82, 0x40]);
    assert!(r.warnings.is_empty());
}

#[tokio::test]
async fn golden_loop_12_bytes() {
    let Some(vasm) = vasm() else { return };
    let src = "loop:\tmove.w #$777,$ffff8240\n\tsubq.w #1,d0\n\tbne.s loop\n\trts\n";
    let prepared = prepare_source(src, Some(Addr::new(0x70000))).unwrap();
    let r = assemble(&vasm, &prepared, false).await.unwrap();
    assert!(r.ok, "errors: {:?}", r.errors);
    assert_eq!(
        r.bytes,
        vec![0x31, 0xFC, 0x07, 0x77, 0x82, 0x40, 0x53, 0x40, 0x66, 0xF6, 0x4E, 0x75]
    );
}

#[tokio::test]
async fn golden_no_org() {
    let Some(vasm) = vasm() else { return };
    let prepared = prepare_source("\tmove.w #$777,$ffff8240\n\trts\n", None).unwrap();
    let r = assemble(&vasm, &prepared, false).await.unwrap();
    assert!(r.ok, "errors: {:?}", r.errors);
    assert_eq!(
        r.bytes,
        vec![0x31, 0xFC, 0x07, 0x77, 0x82, 0x40, 0x4E, 0x75]
    );
}

#[tokio::test]
async fn error_undefined_symbol() {
    let Some(vasm) = vasm() else { return };
    let prepared =
        prepare_source("\tmove.w #$777,bogus_label\n", Some(Addr::new(0x70000))).unwrap();
    let r = assemble(&vasm, &prepared, false).await.unwrap();
    assert!(!r.ok);
    assert!(r.bytes.is_empty());
    assert_eq!(r.errors.len(), 1);
    assert!(r.errors[0].contains("undefined symbol"));
    assert!(r.errors[0].contains("bogus_label"));
    assert!(
        !r.errors[0].contains("shirazmcp-asm"),
        "temp path leaked: {}",
        r.errors[0]
    );
}

#[tokio::test]
async fn warning_auto_align() {
    let Some(vasm) = vasm() else { return };
    let prepared = prepare_source("\tnop\n", Some(Addr::new(0x70001))).unwrap();
    let r = assemble(&vasm, &prepared, false).await.unwrap();
    assert!(r.ok, "errors: {:?}", r.errors);
    assert_eq!(r.warnings.len(), 1);
    assert!(r.warnings[0].contains("auto-aligned"));
}

#[tokio::test]
async fn listing_returned() {
    let Some(vasm) = vasm() else { return };
    let src = "loop:\tmove.w #$777,$ffff8240\n\tsubq.w #1,d0\n\tbne.s loop\n\trts\n";
    let prepared = prepare_source(src, Some(Addr::new(0x70000))).unwrap();
    let r = assemble(&vasm, &prepared, true).await.unwrap();
    assert!(r.ok, "errors: {:?}", r.errors);
    let listing = r.listing.unwrap();
    assert!(listing.contains("31FC07778240"));
    assert!(listing.contains("source"));
    assert!(listing.len() <= shiraz_mcp::assemble::source::LISTING_MAX_BYTES + 64);
}

/// `Json<PatchOutput>` has no `Debug` impl (rmcp's wrapper doesn't derive
/// it), so `Result::unwrap_err` can't be used directly on tool-call results.
#[allow(clippy::panic, reason = "integration test, not library code")]
fn expect_err<T>(r: Result<T, rmcp::ErrorData>) -> rmcp::ErrorData {
    match r {
        Ok(_) => panic!("expected an error result"),
        Err(e) => e,
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

#[tokio::test]
async fn assemble_tool_surface() {
    let Some(_vasm) = vasm() else { return };
    let server = HatariServer::new(HatariConfig::default());
    let Json(out) = server
        .assemble(Parameters(AssembleArgs {
            source: "\tmove.w #$777,$ffff8240\n".to_string(),
            org: Some("$70000".to_string()),
            listing: None,
        }))
        .await
        .unwrap();
    assert!(out.ok, "errors: {:?}", out.errors);
    assert_eq!(out.bytes_hex, "31fc07778240");
    assert_eq!(out.length, 6);
}

#[tokio::test]
async fn patch_without_session_wrong_state() {
    let Some(_vasm) = vasm() else { return };
    let server = HatariServer::new(HatariConfig::default());
    let err = expect_err(
        server
            .patch(Parameters(PatchArgs {
                addr: "$70000".to_string(),
                source: "\trts\n".to_string(),
            }))
            .await,
    );
    assert!(err.message.contains("stopped"));
}

#[tokio::test]
async fn patch_odd_addr_invalid() {
    let server = HatariServer::new(HatariConfig::default());
    let err = expect_err(
        server
            .patch(Parameters(PatchArgs {
                addr: "$70001".to_string(),
                source: "\trts\n".to_string(),
            }))
            .await,
    );
    assert!(err.message.contains("even"));
}

#[tokio::test]
async fn patch_org_in_source_invalid() {
    let server = HatariServer::new(HatariConfig::default());
    let err = expect_err(
        server
            .patch(Parameters(PatchArgs {
                addr: "$70000".to_string(),
                source: "\torg $100\n\trts\n".to_string(),
            }))
            .await,
    );
    assert!(err.message.contains("org/rorg"));
}

// ---- §10.3: live patch round-trip ----

fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: format!("/tmp/shiraz-mcp-assemble-live-{name}").into(),
        ..HatariConfig::default()
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

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, vasm, and the MJJ tree"]
async fn patch_frozen_demo_roundtrip() {
    let Some(_vasm) = vasm() else { return };
    let server = HatariServer::new(mjj_config("patch"));
    start(&server).await;
    wait_until_ready(&server).await;
    server.pause().await.unwrap(); // freeze first — the recommended flow

    // Scratch RAM well above the MJJ demo's load region, inside 1 MB.
    let Json(out) = server
        .patch(Parameters(PatchArgs {
            addr: "$78000".to_string(),
            source: "\tmove.w #$777,$ffff8240\n\trts\n".to_string(),
        }))
        .await
        .unwrap();
    assert!(out.assembled && out.verified && out.ok);
    assert_eq!(out.length, 8);
    assert_eq!(out.end_addr, "$078008");
    assert_eq!(out.readback[0].mnemonic, "move.w");
    assert_eq!(out.readback[1].mnemonic, "rts");
    assert!(!out.readback_truncated);

    // Independent confirmation through a second tool: read_memory sees the bytes.
    let Json(rm): Json<ReadMemoryOutput> = server
        .read_memory(Parameters(ReadMemoryArgs {
            addr: "$78000".to_string(),
            count: Some(8),
        }))
        .await
        .unwrap();
    assert_eq!(rm.bytes_hex, "31fc077782404e75");

    server.resume().await.unwrap(); // thaw cleanly (ADR-11)
    stop(&server).await;
    assert_no_orphan_hatari();
}

/// Regression for the silent-truncation bug (`debug::MEMWRITE_CHUNK_BYTES`):
/// a >64-byte `dc.b` blob spans MULTIPLE `memwrite` lines under the fixed
/// chunking. Before the fix this either reported `verified:false` (bytes
/// never landed) or — worse — `verified:true`/`ok:true` while RAM held
/// nothing, because Hatari's "too many arguments" rejection line didn't
/// match the old case-sensitive `ERROR` check.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, vasm, and the MJJ tree"]
async fn patch_large_blob_roundtrip_no_truncation() {
    let Some(_vasm) = vasm() else { return };
    let server = HatariServer::new(mjj_config("patch-large"));
    start(&server).await;
    wait_until_ready(&server).await;
    server.pause().await.unwrap();

    // 130 bytes: well above the 61-byte hard Hatari line limit and the
    // 32-byte chunk size alike, so this exercises >=5 chunked memwrites.
    let pattern: Vec<u8> = (0..130u32).map(|i| ((i * 37 + 11) % 256) as u8).collect();
    let dc_b_lines: String = pattern
        .chunks(16)
        .map(|c| {
            let vals: Vec<String> = c.iter().map(|b| format!("${b:02x}")).collect();
            format!("\tdc.b {}\n", vals.join(","))
        })
        .collect();

    let Json(out) = server
        .patch(Parameters(PatchArgs {
            addr: "$78100".to_string(),
            source: dc_b_lines,
        }))
        .await
        .unwrap();
    assert!(out.assembled, "assembly failed: {:?}", out.errors);
    assert_eq!(out.length, pattern.len() as u32);
    assert!(out.verified, "patch verify=false: bytes never landed");
    assert!(out.ok);

    let Json(rm): Json<ReadMemoryOutput> = server
        .read_memory(Parameters(ReadMemoryArgs {
            addr: "$78100".to_string(),
            count: Some(pattern.len() as u32),
        }))
        .await
        .unwrap();
    let expected_hex: String = pattern.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(rm.bytes_hex, expected_hex, "readback is not byte-identical");

    server.resume().await.unwrap();
    stop(&server).await;
    assert_no_orphan_hatari();
}
