//! The MJJ-demo end-to-end test (P1.md §10.4). Needs `DISPLAY`,
//! `/usr/bin/hatari`, and the MJJ tree — `#[ignore]`d; run with
//! `DISPLAY=:0 cargo test --test integration -- --ignored --test-threads=1`.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape/timeout against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use shiraz_mcp::ripper::default_registry;
use shiraz_mcp::ripper::image::{decode_planar, palette_words, png_bytes, Palette, ScreenRes};
use shiraz_mcp::session::{memory, HatariConfig, HatariProcess, Paused, Running, Session};
use shiraz_mcp::types::Addr;
use shiraz_mcp::{analyze, debug};

// P1.md §10.4 specifies one shared `work_dir` for both tests; empirically,
// when a preceding test panics (e.g. `pause_freezes_pc` against the known
// unresolved `hatari-stop` issue -- ADR-9 addendum) its `HatariProcess` only
// gets the RAII `kill_on_drop` teardown, not the graceful `stop()`, and a
// following test's fresh `spawn()` reusing the exact same fifo/log paths
// occasionally races that teardown. Per-test work dirs remove the coupling
// entirely at zero cost (each test already owns its own Hatari process).
fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: format!("/tmp/shiraz-mcp-itest-{name}").into(),
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

/// RAM size for `mjj_config` (`HatariConfig::default().memsize_mb` == 1).
const RAM_LEN: u32 = 1 << 20;

