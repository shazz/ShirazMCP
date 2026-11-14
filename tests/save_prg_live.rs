//! P16.md §9.6 (amended per ADR-32): two `#[ignore]`d live tests.
//! `save_prg_persists_a_text_patch` proves the full repatch round trip
//! against a **staged copy** of the MJJ tree with `STRGOOSE.PRG` (confirmed
//! unpacked, real ~8.5 KB reloc table) as the sole auto-run target — the
//! sanctioned `AUTO/MJJHACK.PRG` smoke target turned out to be UPX-packed
//! (ADR-32), which `save_prg`'s header cross-check correctly rejects, so it
//! cannot demonstrate a *successful* save. `save_prg_rejects_packed_program`
//! keeps MJJHACK.PRG exercised as a live regression test of that rejection
//! path. Needs `DISPLAY`, `/usr/bin/hatari`, and the MJJ tree (read-only —
//! never written; ADR-8). Run:
//! `DISPLAY=:0 cargo test --test save_prg_live -- --ignored --test-threads=1`
//! `// verify live: the whole §4.3 basepage discovery sequence (ADR-31).`

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape/timeout against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::path::{Path, PathBuf};
use std::process::Command;

use rmcp::handler::server::wrapper::{Json, Parameters};

use shiraz_mcp::ripper::prg_file;
use shiraz_mcp::session::HatariConfig;
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::mem::{SearchMemoryArgs, WriteMemoryArgs};
use shiraz_mcp::tools::save_prg::{RepatchOutput, SavePrgArgs, SavePrgOutput};
use shiraz_mcp::tools::HatariServer;
use shiraz_mcp::types::Addr;

const MJJ_BUILD: &str = "/path/to/projects/MJJ/build";
const MJJHACK_PRG: &str = "/path/to/projects/MJJ/build/AUTO/MJJHACK.PRG";
const SCROLL_PATTERN: &str = "PLEASE, READ ALL THIS SCROLL";
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

/// `Json<T>` has no `Debug` impl (rmcp's wrapper doesn't derive it), so
/// `Result::unwrap_err` can't be used directly on tool-call results.
#[allow(clippy::panic, reason = "integration test, not library code")]
fn expect_err<T>(r: Result<T, rmcp::ErrorData>) -> rmcp::ErrorData {
    match r {
        Ok(_) => panic!("expected an error result"),
        Err(e) => e,
    }
}

/// mode="repatch" always yields the `Repatch` variant; unwrap it (these
/// tests never call mode="snapshot").
#[allow(clippy::panic, reason = "integration test, not library code")]
fn expect_repatch(out: SavePrgOutput) -> RepatchOutput {
    match out {
        SavePrgOutput::Repatch(r) => r,
        SavePrgOutput::Snapshot(_) => panic!("expected the Repatch variant"),
    }
}

#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_until_ready(server: &HatariServer) {
    for _ in 0..50 {
        let Json(r) = server.registers().await.unwrap();
        if Addr::parse(&r.pc).is_ok_and(|pc| pc.raw() < RAM_LEN) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("program never became ready (PC in RAM) within the poll budget");
}

/// Recursive copy — no external dependency for a one-off ~1 MB test fixture copy.
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// ADR-32: stage a read-only-source copy of the MJJ tree (root data files
/// included — the demo GEMDOS-loads them) with `AUTO/MJJHACK.PRG` removed,
/// so the confirmed-unpacked `STRGOOSE.PRG` becomes the sole auto-run
/// target. The MJJ tree itself is never written.
fn stage_strgoose_harddrive(work_dir: &Path) -> PathBuf {
    let hd = work_dir.join("hd");
    let _ = std::fs::remove_dir_all(&hd);
    copy_dir_all(Path::new(MJJ_BUILD), &hd).unwrap();
    std::fs::remove_file(hd.join("AUTO").join("MJJHACK.PRG")).unwrap();
    hd
}

fn scroll_search_args() -> SearchMemoryArgs {
    SearchMemoryArgs {
        pattern_hex: None,
        pattern_ascii: Some(SCROLL_PATTERN.to_string()),
        start: None,
        end: None,
        limit: Some(1),
        offset: None,
    }
}

