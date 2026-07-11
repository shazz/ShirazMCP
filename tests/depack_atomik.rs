//! Atomik 3.5 (`ATM5`) fixtures + the depack→rescan recursion proof (P9
//! §6.1). No committed binaries: fixtures are byte-pinned builders (a
//! hand-traced vector table + a pinned stream assembler), consistent with
//! ADR-12's fixture policy — see ADR-19.
#![allow(clippy::unwrap_used, reason = "test code, not library code")]

use std::sync::Arc;

use shiraz_mcp::debug;
use shiraz_mcp::ripper::image::ScreenRes;
use shiraz_mcp::ripper::plugins::atomik::{Atomik, AtomikRegion};
use shiraz_mcp::ripper::plugins::powerpacker::{PowerPacker, PowerPackerRegion};
use shiraz_mcp::ripper::{default_registry, extract, Depacker, ExtractFormat, RipKind, RipPlugin};
use shiraz_mcp::session::{memory, HatariConfig, HatariProcess, Session};
use shiraz_mcp::types::{Addr, MemImage, PluginId};

mod common;
use common::{make_pi, plant};

// ---- pinned byte vectors (P9 §6.1.2) ----

const A1: &[u8] = &[
    0x41, 0x54, 0x4D, 0x35, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x07, 0x41, 0x42, 0x02, 0x00,
    0x00, 0x08, 0x00,
];
const A2: &[u8] = &[
    0x41, 0x54, 0x4D, 0x35, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x08, 0x00, 0x80, 0x41, 0x06,
    0x00, 0x03, 0x11, 0x00,
];
const A3: &[u8] = &[
    0x41, 0x54, 0x4D, 0x35, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x07, 0x20, 0x42, 0x02, 0x00,
    0x00, 0x1D, 0x00,
];

// A duplicate of powerpacker's P1 (P9 §6.2.2), for the cross-scheme test.
const PP_P1: &[u8] = &[
    0x50, 0x50, 0x32, 0x30, 0x09, 0x0A, 0x0C, 0x0D, 0x00, 0x1C, 0x10, 0x00, 0x00, 0x00, 0x06, 0x08,
];

// ---- the pinned stream assembler (P9 §6.1.1) ----

/// One decode event, in DECODER-consumption order.
enum Ev {
    Bits(Vec<u8>),
    Byte(u8),
}

/// §6.1.1 rule 3: how many bit bytes must be loaded before `n` consumed
/// bits are available.
fn bytes_loaded(n: usize) -> usize {
    if n <= 7 {
        1
    } else {
        1 + (n - 7).div_ceil(8)
    }
}

/// §6.1.1 rule 2, the first (raw-init) byte: data bits at the top, then a
/// `1` sentinel, then ZERO-fill (the recycled-carry reader's one deviation
/// from Pack-Ice's forced-1-fill, P5 §6.3 rule 2).
fn pack_first(bits: &[u8]) -> u8 {
    let mut v: u16 = 0;
    for &b in bits {
        v = (v << 1) | u16::from(b);
    }
    let pad = 8 - bits.len() as u32;
    ((v << pad) | (1u16 << (pad - 1))) as u8
}

/// §6.1.1 rule 2, a reloaded byte: 8 data bits, the last zero-padded low.
fn pack_plain(bits: &[u8]) -> u8 {
    let mut v: u16 = 0;
    for &b in bits {
        v = (v << 1) | u16::from(b);
    }
    (v << (8 - bits.len() as u32)) as u8
}

fn build_bit_bytes(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let first_k = bits.len().min(7);
    out.push(pack_first(&bits[..first_k]));
    let mut i = first_k;
    while i < bits.len() {
        let end = (i + 8).min(bits.len());
        out.push(pack_plain(&bits[i..end]));
        i = end;
    }
    out
}

/// §6.1.1's walk: interleave bit bytes and raw `Byte`s in consumption
/// order, loading exactly the bit bytes each `Byte` needs beforehand.
fn build_consumption(script: &[Ev], bit_bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut bits_so_far = 0usize;
    let mut pushed = 0usize;
    for ev in script {
        match ev {
            Ev::Bits(b) => bits_so_far += b.len(),
            Ev::Byte(byte) => {
                let needed = bytes_loaded(bits_so_far);
                while pushed < needed {
                    out.push(vec![bit_bytes[pushed]]);
                    pushed += 1;
                }
                out.push(vec![*byte]);
            }
        }
    }
    while pushed < bit_bytes.len() {
        out.push(vec![bit_bytes[pushed]]);
        pushed += 1;
    }
    out
}

