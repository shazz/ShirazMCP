//! PowerPacker (`PP20`) fixtures + the depack→rescan recursion proof (P9
//! §6.2). No committed binaries: fixtures are byte-pinned builders (a
//! hand-traced vector table + a pinned stream assembler), consistent with
//! ADR-12's fixture policy — see ADR-19.
#![allow(clippy::unwrap_used, reason = "test code, not library code")]

use std::sync::Arc;

use shiraz_mcp::ripper::image::ScreenRes;
use shiraz_mcp::ripper::plugins::powerpacker::{PowerPacker, PowerPackerRegion};
use shiraz_mcp::ripper::{default_registry, extract, Depacker, ExtractFormat, RipKind, RipPlugin};
use shiraz_mcp::types::{Addr, MemImage, PluginId};

mod common;
use common::{make_pi, plant};

// ---- pinned byte vectors (P9 §6.2.2) ----

const P1: &[u8] = &[
    0x50, 0x50, 0x32, 0x30, 0x09, 0x0A, 0x0C, 0x0D, 0x00, 0x1C, 0x10, 0x00, 0x00, 0x00, 0x06, 0x08,
];
const P2: &[u8] = &[
    0x50, 0x50, 0x32, 0x30, 0x09, 0x0A, 0x0C, 0x0D, 0xEA, 0x1A, 0x9A, 0x5A, 0x30, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x04, 0x1B,
];

const EFF: [u8; 4] = [9, 10, 12, 13];

// ---- the pinned stream assembler (P9 §6.2.1) ----

/// A value's bits, MSB-first (the first pushed bit is the value's high bit
/// — the decoder's `roxl.l` fold order).
fn msb(v: u32, n: u32) -> Vec<u8> {
    (0..n).rev().map(|i| ((v >> i) & 1) as u8).collect()
}

/// §4.B.2 serialization: `bits` is one flat list in DECODER-consumption
/// order. `n_longs = bits.len().div_ceil(32)`; `skip = 32*n_longs -
/// bits.len()`. The FIRST-consumed long is the LAST data long in the file
/// and holds `bits[0..32-skip]` at bit positions `skip..32`; each earlier
/// file long holds the next 32 bits at positions `0..32`.
fn pp_assemble(bits: &[u8], out_len: u32) -> Vec<u8> {
    let n_longs = bits.len().div_ceil(32);
    let skip = 32 * n_longs - bits.len();
    let mut longs: Vec<u32> = Vec::with_capacity(n_longs);
    let mut idx = 0usize;
    for i in 0..n_longs {
        let count = if i == 0 { 32 - skip } else { 32 };
        let mut word: u32 = 0;
        for j in 0..count {
            let bit_pos = if i == 0 { skip + j } else { j };
            word |= u32::from(bits[idx]) << bit_pos;
            idx += 1;
        }
        longs.push(word);
    }
    let mut out = b"PP20".to_vec();
    out.extend(EFF);
    for w in longs.iter().rev() {
        out.extend(w.to_be_bytes());
    }
    out.extend(((out_len << 8) | skip as u32).to_be_bytes());
    out
}

/// Literal-only: flag 0, chunked 2-bit count (emit 3s, final chunk < 3),
/// then each byte as 8 bits — payload REVERSED (output is backward).
fn pp_pack_literal(payload: &[u8]) -> Vec<u8> {
    let mut bits = vec![0u8];
    let mut d2 = payload.len() - 1;
    while d2 >= 3 {
        bits.extend(msb(3, 2));
        d2 -= 3;
    }
    bits.extend(msb(d2 as u32, 2));
    for &b in payload.iter().rev() {
        bits.extend(msb(u32::from(b), 8));
    }
    pp_assemble(&bits, payload.len() as u32)
}

// ---- misc helpers ----

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shiraz-mcp-p9-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

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
    assert_eq!(PowerPacker.try_unpack(P1), Some(b"AAAAAA".to_vec()));
    assert_eq!(PowerPacker.try_unpack(P2), Some(b"WXYZ".to_vec()));
}

#[test]
fn assembler_matches_pinned_vectors() {
    let mut bits = vec![0u8];
    bits.extend(msb(0, 2));
    bits.extend(msb(0x41, 8));
    bits.extend(msb(3, 2));
    bits.push(0);
    bits.extend(msb(0, 7));
    bits.extend(msb(0, 3));
    assert_eq!(pp_assemble(&bits, 6), P1);

    assert_eq!(pp_pack_literal(b"WXYZ"), P2);
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
        assert_eq!(PowerPacker.try_unpack(&pp_pack_literal(&p)), Some(p));
    }
    let pi1 = make_pi(ScreenRes::Low);
    assert_eq!(PowerPacker.try_unpack(&pp_pack_literal(&pi1)), Some(pi1));
}

fn planted_pi1_buf() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let pi1 = make_pi(ScreenRes::Low);
    let blob = pp_pack_literal(&pi1);
    assert_eq!(blob.len(), 34_716);
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
    assert_eq!(f.via(), [PluginId::new("powerpacker")]);
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
    assert_eq!(f.plugin.as_str(), "powerpacker-region");
    assert_eq!(f.offset.raw(), 0x0400);
    assert_eq!(f.len, blob.len());
    assert!(f.label.contains("PP20"));
    assert!(f.label.contains("34716"));
    assert!(f.label.contains("32034"));
    assert!(f.label.contains("end estimated"));
}

#[test]
fn end_derivation_buffer_end() {
    // The blob ends exactly at the buffer end: e1 == e2 (whole-file case).
    let mut buf = vec![0u8; 0x1000];
    plant(&mut buf, 0x1000 - 16, P1);
    assert_eq!(PowerPacker.try_unpack(&buf), Some(b"AAAAAA".to_vec()));

    // One zero byte after the blob: the zero-trimmed end still rounds up
    // to the real end relative to the (shifted) offset.
    let mut buf2 = vec![0u8; 0x1000];
    plant(&mut buf2, 0x1000 - 17, P1);
    assert_eq!(PowerPacker.try_unpack(&buf2), Some(b"AAAAAA".to_vec()));
}

#[test]
fn end_derivation_junk_after_blob_misses() {
    let mut buf = vec![0u8; 4096];
    plant(&mut buf, 0x100, P1);
    // 0x803, not 0x800: aligned so the junk byte lands as the derived
    // trailer's LAST byte (the `skip`/`out_len` low byte), forcing
    // out_len == 0 — both candidate ends then have bad trailers.
    buf[0x803] = 0xFF;
    assert_eq!(PowerPacker.try_unpack(&buf), None);
    let mem = MemImage::new(buf, Addr::new(0));
    assert!(PowerPackerRegion.scan(&mem).is_empty());
}

#[test]
fn eff_gate_rejects_text() {
    let buf = b"PP20 is a packer!".to_vec();
    assert_eq!(PowerPacker.try_unpack(&buf), None);
    let mem = MemImage::new(buf, Addr::new(0));
    assert!(PowerPackerRegion.scan(&mem).is_empty());
}

#[test]
fn corruption_never_panics() {
    for pos in 0..P1.len() {
        for byte in 0..=u8::MAX {
            let mut mutated = P1.to_vec();
            mutated[pos] = byte;
            let _ = PowerPacker.try_unpack(&mutated);
        }
    }
}