/// Poll live (Running) — a paused session can't make progress — until the
/// scroller text is resident in RAM.
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_for_scroll_text(server: &HatariServer) {
    for _ in 0..100 {
        let Json(found) = server
            .search_memory(Parameters(scroll_search_args()))
            .await
            .unwrap();
        if !found.hits.is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("scroller text never appeared in RAM within the poll budget");
}

/// Byte-compare `out_bytes` against `source_bytes`; return the (offset, len)
/// spans that differ.
fn diff_spans(source_bytes: &[u8], out_bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < source_bytes.len() {
        if source_bytes[i] == out_bytes[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < source_bytes.len() && source_bytes[i] != out_bytes[i] {
            i += 1;
        }
        spans.push((start, i - start));
    }
    spans
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn save_prg_persists_a_text_patch() {
    let work_dir: PathBuf = "/tmp/shiraz-mcp-itest-save-prg-hack".into();
    let hd = stage_strgoose_harddrive(&work_dir);
    let source_prg = hd.join("AUTO").join("STRGOOSE.PRG");
    let server = HatariServer::new(HatariConfig {
        harddrive: Some(hd),
        work_dir: work_dir.join("mcp"),
        ..HatariConfig::default()
    });
    server.start(Parameters(no_args())).await.unwrap();
    wait_until_ready(&server).await;
    wait_for_scroll_text(&server).await;

    let out_path = work_dir.join("hacked.prg");
    let save_args = || SavePrgArgs {
        out_path: out_path.display().to_string(),
        source_prg: Some(source_prg.display().to_string()),
        mode: None,
        regions: None,
    };

    // Gate: save_prg while Running is rejected.
    let err = expect_err(server.save_prg(Parameters(save_args())).await);
    assert!(err.message.contains("paused"));

    server.pause().await.unwrap();

    let Json(found) = server
        .search_memory(Parameters(scroll_search_args()))
        .await
        .unwrap();
    assert!(!found.hits.is_empty(), "scroller text not found in RAM");
    let hit_addr = found.hits[0].addr.clone();

    let patch = b"SHIRAZMCP!";
    let hex: String = patch.iter().map(|b| format!("{b:02x}")).collect();
    server
        .write_memory(Parameters(WriteMemoryArgs {
            addr: hit_addr.clone(),
            bytes_hex: hex,
        }))
        .await
        .unwrap();

    let Json(out): Json<SavePrgOutput> = server.save_prg(Parameters(save_args())).await.unwrap();
    let out = expect_repatch(out);
    assert!(out.changed_bytes >= patch.len());
    let run = out
        .applied
        .iter()
        .find(|r| r.addr == hit_addr)
        .expect("no applied run at the patched address");
    assert_eq!(run.len, patch.len());

    let source = prg_file::read_prg_file(&source_prg).unwrap();
    let parsed = prg_file::read_prg_file(&out_path).unwrap();
    assert_eq!(parsed.header, source.header);
    assert_eq!(parsed.bytes.len(), source.bytes.len());
    let fo = run.file_offset;
    assert_eq!(&parsed.bytes[fo..fo + patch.len()], patch);

    // The (staged, disposable) source file itself was never touched.
    let source_disk = std::fs::read(&source_prg).unwrap();
    assert_eq!(source_disk, source.bytes);

    // Exactly the applied runs differ from the source (modulo the cap).
    let spans = diff_spans(&source.bytes, &parsed.bytes);
    if out.applied_truncated {
        assert!(spans.len() > out.applied.len());
    } else {
        assert_eq!(spans.len(), out.applied.len());
    }
    assert!(spans
        .iter()
        .any(|&(off, len)| off == fo && len == patch.len()));

    server.resume().await.unwrap();
    server.stop().await.unwrap();
    assert_no_orphan_hatari();
}

/// ADR-32: `AUTO/MJJHACK.PRG` (the ADR-8 smoke target) is UPX-packed — a
/// live regression test of the §7.1/§12 packed-program safety net: the
/// Paused gate, live basepage discovery and RAM acquire all succeed, and
/// `save_prg` still refuses to (mis)save a packed program, loudly.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn save_prg_rejects_packed_program() {
    let work_dir: PathBuf = "/tmp/shiraz-mcp-itest-save-prg-packed".into();
    let server = HatariServer::new(HatariConfig {
        harddrive: Some(MJJ_BUILD.into()),
        work_dir: work_dir.clone(),
        ..HatariConfig::default()
    });
    server.start(Parameters(no_args())).await.unwrap();
    wait_until_ready(&server).await;

    let save_args = || SavePrgArgs {
        out_path: work_dir.join("should-not-exist.prg").display().to_string(),
        source_prg: Some(MJJHACK_PRG.to_string()),
        mode: None,
        regions: None,
    };
    let err = expect_err(server.save_prg(Parameters(save_args())).await);
    assert!(err.message.contains("paused"));

    server.pause().await.unwrap();
    let err = expect_err(server.save_prg(Parameters(save_args())).await);
    assert!(
        err.message.contains("does not match"),
        "expected the packed-program cross-check rejection, got: {}",
        err.message
    );

    server.resume().await.unwrap();
    server.stop().await.unwrap();
    assert_no_orphan_hatari();
}