/// Assemble an Atomik blob ("ATM5" header) from a decode script (§6.1.1).
/// Consumption units = `[pic_count=0] ++ walk(script)`; the file stream is
/// the REVERSE of the units (the stream is read backward).
fn atomik_assemble(script: &[Ev], decrunched: u32) -> Vec<u8> {
    let all_bits: Vec<u8> = script
        .iter()
        .flat_map(|ev| match ev {
            Ev::Bits(b) => b.clone(),
            Ev::Byte(_) => Vec::new(),
        })
        .collect();
    let bit_bytes = build_bit_bytes(&all_bits);
    let mut units: Vec<Vec<u8>> = vec![vec![0u8]]; // pic_count, fixed 0
    units.extend(build_consumption(script, &bit_bytes));

    let stream_len: usize = units.iter().map(Vec::len).sum();
    let mut out = Vec::new();
    out.extend_from_slice(b"ATM5");
    out.extend_from_slice(&decrunched.to_be_bytes());
    out.extend_from_slice(&(stream_len as u32).to_be_bytes());
    for chunk in units.iter().rev() {
        out.extend_from_slice(chunk);
    }
    out
}

/// One chunk, all literals. Valid for 1..=65_535 bytes.
fn atomik_pack_literal(payload: &[u8]) -> Vec<u8> {
    assert!(!payload.is_empty() && payload.len() <= 65_535);
    let mut script = vec![
        Ev::Bits(vec![0]),
        Ev::Bits(vec![0]), // no spec table, no freq table
        Ev::Byte(0x00),    // d3
        Ev::Byte((payload.len() >> 8) as u8),
        Ev::Byte(payload.len() as u8),
    ];
    for &b in payload.iter().rev() {
        // output is backward: last payload byte first
        script.push(Ev::Bits(vec![0]));
        script.push(Ev::Byte(b));
    }
    atomik_assemble(&script, payload.len() as u32)
}

// ---- misc helpers ----

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shiraz-mcp-p9-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Pinned LCG (seed 1): x = x*1103515245 + 12345 (wrapping); each byte is
/// the high byte of x, `(x >> 16) as u8` (P5's pinned generator).
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
fn pinned_blobs_depack() {
    assert_eq!(Atomik.try_unpack(A1), Some(b"AB".to_vec()));
    assert_eq!(Atomik.try_unpack(A2), Some(b"AAAAAA".to_vec()));
    assert_eq!(Atomik.try_unpack(A3), Some(b"AB".to_vec()));
}

#[test]
fn assembler_matches_pinned_vectors() {
    assert_eq!(atomik_pack_literal(b"AB"), A1);

    let a2_script = [
        Ev::Bits(vec![0]),
        Ev::Bits(vec![0]),
        Ev::Byte(0x03),
        Ev::Byte(0x00),
        Ev::Byte(0x06),
        Ev::Bits(vec![0]),
        Ev::Byte(0x41),
        Ev::Bits(vec![1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]),
    ];
    assert_eq!(atomik_assemble(&a2_script, 6), A2);

    let a3_script = [
        Ev::Bits(vec![0]),
        Ev::Bits(vec![0]),
        Ev::Byte(0x00),
        Ev::Byte(0x00),
        Ev::Byte(0x02),
        Ev::Bits(vec![0]),
        Ev::Byte(0x42),
        Ev::Bits(vec![1, 1, 1, 0, 0, 0, 1]),
    ];
    assert_eq!(atomik_assemble(&a3_script, 2), A3);
}

#[test]
fn encoder_roundtrips_via_depacker() {
    let payloads: Vec<Vec<u8>> = vec![
        b"Q".to_vec(),
        b"AB".to_vec(),
        b"WXYZ".to_vec(),
        vec![0xCCu8; 100],
        lcg_bytes(5000),
    ];
    for p in payloads {
        assert_eq!(Atomik.try_unpack(&atomik_pack_literal(&p)), Some(p));
    }
    let pi1 = make_pi(ScreenRes::Low);
    assert_eq!(Atomik.try_unpack(&atomik_pack_literal(&pi1)), Some(pi1));
}

fn planted_pi1_buf() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let pi1 = make_pi(ScreenRes::Low);
    let blob = atomik_pack_literal(&pi1);
    assert_eq!(blob.len(), 36_055);
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
    assert_eq!(f.via(), [PluginId::new("atomik")]);
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
    assert_eq!(f.plugin.as_str(), "atomik-region");
    assert_eq!(f.offset.raw(), 0x0400);
    assert_eq!(f.len, blob.len());
    assert_eq!(f.addr(), Some(Addr::new(0x10400)));
    assert!(f.label.contains("ATM5"));
    assert!(f.label.contains("36055"));
    assert!(f.label.contains("32034"));
}

