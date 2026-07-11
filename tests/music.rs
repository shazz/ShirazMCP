//! SNDH + YM2149 replay detection (P6.md §7). No real SNDH files are
//! committed — byte-pinned builders, exactly like the P4 graphics and P5
//! depack fixtures (deterministic, reviewable, no binary blobs).

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "test code, not library code")]

use std::sync::Arc;

use shiraz_mcp::ripper::plugins::ym_player::format_label;
use shiraz_mcp::ripper::{default_registry, extract, ExtractFormat, RipKind};
use shiraz_mcp::types::{Addr, MemImage};

/// 18 bytes of hand-assembled replay code containing the §4.3
/// hippel-movem-w signature. Candidates at 2/8/14 (all short-form, none
/// FF FF-preceded). Hits (§4.2): lea@0 (len 4), move.b@4 (len 6),
/// movem.l@10 (len 6) -> one cluster start=0 end=16 hits=3 len=16; sig 2
/// matches at block+10 -> format_label("Jochen Hippel (Mad Max)", 0.82, 3).
fn replay_block() -> Vec<u8> {
    vec![
        0x41, 0xF8, 0x88, 0x00, // lea.l   $8800.w, a0
        0x11, 0xFC, 0x00, 0x0B, 0x88, 0x00, // move.b  #$b, $8800.w  (select R11)
        0x48, 0xF8, 0x00, 0x0F, 0x88, 0x00, // movem.l d0-d3, $8800.w (hippel-movem-w)
        0x4E, 0x75, // rts
    ]
}

/// 10 bytes: short-form PSG writes only -- anchors, matches NO signature.
/// Hits: move@0 (len 4), move@4 (len 4) -> cluster start=0 end=8 hits=2.
fn raw_writes_block() -> Vec<u8> {
    vec![
        0x11, 0xC0, 0x88, 0x00, // move.b d0, $8800.w
        0x11, 0xC0, 0x88, 0x02, // move.b d0, $8802.w
        0x4E, 0x75, // rts
    ]
}

/// Full-tag SNDH blob, layout §4.1. Tag block (52 bytes, header_len 68):
/// TITL"Test Tune\0" COMM"Claude\0" RIPP"ShirazMCP\0" "##02" "TC50\0" HDNS.
/// Entries: init=68 (rts), exit=70 (rts), play=72 (replay_block).
/// bra.w displacements: init $0042, exit $0040, play $003E.
fn build_sndh() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x60, 0x00, 0x00, 0x42]); // bra.w +66 -> 68
    out.extend_from_slice(&[0x60, 0x00, 0x00, 0x40]); // bra.w +64 -> 70
    out.extend_from_slice(&[0x60, 0x00, 0x00, 0x3E]); // bra.w +62 -> 72
    out.extend_from_slice(b"SNDH");
    out.extend_from_slice(b"TITLTest Tune\0");
    out.extend_from_slice(b"COMMClaude\0");
    out.extend_from_slice(b"RIPPShirazMCP\0");
    out.extend_from_slice(b"##02");
    out.extend_from_slice(b"TC50\0");
    out.extend_from_slice(b"HDNS");
    assert_eq!(out.len(), 68);
    out.extend_from_slice(&[0x4E, 0x75, 0x4E, 0x75]); // init: rts / exit: rts
    out.extend(replay_block()); // play
    out
}

fn plant(buf: &mut [u8], at: usize, data: &[u8]) {
    buf[at..at + data.len()].copy_from_slice(data);
}

/// A minimal valid NEO header (128-byte header + 32000-byte screen) —
/// reuses neochrome.rs's test shape (ADR-13's plausible-palette + 320x200
/// geometry gates).
fn make_neo() -> Vec<u8> {
    const PAL: [u16; 16] = [
        0x000, 0x700, 0x070, 0x007, 0x770, 0x707, 0x077, 0x777, 0x333, 0x400, 0x040, 0x004, 0x440,
        0x404, 0x044, 0x555,
    ];
    let mut out = Vec::with_capacity(128 + 32000);
    out.extend_from_slice(&0u16.to_be_bytes()); // flag
    out.extend_from_slice(&0u16.to_be_bytes()); // res = low
    out.extend(PAL.iter().flat_map(|w| w.to_be_bytes()));
    out.extend(vec![0u8; 12]); // filename
    out.extend(vec![0u8; 10]); // color-anim limits
    out.extend_from_slice(&320u16.to_be_bytes());
    out.extend_from_slice(&200u16.to_be_bytes());
    out.extend(vec![0u8; 66]);
    assert_eq!(out.len(), 128);
    out.extend(vec![0u8; 32000]);
    out
}

