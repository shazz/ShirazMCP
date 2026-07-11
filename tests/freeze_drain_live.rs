//! Live reproduction + regression proof for the ADR-11 freeze-drain bug
//! (TODO.md "pause() stray-log corrupts the first post-freeze command"):
//! entering the debugger on `pause()` emits banner/symbol-load text that,
//! for a symbol-bearing program, can be left queued in `LogTail::pending`
//! past the freeze's own confirm sentinel — corrupting the FIRST real
//! command issued after `pause()` (observed live as truncated
//! `savebin`/`memory::acquire` reads). MJJHACK.PRG (the other live tests'
//! target) carries no DRI/GST symbol table (ADR-8) and never exercises
//! this path — nor, it turns out, does the DHS "Uhane" demo that
//! originally surfaced the bug: its shipped `.PRG` is stripped the same
//! way (`symbols prg` reports "symbol table missing from the program!").
//! `tests/fixtures/symtest.s` is a minimal fixture built specifically to
//! carry REAL, loadable labels (ADR-8's "ship our own minimal
//! symbol-bearing .PRG fixture" plan) so Hatari's autoload prints a
//! genuine, non-trivial "Loaded N symbols..." banner on every debugger
//! entry — the exact condition this fix targets.
//!
//! vasm-gated (like `tests/assemble.rs`): self-skips (eprintln, not a
//! panic) without the assembler so `cargo test` stays green on machines
//! without the MJJ toolchain. The live run itself is `#[ignore]`d — needs
//! `DISPLAY` and `/usr/bin/hatari`. Run:
//! `DISPLAY=:0 cargo test --test freeze_drain_live -- --ignored --test-threads=1`.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/integration.rs for the same rationale) — a panic on unexpected
// shape/timeout against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::path::{Path, PathBuf};
use std::process::Command;

use shiraz_mcp::assemble::DEFAULT_VASM;
use shiraz_mcp::debug;
use shiraz_mcp::session::{memory, HatariConfig, HatariProcess, Session};
use shiraz_mcp::types::MemImage;

const RAM_LEN: usize = 1 << 20;
const FIXTURE_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/symtest.s");

/// vasm presence gate (mirrors `tests/assemble.rs`'s `vasm()`): self-skip
/// with an eprintln, not a panic, so plain `cargo test` (no `--ignored`)
/// stays green on machines without the MJJ toolchain — this whole file only
/// runs under `--ignored` anyway, but the gate keeps the pattern consistent
/// and gives a clear reason when a manual `--ignored` run can't proceed.
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

/// Assemble `symtest.s` straight to a TOS executable (DRI/GST symbol table
/// included by default — unlike the `-Fbin`/patch path, this wants a real
/// bootable `.PRG`, so it shells out to vasm directly rather than going
/// through `shiraz_mcp::assemble` (which only speaks `-Fbin`).
fn build_fixture_prg(vasm: &Path, auto_dir: &Path) {
    std::fs::create_dir_all(auto_dir).unwrap();
    let prg = auto_dir.join("SYMTEST.PRG");
    let status = Command::new(vasm)
        .args(["-Ftos", "-quiet", "-spaces", "-o"])
        .arg(&prg)
        .arg(FIXTURE_SRC)
        .status()
        .unwrap();
    assert!(status.success(), "vasm failed to assemble {FIXTURE_SRC}");
    assert!(prg.is_file(), "vasm did not produce {}", prg.display());
}

fn fixture_config(work_dir: &str) -> HatariConfig {
    let harddrive = PathBuf::from(work_dir).join("hd");
    HatariConfig {
        harddrive: Some(harddrive),
        work_dir: format!("{work_dir}/hatari").into(),
        ..HatariConfig::default()
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

/// `symtest.s` never returns (an infinite `bra.s loop`), so once GEMDOS HD
/// auto-run reaches it, PC parks somewhere inside RAM forever — polling for
/// "PC left TOS ROM" is a reliable readiness signal (mirrors
/// `tests/integration.rs`'s `wait_until_ready`). Tolerates (retries past) a
/// transient `Err` from `debug::registers` — under CPU contention the
/// sentinel-retry race this file's own test targets can misfire on an
/// UNRELATED earlier command too (`fifo::debug`'s `SENTINEL_RETRY_BUDGET`
/// path, not the freeze-drain fix); that is a pre-existing, separate
/// robustness gap in the sentinel protocol (tracked as follow-up work, see
/// `drain_stdin_entry_text`'s "known limitation" doc in `control.rs`), not
/// what this test is proving, so readiness-polling must not let it mask the
/// freeze-specific assertions below.
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_until_ready(proc: &mut HatariProcess) {
    for _ in 0..150 {
        if let Ok(regs) = debug::registers(proc).await {
            if regs.pc.raw() < RAM_LEN as u32 {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("symtest fixture never became ready (PC in RAM) within the poll budget");
}

fn assert_full_length(mem: &MemImage, label: &str) {
    assert_eq!(
        mem.bytes().len(),
        RAM_LEN,
        "{label}: truncated snapshot ({} of {RAM_LEN} bytes) — the freeze-drain \
         regression (a stale queued sentinel echo satisfied this command's own \
         sentinel search before Hatari finished writing it)",
        mem.bytes().len()
    );
}

/// The regression proof: freeze against a REAL symbol-bearing program, then
/// confirm (1) the FIRST post-freeze debugger command parses cleanly and
/// (2) two immediately-following whole-RAM acquisitions are both
/// full-length and byte-identical. Pre-fix, this reproduced reliably
/// (truncated snapshots for the rest of the paused episode); MJJHACK never
/// exercised it (no symbol table to trigger the autoload banner).
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and vasm"]
async fn pause_against_symbol_bearing_program_does_not_truncate_snapshots() {
    let Some(vasm) = vasm() else { return };
    let work_dir = "/tmp/shiraz-mcp-freeze-drain-live";
    build_fixture_prg(&vasm, &PathBuf::from(work_dir).join("hd/AUTO"));

    let running = Session::new(fixture_config(work_dir))
        .start()
        .await
        .unwrap();
    let mut running = running;
    wait_until_ready(running.process_mut()).await;

    let mut paused = running.pause().await.unwrap();

    // First real post-freeze command: must parse cleanly, not choke on
    // stray banner/symbol-load text left over from the freeze itself.
    let regs = debug::registers(paused.process_mut()).await.unwrap();
    assert!(regs.pc.raw() < RAM_LEN as u32, "PC must still be in RAM");

    // Two immediately-following whole-RAM snapshots: full length, and
    // byte-identical (the CPU is parked — nothing should be moving).
    let mem1 = memory::acquire(paused.process_mut()).await.unwrap();
    assert_full_length(&mem1, "first acquire");
    let mem2 = memory::acquire(paused.process_mut()).await.unwrap();
    assert_full_length(&mem2, "second acquire");
    assert_eq!(
        mem1.bytes(),
        mem2.bytes(),
        "RAM must not move while paused, and neither read may be truncated"
    );

    let running = paused.resume().await.unwrap();
    running.stop().await;
    assert_no_orphan_hatari();
}
