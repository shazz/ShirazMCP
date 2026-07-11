//! Pack-Ice fixtures + the depack→rescan recursion proof (P5.md §6). No
//! committed binaries: fixtures are byte-pinned builders (hand-traced
//! vectors + a pinned stream assembler), consistent with ADR-12's fixture
//! policy — see ADR-14.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "test code, not library code")]

use std::sync::Arc;

use shiraz_mcp::debug;
use shiraz_mcp::ripper::image::ScreenRes;
use shiraz_mcp::ripper::plugins::pack_ice::{PackIce, PackIceRegion};
use shiraz_mcp::ripper::{default_registry, extract, Depacker, ExtractFormat, RipKind, RipPlugin};
use shiraz_mcp::session::{memory, HatariConfig, HatariProcess, Session};
use shiraz_mcp::types::{Addr, MemImage, PluginId};

mod common;
use common::{make_pi, plant};

// ---- pinned byte vectors (P5 §6.2) ----

const V1_ICE: &[u8] = &[
    0x49, 0x43, 0x45, 0x21, 0x00, 0x00, 0x00, 0x0F, 0x00, 0x00, 0x00, 0x06, 0x01, 0x41, 0xB7,
];
const V1_ICE_LOWER: &[u8] = &[
    0x49, 0x63, 0x65, 0x21, 0x00, 0x00, 0x00, 0x0F, 0x00, 0x00, 0x00, 0x06, 0x01, 0x41, 0xB7,
];
const V3_ICE: &[u8] = &[
    0x49, 0x43, 0x45, 0x21, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x04, 0x0B, 0x41, 0x42, 0xC1,
];

// ---- the pinned stream assembler (P5 §6.3) ----

/// One decode event, in DECODER-consumption order.
enum Ev<'a> {
    Bits(Vec<u8>),
    Block(&'a [u8]),
}

/// §6.3 rule 3: how many bit bytes must be loaded before `n` consumed bits
/// are available.
fn bytes_loaded(n: usize) -> usize {
    if n <= 7 {
        1
    } else {
        1 + (n - 7).div_ceil(8)
    }
}

/// §6.3 rule 2, one byte: `bits` occupy the top positions, sentinel + 1-fill
/// share the rest (numerically both `1`, so this collapses to "1-fill the
/// low bits").
fn pack_bit_byte(bits: &[u8]) -> u8 {
    let mut v: u16 = 0;
    for &b in bits {
        v = (v << 1) | u16::from(b);
    }
    let pad = 8 - bits.len() as u32;
    ((v << pad) | ((1u16 << pad) - 1)) as u8
}

/// §6.3 rule 2, the whole bit list -> bit bytes in consumption order.
fn build_bit_bytes(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let first_k = bits.len().min(7);
    out.push(pack_bit_byte(&bits[..first_k]));
    let mut i = first_k;
    while i < bits.len() {
        let end = (i + 8).min(bits.len());
        out.push(pack_bit_byte(&bits[i..end]));
        i = end;
    }
    out
}

/// Assemble a Pack-Ice blob ("ICE!" header) from a decode script (§6.3).
fn ice_assemble(script: &[Ev], decrunched: u32) -> Vec<u8> {
    let all_bits: Vec<u8> = script
        .iter()
        .flat_map(|ev| match ev {
            Ev::Bits(b) => b.clone(),
            Ev::Block(_) => Vec::new(),
        })
        .collect();
    let bit_bytes = build_bit_bytes(&all_bits);
    let consumption = build_consumption(script, &bit_bytes);

    let mut out = Vec::new();
    out.extend_from_slice(b"ICE!");
    let stream_len: usize = consumption.iter().map(Vec::len).sum();
    out.extend_from_slice(&(12 + stream_len as u32).to_be_bytes());
    out.extend_from_slice(&decrunched.to_be_bytes());
    for chunk in consumption.iter().rev() {
        out.extend_from_slice(chunk);
    }
    out
}

