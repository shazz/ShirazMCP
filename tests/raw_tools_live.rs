//! ADR-47 live smoke for `profile_raw`, `count_hits` and `set_registers`
//! (incl. a set PC being where the next managed run starts),
//! driven through the PUBLIC tool surface against a bare TOS desktop (no
//! hard drive needed). `#[ignore]`d; run with e.g.
//! `SHIRAZMCP_HATARI=/opt/homebrew/bin/hatari SHIRAZMCP_TOS=/path/tos162uk.img
//! SHIRAZMCP_WORKDIR=/tmp/raw-live cargo test --test raw_tools_live --
//! --ignored --nocapture`. The fifo and log live in the work dir, so it does
//! not disturb other Hatari instances.

// [clippy::unwrap_used] test code — a panic on unexpected shape/timeout
// against a real Hatari session is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "integration test, not library code")]

use rmcp::handler::server::wrapper::{Json, Parameters};

use shiraz_mcp::session::HatariConfig;
use shiraz_mcp::tools::count_hits::CountHitsArgs;
use shiraz_mcp::tools::debugger::DebugArgs;
use shiraz_mcp::tools::frames::RunFramesArgs;
use shiraz_mcp::tools::lifecycle::StartArgs;
use shiraz_mcp::tools::profile_raw::{ProfileRawArgs, RangeArg};
use shiraz_mcp::tools::set_registers::SetRegistersArgs;
use shiraz_mcp::tools::HatariServer;

fn parse_hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches('$'), 16).unwrap()
}

async fn boot(server: &HatariServer) {
    server
        .start(Parameters(StartArgs::default()))
        .await
        .unwrap();
    let Json(r) = server
        .run_frames(Parameters(RunFramesArgs {
            frames: 200,
            timeout_ms: None,
        }))
        .await
        .unwrap();
    assert!(r.completed);
}