#[test]
fn detector_reports_and_skips_interior_magic() {
    // A1 planted right after A2's 12-byte header (like Pack-Ice's own
    // precedent): the interior magic is skipped without corrupting A2's
    // own header fields.
    let mut overlap = vec![0u8; 4096];
    plant(&mut overlap, 0x20, A2);
    plant(&mut overlap, 0x2C, A1);
    let mem = MemImage::new(overlap, Addr::new(0));
    let findings = AtomikRegion.scan(&mem);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].offset.raw(), 0x20);

    let mut disjoint = vec![0u8; 4096];
    plant(&mut disjoint, 0x20, A2);
    plant(&mut disjoint, 0x40, A1);
    let mem = MemImage::new(disjoint, Addr::new(0));
    let findings = AtomikRegion.scan(&mem);
    assert_eq!(findings.len(), 2);
    assert_eq!(findings[0].len, 20);
    assert_eq!(findings[1].len, 19);
}

#[test]
fn pic_mode_and_corrupt_are_none() {
    let mut pic_mode = A1.to_vec();
    *pic_mode.last_mut().unwrap() = 0x01;
    assert_eq!(Atomik.try_unpack(&pic_mode), None);

    assert_eq!(Atomik.try_unpack(&A1[..18]), None);

    // Zeroed init byte: no panic either way (Some or None both legal).
    let mut zeroed_init = A1.to_vec();
    let last = zeroed_init.len() - 2;
    zeroed_init[last] = 0x00;
    let _ = Atomik.try_unpack(&zeroed_init);

    // The full corruption sweep of A2's stream bytes is a unit test in
    // `atomik_decode` (only crate-internal code can call `decrunch`
    // directly); here we sweep the full blob through the public API.
    for pos in 0..A2.len() {
        for byte in 0..=u8::MAX {
            let mut mutated = A2.to_vec();
            mutated[pos] = byte;
            let _ = Atomik.try_unpack(&mutated);
        }
    }
}

#[test]
fn cross_scheme_regions() {
    let mut buf = vec![0u8; 64 * 1024];
    plant(&mut buf, 0x100, A2);
    // PP last: its end derivation needs the blob to end the non-zero region.
    plant(&mut buf, 0x2000, PP_P1);

    let atomik_findings = AtomikRegion.scan(&MemImage::new(buf.clone(), Addr::new(0)));
    assert_eq!(atomik_findings.len(), 1);
    assert_eq!(atomik_findings[0].offset.raw(), 0x100);

    let pp_findings = PowerPackerRegion.scan(&MemImage::new(buf.clone(), Addr::new(0)));
    assert_eq!(pp_findings.len(), 1);
    assert_eq!(pp_findings[0].offset.raw(), 0x2000);
    assert_eq!(pp_findings[0].len, 16);

    assert_eq!(Atomik.try_unpack(&buf), Some(b"AAAAAA".to_vec()));
    assert_eq!(PowerPacker.try_unpack(&buf), Some(b"AAAAAA".to_vec()));
}

// Duplicated from tests/integration.rs (separate test crates cannot share
// items without more tests/common/ additions than this one shared helper
// warrants): MJJ config + the "MJJHACK.PRG's own code is running" poll.
fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: format!("/tmp/shiraz-mcp-p9-itest-{name}").into(),
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

/// Load an Atomik-packed PI1 into live ST RAM and prove `rip` finds it
/// through the depacker. Requires Hatari + MJJ — not run by this phase's
/// implementer (another agent owns Hatari); written and compiled only.
#[tokio::test]
#[ignore = "requires a running Hatari + MJJ session (see tests/integration.rs)"]
async fn packed_pi1_live_rip() {
    let config = mjj_config("packed-atomik-live-rip");
    let work_dir = config.work_dir.clone();
    let mut session = Session::new(config).start().await.unwrap();
    wait_until_ready(session.process_mut()).await;
    let mut paused = session.pause().await.unwrap();

    let pi1 = make_pi(ScreenRes::Low);
    let blob = atomik_pack_literal(&pi1);
    let path = work_dir.join("packed.atm");
    std::fs::write(&path, &blob).unwrap();

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
        .find(|f| f.plugin.as_str() == "degas" && f.via() == [PluginId::new("atomik")])
        .unwrap();
    assert_eq!(hit.bytes().unwrap(), pi1.as_slice());

    paused.stop().await;
}
