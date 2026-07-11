//! Regression for the `debug::write_bytes` silent-truncation bug: Hatari's
//! `DebugUI_ParseCommand` hard-caps a `memwrite` line at 61 data-byte
//! tokens (`psArgs[64]`, `hatari/src/debug/debugui.c`) — above that the
//! WHOLE line is rejected, and the rejection text's lowercase "rror" used
//! to slip past the old case-sensitive `ERROR` check, so the write came
//! back `Ok(())` with nothing actually in RAM. `debug::write_bytes` now
//! chunks internally to a safe per-line size, so any single call, of any
//! length, must round-trip byte-identical. `#[ignore]`d — needs DISPLAY,
//! `/usr/bin/hatari`, and the MJJ tree.

// [clippy::unwrap_used] integration test, not library code (see
// tests/mcp_handshake.rs for the same rationale).
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::process::Command;

use shiraz_mcp::session::{memory, HatariConfig, Session};
use shiraz_mcp::types::Addr;
use shiraz_mcp::{debug, error::Error};

fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: format!("/tmp/shiraz-mcp-write-bytes-live-{name}").into(),
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

/// A deliberately non-repeating pattern (not a fixed byte, not aligned to
/// any chunk size) — a naive truncation OR a misaligned/duplicated chunk
/// would both change the tail of this sequence in a way plain zeros or a
/// constant fill could hide.
fn pattern(len: usize) -> Vec<u8> {
    (0..len as u32)
        .map(|i| ((i * 37 + 11) % 256) as u8)
        .collect()
}

#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn write_bytes_large_single_call_roundtrips_full_length() {
    let running = Session::new(mjj_config("large")).start().await.unwrap();
    let mut paused = running.pause().await.unwrap();
    let proc = paused.process_mut();

    // Scratch RAM well above the MJJ demo's load region, inside 1 MB.
    let addr = Addr::new(0x78200);
    let want = pattern(400); // >> the 61-byte hard Hatari-line limit

    debug::write_bytes(proc, addr, &want).await.unwrap();
    let got = memory::acquire_range(proc, addr, want.len()).await.unwrap();

    assert_eq!(got.len(), want.len(), "readback length must not shrink");
    assert_eq!(
        got.bytes(),
        want.as_slice(),
        "readback must be byte-identical — a truncated/rejected memwrite \
         line would show up as a run of stale/zero bytes here"
    );

    paused.stop().await;
    assert_no_orphan_hatari();
}

/// A write landing exactly ON the historically-broken boundary (just above
/// what silently failed before the fix) must still succeed — guards
/// against reintroducing an off-by-one in the new chunk math.
#[tokio::test]
#[ignore = "needs DISPLAY, /usr/bin/hatari, and the MJJ tree"]
async fn write_bytes_at_old_failure_boundary_roundtrips() {
    let running = Session::new(mjj_config("boundary")).start().await.unwrap();
    let mut paused = running.pause().await.unwrap();
    let proc = paused.process_mut();

    let addr = Addr::new(0x78400);
    let want = pattern(62); // one past Hatari's proven 61-byte hard ceiling

    let result = debug::write_bytes(proc, addr, &want).await;
    assert!(!matches!(result, Err(Error::Parse { .. })), "{result:?}");
    let got = memory::acquire_range(proc, addr, want.len()).await.unwrap();
    assert_eq!(got.bytes(), want.as_slice());

    paused.stop().await;
    assert_no_orphan_hatari();
}