#[test]
fn music_scan_finds_sndh_and_names_player() {
    let mut buf = vec![0u8; 64 * 1024];
    plant(&mut buf, 0x1000, &build_sndh());
    let mem = Arc::new(MemImage::new(buf, Addr::new(0x30000)));
    let findings = default_registry().scan(mem, Some(RipKind::Music), 0);
    assert!(findings.len() >= 2);

    let sndh = findings
        .iter()
        .find(|f| f.plugin.as_str() == "sndh")
        .unwrap();
    assert_eq!(sndh.offset.raw(), 0x1000);
    assert_eq!(sndh.len, 0xF000);
    assert_eq!(
        sndh.label,
        "SNDH 'Test Tune' by Claude (2 subtune(s), timer-c 50Hz)"
    );
    assert_eq!(sndh.addr(), Some(Addr::new(0x31000)));

    let player = findings
        .iter()
        .find(|f| f.plugin.as_str() == "ym-player")
        .unwrap();
    assert_eq!(player.offset.raw(), 0x1000 + 72);
    assert_eq!(player.len, 16);
    assert_eq!(
        player.label,
        format_label("Jochen Hippel (Mad Max)", 0.82, 3)
    );
}

#[test]
fn music_scan_unknown_degrades() {
    let mut buf = vec![0u8; 4096];
    plant(&mut buf, 0x800, &raw_writes_block());
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Music), 0);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].plugin.as_str(), "ym-player");
    assert_eq!(findings[0].offset.raw(), 0x800);
    assert_eq!(findings[0].len, 8);
    assert_eq!(
        findings[0].label,
        "YM replay [unknown] confidence=0.25 hits=2"
    );
}

#[test]
fn music_scan_two_separated_players() {
    let mut buf = vec![0u8; 0x3000];
    plant(&mut buf, 0x100, &replay_block());
    plant(&mut buf, 0x2000, &raw_writes_block());
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Music), 0);
    let players: Vec<_> = findings
        .iter()
        .filter(|f| f.plugin.as_str() == "ym-player")
        .collect();
    assert_eq!(players.len(), 2);
    assert_eq!(
        players[0].label,
        format_label("Jochen Hippel (Mad Max)", 0.82, 3)
    );
    assert_eq!(
        players[1].label,
        "YM replay [unknown] confidence=0.25 hits=2"
    );
}

#[test]
fn music_extract_prg_is_runnable_wrapper() {
    let mut buf = vec![0u8; 4096];
    plant(&mut buf, 0, &build_sndh());
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Music), 0);
    let sndh = findings
        .iter()
        .find(|f| f.plugin.as_str() == "sndh")
        .unwrap();

    let dir = std::env::temp_dir().join("shiraz-mcp-music-test-prg");
    let extracted = extract(sndh, ExtractFormat::Prg, &dir, 1).unwrap();
    let bytes = std::fs::read(&extracted.path).unwrap();
    assert_eq!(bytes.len() as u64, sndh.len as u64 + 32);
    assert_eq!(&bytes[..2], &[0x60, 0x1A]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn music_kind_filter_excludes_pictures() {
    let mut buf = vec![0u8; 0x20000];
    plant(&mut buf, 0x1000, &build_sndh());
    plant(&mut buf, 0x9000, &make_neo());
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Music), 0);
    assert!(findings.iter().all(|f| f.plugin.as_str() != "neochrome"));
    assert!(findings.iter().any(|f| f.plugin.as_str() == "sndh"));
}

/// // verify live: manual behavioral check (PLAN §P6 acceptance). Get a RAM
/// dump from a session playing YM music (another agent owns Hatari right
/// now): save_state / savebin path per ADR-9, then:
///   SHIRAZMCP_RAM_DUMP=/path/to/dump.bin cargo test --test music -- --ignored music_live
#[test]
#[ignore = "needs a real RAM dump; set SHIRAZMCP_RAM_DUMP"]
fn music_live_ram_dump_has_replay_anchor() {
    let path = std::env::var("SHIRAZMCP_RAM_DUMP").expect("set SHIRAZMCP_RAM_DUMP");
    let bytes = std::fs::read(path).expect("readable dump");
    let mem = Arc::new(MemImage::new(bytes, Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Music), 2);
    assert!(
        findings.iter().any(|f| f.plugin.as_str() == "ym-player"),
        "no YM anchor in live dump -- investigate before closing P6"
    );
}