// Bounded poll (NOT the boot gate -- that stays sleep-free per P1.md
// section 10.4 item 1) for the GEMDOS HD auto-run to actually start
// MJJHACK.PRG *and* reach its own RAM-resident code. The sentinel-gated
// boot only proves the debugger control plane is live; it fires as soon as
// Hatari answers, which in practice is still inside TOS ROM, before GEMDOS
// has launched anything -- `symbols prg` replies "no program loaded
// (through GEMDOS HD emu)!" until that happens (a third `symbols prg` shape
// P1.md's fixture never exercised, so `parse_symbols` correctly treats it
// as a parse failure and this loop retries past it). Even once that clears,
// PC can still transiently sit in TOS ROM ($E00000+, outside the acquired
// RAM window) for a moment longer, so this also waits for PC to fall inside
// `RAM_LEN` before handing back a `Registers` the rest of the test can
// trust.
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_until_ready(proc: &mut HatariProcess) -> debug::Registers {
    for _ in 0..50 {
        let symbols = debug::load_symbols(proc).await;
        let regs = debug::registers(proc).await.unwrap();
        if symbols.is_ok() && regs.pc.raw() < RAM_LEN {
            return regs;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("MJJHACK.PRG never became ready (loaded + PC in RAM) within the poll budget")
}

async fn assert_palette_annotation(proc: &mut HatariProcess) {
    let bytes = debug::read_bytes(proc, Addr::new(0xFF8240), 32)
        .await
        .unwrap();
    assert_eq!(bytes.len(), 32);
    let a = analyze::annotate(Addr::new(0xFF8240), None).unwrap();
    assert_eq!(a.register, "Shifter");
    assert_eq!(a.detail, "palette register 0");
}

// MJJHACK.PRG is UPX-packed (confirmed live: `strings` on the file shows
// the UPX banner, and a RAM dump taken as little as 0.6s after boot -- deep
// in fast-forward, i.e. many emulated frames -- never contains the file's
// on-disk TEXT bytes at any offset). P1.md's literal needle ("the first 14
// bytes of MJJHACK.PRG at file offset 28 ... loaded verbatim into RAM")
// assumes an unpacked executable and does not hold for this build: GEMDOS's
// raw copy is overwritten by the UPX stub's own decompression essentially
// immediately. A needle fetched via a SEPARATE live `debug::read_bytes`
// round-trip still races the depacker (emulation keeps advancing across the
// extra fifo round-trip); slicing the needle straight out of the ALREADY-
// acquired snapshot removes the race entirely while still exercising the
// exact `memchr::memmem` code path `search_memory` uses over a real,
// realistic 1 MiB image.
async fn assert_acquire_and_search(proc: &mut HatariProcess, pc: Addr) {
    let mem = memory::acquire(proc).await.unwrap();
    assert_eq!(mem.len(), 0x100000);
    assert_eq!(mem.base(), Addr::new(0));
    assert!(mem.bytes()[0x400..0x600].iter().any(|&b| b != 0));

    let pc_off = pc.raw() as usize;
    let needle = &mem.bytes()[pc_off..pc_off + 14];
    assert!(memchr::memmem::find(mem.bytes(), needle).is_some());
}

async fn assert_screenshot(proc: &mut HatariProcess, work_dir: &Path) {
    let path = work_dir.join("itest.png");
    debug::screenshot(proc, &path).await.unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.len() > 1000);
    assert_eq!(
        &bytes[..8],
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
    );
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn boot_debug_rip_cycle() {
    let config = mjj_config("boot-debug-rip-cycle");
    let work_dir = config.work_dir.clone();
    let mut session = Session::new(config).start().await.unwrap();

    // Either SymbolLoad variant is fine — MJJHACK.PRG has no symbol table
    // (ADR-8); `wait_until_ready` only needs `symbols prg` to parse.
    let regs = wait_until_ready(session.process_mut()).await;
    assert!(regs.pc.raw() > 0 && regs.pc.raw() < 0xF0_0000);
    assert_palette_annotation(session.process_mut()).await;
    assert_acquire_and_search(session.process_mut(), regs.pc).await;
    assert_screenshot(session.process_mut(), &work_dir).await;

    session.stop().await;
    assert_no_orphan_hatari();
}

/// ADR-11 §2.7 step 2: two `registers` reads 50 ms apart must see the exact
/// same PC (the sleep asserts frozenness, not synchronization — allowed).
async fn assert_frozen_pc_stable(proc: &mut HatariProcess) -> Addr {
    let pc1 = debug::registers(proc).await.unwrap().pc;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let pc2 = debug::registers(proc).await.unwrap().pc;
    assert_eq!(pc1, pc2, "PC must not move while paused");
    pc1
}

/// ADR-11 §2.7 step 3: two whole-RAM acquisitions while frozen must be
/// byte-identical — freeze-then-rip proven at the memory level, not just
/// via the PC register.
async fn assert_frozen_mem_stable(proc: &mut HatariProcess) {
    let mem1 = memory::acquire(proc).await.unwrap();
    let mem2 = memory::acquire(proc).await.unwrap();
    assert_eq!(mem1.base(), mem2.base());
    assert_eq!(mem1.bytes(), mem2.bytes(), "RAM must not move while paused");
}

// ADR-11 §2.7: pause = debugger freeze over stdin (P1-finalize.md), not the
// deadlocking `hatari-stop`/`hatari-cont` pair. Two full freeze/resume
// cycles are exercised deliberately -- repeatability (not just a single
// freeze) is where queued-line bugs in the two-control-plane handoff would
// surface.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn pause_freezes_pc() {
    let running = Session::new(mjj_config("pause-freezes-pc"))
        .start()
        .await
        .unwrap();

    // Cycle 1: freeze, prove PC *and* RAM are stable, resume, prove PC
    // moves again.
    let mut paused = running.pause().await.unwrap();
    let pc1 = assert_frozen_pc_stable(paused.process_mut()).await;
    assert_frozen_mem_stable(paused.process_mut()).await;
    let mut running = paused.resume().await.unwrap();
    assert!(pc_eventually_moves(&mut running, pc1).await);

    // Cycle 2: repeat freeze -> stable -> resume once more (the frozen PC's
    // value is incidental here; `assert_frozen_pc_stable` is the assertion).
    let mut paused = running.pause().await.unwrap();
    let _pc2 = assert_frozen_pc_stable(paused.process_mut()).await;
    let running = paused.resume().await.unwrap();

    running.stop().await;
    assert_no_orphan_hatari();
}

async fn pc_eventually_moves(proc: &mut Session<Running>, from: Addr) -> bool {
    for _ in 0..20 {
        let pc = debug::registers(proc.process_mut()).await.unwrap().pc;
        if pc != from {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

// P3.md §7.4: freeze-then-rip against a real MJJ session — bounds engine
// overhead, not plugin cost. P3 asserted `findings.is_empty()` (no
// detectors were registered yet). P4 registers real, heuristic detectors
// (ADR-12's `plausible_palette`: upper nibbles clear + >= 4 distinct
// words), so an occasional coincidental match over real, uncontrolled RAM
// is expected and accepted (P4.md §8.4 step 5 makes the same call for
// `view_screen_matches_live`'s live-RAM scan: "findings may be 0..n") —
// only completion within budget is asserted here.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn rip_smoke_on_live_ram() {
    let running = Session::new(mjj_config("rip-smoke")).start().await.unwrap();
    let mut paused = running.pause().await.unwrap();

    let mem = memory::acquire(paused.process_mut()).await.unwrap();
    let t = std::time::Instant::now();
    let findings = default_registry().scan(Arc::new(mem), None, 2);
    let _ = findings.len();
    assert!(t.elapsed() < std::time::Duration::from_secs(5));

    paused.stop().await;
    assert_no_orphan_hatari();
}

/// Render the live screen via the same library calls `view_memory_as_screen`
/// uses (hi/mid screen-base bytes at $FF8201/03, resolution at $FF8260,
/// palette at $FF8240). Shared by `freeze_when_rendered`'s readiness probe
/// and `assert_view_screen`'s final capture so both read the *exact* same
/// bytes-to-pixels path.
async fn render_live_screen(proc: &mut HatariProcess) -> image::RgbaImage {
    let base_bytes = debug::read_bytes(proc, Addr::new(0x00FF_8201), 3)
        .await
        .unwrap();
    let base = (u32::from(base_bytes[0]) << 16) | (u32::from(base_bytes[2]) << 8);

    let res_byte = debug::read_bytes(proc, Addr::new(0x00FF_8260), 1)
        .await
        .unwrap();
    let res = ScreenRes::from_word(u16::from(res_byte[0] & 3)).unwrap();

    let pal_bytes = debug::read_bytes(proc, Addr::new(0x00FF_8240), 32)
        .await
        .unwrap();
    let pal = Palette::from_words(&palette_words(&pal_bytes).unwrap());

    let screen_bytes = shiraz_mcp::ripper::image::SCREEN_BYTES;
    let mem = memory::acquire_range(proc, Addr::new(base), screen_bytes)
        .await
        .unwrap();
    decode_planar(mem.bytes(), res, &pal).unwrap()
}

/// At least 2 distinct RGBA values across pixels -- the demo shows
/// *something* rather than a blank (all-white/all-black) framebuffer.
fn is_non_degenerate(img: &image::RgbaImage) -> bool {
    let distinct: std::collections::HashSet<[u8; 4]> = img.pixels().map(|p| p.0).collect();
    distinct.len() >= 2
}

/// `wait_until_ready` only proves the demo's code has started executing (PC
/// in RAM + symbols loaded) -- it says nothing about whether the demo has
/// painted a frame yet. MJJHACK.PRG is UPX-packed and needs a handful of
/// VBLs to unpack and draw, so freezing right after `wait_until_ready`
/// occasionally caught a still-blank framebuffer -- the actual source of
/// `view_screen_matches_live`'s flakiness. Freeze, check the frozen render
/// for real content, and retry (resume, wait a beat, re-freeze) until it is;
/// the returned `Session<Paused>` is then the single frozen instant every
/// subsequent capture (screenshot, RAM-render) is taken from, so nothing
/// downstream can race the demo's animating framebuffer again.
#[allow(clippy::panic, reason = "integration test, not library code")]
async fn freeze_when_rendered(mut running: Session<Running>) -> Session<Paused> {
    for _ in 0..50 {
        let mut paused = running.pause().await.unwrap();
        let img = render_live_screen(paused.process_mut()).await;
        if is_non_degenerate(&img) {
            return paused;
        }
        running = paused.resume().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("MJJHACK.PRG never rendered a non-degenerate screen within the poll budget")
}

/// P4.md §8.4: prove the RAM-render decodes to a non-degenerate PNG. Byte-
/// exact comparison against `screenshot` output is explicitly NOT asserted
/// -- Hatari's screenshot includes borders/scaling; "matches visually"
/// stays a manual check. Both are still taken from the same frozen instant
/// (`freeze_when_rendered`), which is what makes this deterministic.
async fn assert_view_screen(proc: &mut HatariProcess, work_dir: &Path) {
    let img = render_live_screen(proc).await;
    let png = png_bytes(&img).unwrap();
    std::fs::write(work_dir.join("view-itest.png"), &png).unwrap();

    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!(
        (decoded.width(), decoded.height()),
        (img.width(), img.height())
    );
    assert!(
        is_non_degenerate(&decoded.to_rgba8()),
        "expected a non-degenerate screen"
    );
}

/// P4.md §8.4 step 5: scanning the acquired live RAM must not panic (the
/// planted-picture golden-decode proof lives in tests/graphics.rs §8.2).
async fn assert_rip_scan_completes(proc: &mut HatariProcess) {
    let mem = memory::acquire(proc).await.unwrap();
    let findings = default_registry().scan(Arc::new(mem), None, 2);
    let _ = findings.len(); // 0..n is fine -- only "did not panic" is asserted
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn view_screen_matches_live() {
    let config = mjj_config("view-screen-matches-live");
    let work_dir = config.work_dir.clone();
    let mut session = Session::new(config).start().await.unwrap();
    wait_until_ready(session.process_mut()).await;

    let mut paused = freeze_when_rendered(session).await;
    // Screenshot and RAM-render both come from this single frozen instant
    // (ADR-11) -- the fix for the VBL-timing race where the demo's
    // framebuffer would otherwise animate between two live captures taken
    // moments apart.
    assert_screenshot(paused.process_mut(), &work_dir).await;
    assert_view_screen(paused.process_mut(), &work_dir).await;
    assert_rip_scan_completes(paused.process_mut()).await;

    paused.stop().await;
    assert_no_orphan_hatari();
}