/// §6.3's walk: interleave bit bytes and literal blocks in consumption
/// order, loading exactly the bit bytes each Block needs beforehand.
fn build_consumption(script: &[Ev], bit_bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut bits_so_far = 0usize;
    let mut pushed = 0usize;
    for ev in script {
        match ev {
            Ev::Bits(b) => bits_so_far += b.len(),
            Ev::Block(block) => {
                let needed = bytes_loaded(bits_so_far);
                while pushed < needed {
                    out.push(vec![bit_bytes[pushed]]);
                    pushed += 1;
                }
                out.push(block.to_vec());
            }
        }
    }
    while pushed < bit_bytes.len() {
        out.push(vec![bit_bytes[pushed]]);
        pushed += 1;
    }
    out
}

/// Invariant: whenever row i is reached, len >= ADD[i] (guaranteed by the
/// escalation condition of row i-1) — the subtraction cannot underflow.
/// The decoder escalates exactly when a chunk equals ONES[i], so emit
/// ONES[i] both when the remainder doesn't fit and when it equals ONES[i];
/// stop only on value < ONES[i].
fn push_direct_length(bits: &mut Vec<u8>, len: usize) {
    const BITS: [u32; 6] = [1, 2, 2, 3, 8, 15];
    const ONES: [usize; 6] = [1, 3, 3, 7, 0xFF, 0x7FFF];
    const ADD: [usize; 6] = [1, 2, 5, 8, 15, 270];
    for i in 0..6 {
        let value = (len - ADD[i]).min(ONES[i]);
        for b in (0..BITS[i]).rev() {
            bits.push((value >> b & 1) as u8); // MSB-first (get_bits order)
        }
        if value < ONES[i] {
            return;
        }
    }
    unreachable!("callers cap len at 33_036");
}

/// One direct-copy block. Valid for 1..=33_036 bytes (§4.4 rows 0..=5). The
/// payload appears VERBATIM in the blob — never use it for a payload a
/// detector could match (§6.1).
fn ice_pack_literal(payload: &[u8]) -> Vec<u8> {
    assert!(!payload.is_empty() && payload.len() <= 33_036);
    let mut bits = vec![1u8]; // literal flag; stream ends via the literal
    push_direct_length(&mut bits, payload.len());
    ice_assemble(&[Ev::Bits(bits), Ev::Block(payload)], payload.len() as u32)
}

/// Pack a Degas PI1 so that NO contiguous res-word+palette survives in the
/// blob: payload[0..4] must be 00 00 00 00 (PI1 res=low + black palette
/// word 0) — the leading 3 bytes are rebuilt by an RLE-run match (§6.1).
fn ice_pack_pi1(payload: &[u8]) -> Vec<u8> {
    assert_eq!(&payload[..4], &[0, 0, 0, 0]);
    let mut bits1 = vec![1u8];
    push_direct_length(&mut bits1, payload.len() - 3);
    let bits2 = vec![1, 0, 1, 0, 0, 0, 0, 0, 0, 0]; // §6.1 match + final flag
    ice_assemble(
        &[Ev::Bits(bits1), Ev::Block(&payload[3..]), Ev::Bits(bits2)],
        payload.len() as u32,
    )
}

// ---- misc helpers ----

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shiraz-mcp-p5-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Pinned LCG (seed 1): x = x*1103515245 + 12345 (wrapping); each byte is
/// the high byte of x, `(x >> 16) as u8`.
fn lcg_bytes(n: usize) -> Vec<u8> {
    let mut x: u32 = 1;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (x >> 16) as u8
        })
        .collect()
}

// ---- tests ----

#[test]
fn pinned_blobs_depack_both_magics() {
    assert_eq!(PackIce.try_unpack(V1_ICE), Some(b"AAAAAA".to_vec()));
    assert_eq!(PackIce.try_unpack(V1_ICE_LOWER), Some(b"AAAAAA".to_vec()));
    assert_eq!(PackIce.try_unpack(V3_ICE), Some(b"ABAB".to_vec()));
}

