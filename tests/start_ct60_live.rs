//! `start` with a CT60-like Falcon (68060, 32-bit addressing, TT-RAM) against
//! a real Hatari session. `#[ignore]`d - needs `DISPLAY`, `/usr/bin/hatari`
//! and a Falcon TOS 4.04 image (`SHIRAZMCP_TOS404`). Run:
//! `SHIRAZMCP_TOS404=/path/to/tos404.img DISPLAY=:0 \
//!  cargo test --test start_ct60_live -- --ignored --test-threads=1`.
//!
//! Proves the 32-bit address path end to end: TT-RAM ($01000000+) is
//! writable/readable, disassembled (snapshot + live), and the same addresses
//! are refused on a default 24-bit session.

// [clippy::unwrap_used] whole binary is test code - a panic on an unexpected
// shape against a real Hatari is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use shiraz_mcp::session::SessionConfig;
use shiraz_mcp::tools::analyze::{DisasmArgs, DisasmSource};
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::mem::{ReadMemoryArgs, WriteMemoryArgs};
use shiraz_mcp::tools::HatariServer;

fn tos404() -> String {
    std::env::var("SHIRAZMCP_TOS404").unwrap_or_else(|_| "/path/to/tos404.img".into())
}

fn server(tag: &str) -> HatariServer {
    HatariServer::new(SessionConfig {
        work_dir: format!("/tmp/shiraz-mcp-itest-ct60-{tag}").into(),
        ..SessionConfig::default()
    })
}

fn ct60_args() -> StartArgs {
    StartArgs {
        machine: Some("falcon".into()),
        tos_path: Some(tos404()),
        memsize_mb: Some(14),
        cpu_level: Some(6),
        fpu: Some(shiraz_mcp::session::spawn::options::Fpu::Internal),
        cpu_clock: Some(32),
        compatible: Some(false),
        cpu_exact: Some(false),
        mmu: Some(false),
        addr24: Some(false),
        tt_ram_mb: Some(16),
        dsp: Some(shiraz_mcp::session::spawn::options::Dsp::Emu),
        ..Default::default()
    }
}

fn read(addr: &str, count: u32) -> Parameters<ReadMemoryArgs> {
    Parameters(ReadMemoryArgs {
        addr: addr.into(),
        count: Some(count),
    })
}

#[tokio::test]
#[ignore = "needs Hatari + DISPLAY + TOS 4.04 (SHIRAZMCP_TOS404)"]
async fn ct60_falcon_tt_ram_is_reachable() {
    let s = server("ttram");
    let pid = s.start(Parameters(ct60_args())).await.unwrap().0.pid;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // 32-bit write/read in TT-RAM (and a NOP + RTS to disassemble).
    let w = WriteMemoryArgs {
        addr: "$01000000".into(),
        bytes_hex: "deadbeef4e714e75".into(),
    };
    s.write_memory(Parameters(w)).await.unwrap();
    let got = s.read_memory(read("$01000000", 8)).await.unwrap().0;
    assert_eq!(got.bytes_hex, "deadbeef4e714e75");
    assert!(got.annotations.is_empty(), "TT-RAM is not a register");

    // 32-bit alias of the I/O page still annotates.
    let io = s.read_memory(read("$FFFF8240", 2)).await.unwrap().0;
    assert!(io.annotations.iter().any(|a| a.label.contains("palette")));

    for source in [DisasmSource::Snapshot, DisasmSource::Live] {
        let d = s
            .disasm(Parameters(DisasmArgs {
                addr: "$01000004".into(),
                count: Some(2),
                source: Some(source),
            }))
            .await
            .unwrap()
            .0;
        assert_eq!(d.insns.len(), 2, "{source:?}");
        assert!(d.insns[0].mnemonic.to_lowercase().starts_with("nop"));
    }
    s.stop().await.unwrap();
    assert_gone(pid);
}

/// The same address is refused on a stock 24-bit session (no silent mask).
#[tokio::test]
#[ignore = "needs Hatari + DISPLAY + TOS 4.04 (SHIRAZMCP_TOS404)"]
async fn default_session_refuses_tt_ram_addresses() {
    let s = server("st24");
    let pid = s
        .start(Parameters(StartArgs {
            machine: Some("falcon".into()),
            tos_path: Some(tos404()),
            memsize_mb: Some(4),
            ..Default::default()
        }))
        .await
        .unwrap()
        .0
        .pid;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let err = s.read_memory(read("$01000000", 4)).await.err().unwrap();
    assert!(err.message.contains("24-bit"), "{}", err.message);
    s.stop().await.unwrap();
    assert_gone(pid);
}

/// Hatari rejects nothing here - the server must refuse before launch.
#[tokio::test]
async fn invalid_combination_fails_before_launch() {
    let s = server("invalid");
    let err = s
        .start(Parameters(StartArgs {
            machine: Some("st".into()),
            tt_ram_mb: Some(8),
            addr24: Some(false),
            ..Default::default()
        }))
        .await
        .err()
        .unwrap();
    assert!(err.message.contains("tt or falcon"), "{}", err.message);
}

/// Per-pid, not `pgrep -x hatari`: other Hatari instances may be running on
/// the box and are none of this test's business.
fn assert_gone(pid: Option<u32>) {
    let pid = pid.unwrap();
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "hatari {pid} survived stop()"
    );
}