async fn smoke_profile_and_count(server: &HatariServer) {
    let Json(p) = server
        .profile_raw(Parameters(ProfileRawArgs {
            frames: 5,
            ranges: Some(vec![RangeArg {
                name: "tos".into(),
                from: "$e00000".into(),
                to: "$e40000".into(),
            }]),
            top: Some(5),
            timeout_ms: None,
        }))
        .await
        .unwrap();
    println!("profile_raw: {}", serde_json::to_string_pretty(&p).unwrap());
    assert_eq!(p.expected_cycles, 5 * 160_256);
    assert!(
        p.coverage > 0.9 && p.coverage < 1.1,
        "coverage {}",
        p.coverage
    );
    let hot = p.top[0].addr.clone();

    let Json(c) = server
        .count_hits(Parameters(CountHitsArgs {
            pcs: vec![hot.clone(), "$e00000".into()],
            frames: 5,
            timeout_ms: None,
        }))
        .await
        .unwrap();
    println!("count_hits: {}", serde_json::to_string_pretty(&c).unwrap());
    assert_eq!(c.vbl_delta, 5);
    assert!(c.hits[0].hits > 0, "hottest profiled PC {hot} never hit");
    assert_eq!(c.hits[1].hits, 0, "TOS reset entry should not run");
    let Json(b) = server
        .breakpoint(Parameters(
            serde_json::from_str(r#"{"action":"list"}"#).unwrap(),
        ))
        .await
        .unwrap();
    assert!(
        b.breakpoints.unwrap_or_default().is_empty(),
        "count_hits left breakpoints armed"
    );
}

fn sr_word(f: &shiraz_mcp::tools::debugger::SrOutput) -> u16 {
    let bit = |b: bool, n: u16| if b { 1 << n } else { 0 };
    bit(f.supervisor, 13)
        | (u16::from(f.imask) << 8)
        | bit(f.x, 4)
        | bit(f.n, 3)
        | bit(f.z, 2)
        | bit(f.v, 1)
        | bit(f.c, 0)
}

fn stacks(sr: u16, usp: u32, isp: u32) -> SetRegistersArgs {
    SetRegistersArgs {
        sr: Some(sr),
        usp: Some(usp),
        isp: Some(isp),
        d3: Some(0xdead_beef),
        ..Default::default()
    }
}

async fn smoke_set_registers(server: &HatariServer) {
    let Json(orig) = server.registers().await.unwrap();
    // super -> user, then user -> super: both stacks must land each time.
    for (sr, usp, isp) in [(0x0300, 0x7_0000, 0x7_8000), (0x2700, 0x6_0000, 0x6_8000)] {
        let Json(o) = server
            .set_registers(Parameters(stacks(sr, usp, isp)))
            .await
            .unwrap();
        println!(
            "set_registers: {}",
            serde_json::to_string_pretty(&o).unwrap()
        );
        assert!(o.ok, "not applied: {:?}", o.not_applied);
    }
    let restore = SetRegistersArgs {
        d3: Some(parse_hex(&orig.d[3])),
        usp: Some(parse_hex(&orig.usp)),
        isp: Some(parse_hex(&orig.isp)),
        sr: Some(sr_word(&orig.sr)),
        ..Default::default()
    };
    let Json(o) = server.set_registers(Parameters(restore)).await.unwrap();
    assert!(o.ok, "restore not applied: {:?}", o.not_applied);
}

async fn dbg(server: &HatariServer, cmd: &str) -> String {
    let Json(o) = server
        .debug(Parameters(DebugArgs {
            cmd: cmd.into(),
            timeout_ms: None,
        }))
        .await
        .unwrap();
    o.output
}

/// A PC written by set_registers is where the next managed run starts:
/// a counter loop at $40000 (`addq.l #1,$40010.l; bra.s *-6`) at IPL 7
/// must have run after run_frames 1, and PC must still be in the loop.
/// Then the 68000 SR mask: $B300 reads back $A300, reported ok + notes.
async fn smoke_pc_takes(server: &HatariServer) {
    let Json(orig) = server.registers().await.unwrap();
    dbg(
        server,
        "w $40000 $52 $b9 $00 $04 $00 $10 $60 $f8 0 0 0 0 0 0 0 0",
    )
    .await;
    let Json(o) = server
        .set_registers(Parameters(SetRegistersArgs {
            pc: Some(0x4_0000),
            sr: Some(0x2700),
            ..Default::default()
        }))
        .await
        .unwrap();
    assert!(
        o.ok && o.notes.is_empty(),
        "{:?} {:?}",
        o.not_applied,
        o.notes
    );
    let Json(r) = server
        .run_frames(Parameters(RunFramesArgs {
            frames: 1,
            timeout_ms: None,
        }))
        .await
        .unwrap();
    let pc = parse_hex(&r.pc.unwrap());
    assert!(
        (0x4_0000..0x4_0008).contains(&pc),
        "pc ${pc:x} left the loop"
    );
    let cnt = dbg(server, "m $40010 4").await;
    println!("counter after 1 frame: {cnt}");
    assert!(!cnt.contains("00 00 00 00"), "loop never ran: {cnt}");

    let Json(o) = server
        .set_registers(Parameters(SetRegistersArgs {
            sr: Some(0xB300),
            ..Default::default()
        }))
        .await
        .unwrap();
    println!("sr $B300: {}", serde_json::to_string_pretty(&o).unwrap());
    assert!(o.ok, "{:?}", o.not_applied);
    assert_eq!(o.sr_raw, "$A300");
    assert_eq!(o.notes.len(), 2, "{:?}", o.notes);
    let restore = SetRegistersArgs {
        pc: Some(parse_hex(&orig.pc)),
        sr: Some(sr_word(&orig.sr)),
        ..Default::default()
    };
    let Json(o) = server.set_registers(Parameters(restore)).await.unwrap();
    assert!(o.ok, "restore not applied: {:?}", o.not_applied);
}

#[tokio::test]
#[ignore = "needs hatari + a TOS image (SHIRAZMCP_HATARI/SHIRAZMCP_TOS)"]
async fn raw_tools_smoke() {
    let server = HatariServer::new(HatariConfig::default());
    boot(&server).await;
    smoke_profile_and_count(&server).await;
    smoke_set_registers(&server).await;
    smoke_pc_takes(&server).await;
    server.stop().await.unwrap();
}