#[test]
fn encoder_roundtrips_via_depacker() {
    let payloads: Vec<Vec<u8>> = vec![
        b"Q".to_vec(),
        b"AB".to_vec(),
        b"WXYZ".to_vec(),
        vec![0xAAu8; 6],
        vec![0xBBu8; 10],
        vec![0xCCu8; 100],
        vec![0xDDu8; 300],
        lcg_bytes(5000),
    ];
    for p in payloads {
        assert_eq!(PackIce.try_unpack(&ice_pack_literal(&p)), Some(p));
    }
    let pi1 = make_pi(ScreenRes::Low);
    assert_eq!(PackIce.try_unpack(&ice_pack_pi1(&pi1)), Some(pi1));
}

#[test]
fn assembler_matches_pinned_vectors() {
    let script = [
        Ev::Bits(vec![1, 0]),
        Ev::Block(b"A"),
        Ev::Bits(vec![1, 1, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0]),
    ];
    assert_eq!(ice_assemble(&script, 6), V1_ICE);
    assert_eq!(ice_pack_literal(b"Q"), {
        let mut h = vec![0x49, 0x43, 0x45, 0x21];
        h.extend(14u32.to_be_bytes());
        h.extend(1u32.to_be_bytes());
        h.extend([0x51, 0xBF]);
        h
    });
    assert_eq!(ice_pack_literal(b"WXYZ"), {
        let mut h = vec![0x49, 0x43, 0x45, 0x21];
        h.extend(17u32.to_be_bytes());
        h.extend(4u32.to_be_bytes());
        h.extend([0x57, 0x58, 0x59, 0x5A, 0xEF]);
        h
    });
}

fn planted_pi1_buf() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let pi1 = make_pi(ScreenRes::Low);
    let blob = ice_pack_pi1(&pi1);
    assert_eq!(blob.len(), 32049);
    let mut buf = vec![0u8; 256 * 1024];
    plant(&mut buf, 0x0400, &blob);
    (buf, blob, pi1)
}

#[test]
fn packed_pi1_recursion_finds_picture() {
    let (buf, _blob, pi1) = planted_pi1_buf();
    let mem = Arc::new(MemImage::new(buf, Addr::new(0x10000)));
    let registry = default_registry();
    let findings = registry.scan(mem, Some(RipKind::Picture), 2);

    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.plugin.as_str(), "degas");
    assert_eq!(f.via(), [PluginId::new("pack-ice")]);
    assert_eq!(f.offset.raw(), 0);
    assert_eq!(f.len, 32034);
    assert!(f.addr().is_none());
    assert_eq!(f.bytes().unwrap(), pi1.as_slice());

    let png = registry.render_png(f.plugin, f.bytes().unwrap()).unwrap();
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (320, 200));
    let rgba8 = decoded.to_rgba8();
    assert_eq!(rgba8.get_pixel(0, 0).0, [0, 0, 0, 255]);
    assert_eq!(rgba8.get_pixel(25, 0).0, [255, 0, 0, 255]);
    assert_eq!(rgba8.get_pixel(319, 199).0, [182, 182, 182, 255]);

    let dir = temp_dir("recursion");
    let extracted = extract(f, ExtractFormat::Raw, &dir, 1).unwrap();
    assert_eq!(extracted.path, dir.join("rip-0001-degas-000000.bin"));
    assert_eq!(std::fs::read(&extracted.path).unwrap(), pi1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn depth_zero_sees_region_not_picture() {
    let (buf, blob, _pi1) = planted_pi1_buf();
    let mem = Arc::new(MemImage::new(buf, Addr::new(0x10000)));
    let registry = default_registry();

    assert!(registry
        .scan(mem.clone(), Some(RipKind::Picture), 0)
        .is_empty());

    let findings = registry.scan(mem, Some(RipKind::Packer), 0);
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.plugin.as_str(), "pack-ice-region");
    assert_eq!(f.kind, RipKind::Packer);
    assert_eq!(f.offset.raw(), 0x0400);
    assert_eq!(f.len, blob.len());
    assert_eq!(f.addr(), Some(Addr::new(0x10400)));
    assert!(f.label.contains("ICE!"));
    assert!(f.label.contains("32049"));
    assert!(f.label.contains("32034"));
}

#[test]
fn full_scan_orders_region_before_recursed_picture() {
    let (buf, _blob, _pi1) = planted_pi1_buf();
    let mem = Arc::new(MemImage::new(buf, Addr::new(0x10000)));
    let findings = default_registry().scan(mem, None, 2);

    assert_eq!(findings.len(), 2);
    assert_eq!(findings[0].plugin.as_str(), "pack-ice-region");
    assert!(findings[0].via().is_empty());
    assert_eq!(findings[1].plugin.as_str(), "degas");
    assert_eq!(findings[1].via(), [PluginId::new("pack-ice")]);
}

#[test]
fn two_regions_detector_reports_both_depacker_takes_first() {
    let mut buf = vec![0u8; 64 * 1024];
    plant(&mut buf, 0x100, &ice_pack_literal(b"WXYZ"));
    plant(&mut buf, 0x2000, V1_ICE);
    let mem = MemImage::new(buf.clone(), Addr::new(0));

    let findings = PackIceRegion.scan(&mem);
    assert_eq!(findings.len(), 2);
    assert_eq!(findings[0].offset.raw(), 0x100);
    assert_eq!(findings[1].offset.raw(), 0x2000);

    assert_eq!(PackIce.try_unpack(&buf), Some(b"WXYZ".to_vec()));
}

// Duplicated from tests/integration.rs (separate test crates cannot share
// items without a tests/common/ module — not worth adding for one shared
// helper): MJJ config + the "MJJHACK.PRG's own code is running" poll.
fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: format!("/tmp/shiraz-mcp-p5-itest-{name}").into(),
        ..HatariConfig::default()
    }
}

#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_until_ready(proc: &mut HatariProcess) {
    for _ in 0..50 {
        let symbols = debug::load_symbols(proc).await;
        let regs = debug::registers(proc).await.unwrap();
        if symbols.is_ok() && regs.pc.raw() < (1 << 20) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("MJJHACK.PRG never became ready (loaded + PC in RAM) within the poll budget");
}

/// §6.5-8: load a Pack-Ice-packed PI1 into live ST RAM and prove `rip`
/// finds it through the depacker. Requires Hatari + MJJ (PLAN §P5
/// behavioral acceptance) — not run by this phase's implementer (another
/// agent may be driving Hatari concurrently).
#[tokio::test]
#[ignore = "requires a running Hatari + MJJ session (see tests/integration.rs)"]
async fn packed_pi1_live_rip() {
    let config = mjj_config("packed-pi1-live-rip");
    let work_dir = config.work_dir.clone();
    let mut session = Session::new(config).start().await.unwrap();
    wait_until_ready(session.process_mut()).await;
    let mut paused = session.pause().await.unwrap();

    let pi1 = make_pi(ScreenRes::Low);
    let blob = ice_pack_pi1(&pi1);
    let path = work_dir.join("packed.ice");
    std::fs::write(&path, &blob).unwrap();

    // `loadbin` is savebin's counterpart (ADR-9 uses savebin). // verify:
    // if the debugger rejects it, fall back to chunked
    // debug::write_bytes in 1024-byte chunks — nothing else changes.
    let loaded = paused
        .process_mut()
        .debug(&format!("loadbin {} $60000", path.display()))
        .await;
    if loaded.is_err() {
        for (i, chunk) in blob.chunks(1024).enumerate() {
            let addr = Addr::new(0x60000 + (i * 1024) as u32);
            debug::write_bytes(paused.process_mut(), addr, chunk)
                .await
                .unwrap();
        }
    }

    let mem = memory::acquire(paused.process_mut()).await.unwrap();
    let findings = default_registry().scan(Arc::new(mem), Some(RipKind::Picture), 2);
    let hit = findings
        .iter()
        .find(|f| f.plugin.as_str() == "degas" && f.via() == [PluginId::new("pack-ice")])
        .unwrap();
    assert_eq!(hit.bytes().unwrap(), pi1.as_slice());
    let png = default_registry()
        .render_png(hit.plugin, hit.bytes().unwrap())
        .unwrap();
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (320, 200));

    paused.stop().await;
}
