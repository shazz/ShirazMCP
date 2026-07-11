//! Golden picture decode tests + planted-rip engine tests (P4.md §8.2). No
//! Hatari: all fixtures are generated deterministically by the builders
//! below — no committed binaries. Archive images have unclear
//! redistribution status, and a byte-pinned builder IS the golden reference
//! (the expected RGBA of every pixel is known analytically); ADR-12.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "test code, not library code")]

use std::sync::Arc;

use shiraz_mcp::ripper::image::{decode_planar, Palette, ScreenRes, SCREEN_BYTES};
use shiraz_mcp::ripper::{default_registry, extract_png, RipKind};
use shiraz_mcp::types::{Addr, MemImage};

// ---- pinned fixture constants + builders (P4.md §8.2) ----

/// 16 distinct ST palette words (upper nibbles clear, distinct >= 4).
const PAL: [u16; 16] = [
    0x000, 0x700, 0x070, 0x007, 0x770, 0x707, 0x077, 0x777, 0x333, 0x400, 0x040, 0x004, 0x440,
    0x404, 0x044, 0x555,
];

/// ST 3-bit channel -> 8-bit, the same integer math as image.rs.
fn st8(v: u16) -> u8 {
    (v * 255 / 7) as u8
}

fn rgba(word: u16) -> [u8; 4] {
    [
        st8((word >> 8) & 7),
        st8((word >> 4) & 7),
        st8(word & 7),
        255,
    ]
}

fn pal_bytes() -> Vec<u8> {
    PAL.iter().flat_map(|w| w.to_be_bytes()).collect()
}

/// chunky->planar encoder (test-side inverse of decode_planar): for each
/// row, group, plane, build the word MSB-first (pixel 0 = bit 15).
fn planar_body(res: ScreenRes, idx: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    let (planes, bpl) = (res.planes(), res.bytes_per_line());
    let mut out = vec![0u8; SCREEN_BYTES];
    for y in 0..res.height() {
        for g in 0..res.width() / 16 {
            for pl in 0..planes {
                let mut w = 0u16;
                for p in 0..16 {
                    w = (w << 1) | u16::from((idx(g * 16 + p, y) >> pl) & 1);
                }
                let off = y as usize * bpl + g as usize * planes * 2 + pl * 2;
                out[off..off + 2].copy_from_slice(&w.to_be_bytes());
            }
        }
    }
    out
}

/// Pinned test images: low -> 16 vertical bands (idx = x/20); med -> 4
/// bands (idx = x/160); high -> alternating 40px bands (idx = (x/40) % 2).
fn bands(res: ScreenRes) -> Vec<u8> {
    match res {
        ScreenRes::Low => planar_body(res, |x, _y| (x / 20) as u8),
        ScreenRes::Med => planar_body(res, |x, _y| (x / 160) as u8),
        ScreenRes::High => planar_body(res, |x, _y| ((x / 40) % 2) as u8),
    }
}

fn res_word(res: ScreenRes) -> u16 {
    match res {
        ScreenRes::Low => 0,
        ScreenRes::Med => 1,
        ScreenRes::High => 2,
    }
}

fn make_pi(res: ScreenRes) -> Vec<u8> {
    let mut out = res_word(res).to_be_bytes().to_vec();
    out.extend(pal_bytes());
    out.extend(bands(res));
    out
}

fn make_neo() -> Vec<u8> {
    let mut out = Vec::with_capacity(128 + SCREEN_BYTES);
    out.extend_from_slice(&0u16.to_be_bytes()); // flag
    out.extend_from_slice(&0u16.to_be_bytes()); // res = low
    out.extend(pal_bytes());
    out.extend([b' '; 12]); // filename
    for _ in 0..5 {
        out.extend_from_slice(&0u16.to_be_bytes()); // anim limits + reserved
    }
    out.extend_from_slice(&320u16.to_be_bytes());
    out.extend_from_slice(&200u16.to_be_bytes());
    out.extend(vec![0u8; 66]);
    assert_eq!(out.len(), 128);
    out.extend(bands(ScreenRes::Low));
    assert_eq!(out.len(), 128 + SCREEN_BYTES);
    out
}

/// Standard interleaved -> Degas-linear (inverse of degas.rs's
/// `reinterleave`).
fn interleaved_to_linear(inter: &[u8], res: ScreenRes) -> Vec<u8> {
    let (planes, bpl) = (res.planes(), res.bytes_per_line());
    let plane_line = bpl / planes;
    let mut out = vec![0u8; SCREEN_BYTES];
    for r in 0..SCREEN_BYTES / bpl {
        for p in 0..planes {
            for k in 0..plane_line {
                let src = r * bpl + (k / 2) * 2 * planes + p * 2 + (k & 1);
                out[r * bpl + p * plane_line + k] = inter[src];
            }
        }
    }
    out
}

/// Length of the run of bytes identical to `data[i]`, capped at 128 (the
/// RLE format's max block size).
fn run_length_at(data: &[u8], i: usize) -> usize {
    let b = data[i];
    let mut n = 1;
    while i + n < data.len() && n < 128 && data[i + n] == b {
        n += 1;
    }
    n
}

/// Deterministic test RLE encoder (valid Degas Elite stream, not optimal):
/// runs of >= 3 equal bytes become run blocks (control 257-n); everything
/// else becomes literal blocks (control n-1, both <= 128). Never emits 0x80.
fn rle_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let run = run_length_at(data, i);
        if run >= 3 {
            out.push((257 - run) as u8);
            out.push(data[i]);
            i += run;
            continue;
        }
        let start = i;
        let mut len = 0usize;
        while i < data.len() && len < 128 && run_length_at(data, i) < 3 {
            i += 1;
            len += 1;
        }
        out.push((len - 1) as u8);
        out.extend_from_slice(&data[start..start + len]);
    }
    out
}

fn make_pc1() -> Vec<u8> {
    let mut out = 0x8000u16.to_be_bytes().to_vec();
    out.extend(pal_bytes());
    let linear = interleaved_to_linear(&bands(ScreenRes::Low), ScreenRes::Low);
    out.extend(rle_encode(&linear));
    out
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shiraz-mcp-p4-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn plant(buf: &mut [u8], at: usize, data: &[u8]) {
    buf[at..at + data.len()].copy_from_slice(data);
}

/// The pixel checks common to every low-res golden decode: (0,0)=PAL[0],
/// (25,0)=PAL[1], (319,199)=PAL[15].
fn assert_low_res_pixels(rgba8: &image::RgbaImage) {
    assert_eq!(rgba8.get_pixel(0, 0).0, rgba(PAL[0]));
    assert_eq!(rgba8.get_pixel(25, 0).0, rgba(PAL[1]));
    assert_eq!(rgba8.get_pixel(319, 199).0, rgba(PAL[15]));
}

// ---- tests ----

#[test]
fn pi1_planted_scan_and_golden() {
    // 0xFF filler, not 0x00: a zero res word is itself a valid Low header,
    // so an all-zero preamble can straddle-match the picture's leading
    // palette bytes a few bytes early. 0xFFFF is never a valid res word,
    // so every padding-only offset is rejected at the very first check.
    let mut buf = vec![0xFFu8; 64 * 1024];
    let pic = make_pi(ScreenRes::Low);
    plant(&mut buf, 0x400, &pic);
    let mem = Arc::new(MemImage::new(buf, Addr::new(0x10000)));
    let registry = default_registry();
    let findings = registry.scan(mem, Some(RipKind::Picture), 0);

    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.plugin.as_str(), "degas");
    assert_eq!(f.offset.raw(), 0x400);
    assert_eq!(f.len, pic.len());
    assert_eq!(f.addr(), Some(Addr::new(0x10400)));
    assert!(f.label.contains("PI1"));

    let png = registry.render_png(f.plugin, f.bytes().unwrap()).unwrap();
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (320, 200));
    assert_low_res_pixels(&decoded.to_rgba8());
}

#[test]
fn pi2_pi3_golden_decode() {
    let pal = Palette::from_words(&PAL);

    let med = decode_planar(&bands(ScreenRes::Med), ScreenRes::Med, &pal).unwrap();
    assert_eq!(med.get_pixel(0, 0).0, rgba(PAL[0]));
    assert_eq!(med.get_pixel(639, 0).0, rgba(PAL[3]));

    let high = decode_planar(&bands(ScreenRes::High), ScreenRes::High, &pal).unwrap();
    assert_eq!(high.get_pixel(0, 0).0, rgba(PAL[0]));
    assert_eq!(high.get_pixel(40, 0).0, rgba(PAL[1]));
    assert_eq!((high.width(), high.height()), (640, 400));
}

#[test]
fn pc1_roundtrip_equals_pi1() {
    let mut buf = vec![0u8; 64 * 1024];
    let pc1 = make_pc1();
    plant(&mut buf, 0x400, &pc1);
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let registry = default_registry();
    let findings = registry.scan(mem, Some(RipKind::Picture), 0);

    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert!(f.label.contains("PC1"));
    let compressed_len = pc1.len() - 34;
    assert_eq!(f.len, 34 + compressed_len);

    let pc1_png = registry.render_png(f.plugin, f.bytes().unwrap()).unwrap();
    let pi1_png = registry
        .render_png(f.plugin, &make_pi(ScreenRes::Low))
        .unwrap();
    let pc1_rgba = image::load_from_memory(&pc1_png).unwrap().to_rgba8();
    let pi1_rgba = image::load_from_memory(&pi1_png).unwrap().to_rgba8();
    assert_eq!(pc1_rgba.into_raw(), pi1_rgba.into_raw());
}

#[test]
fn neo_planted_scan_and_golden() {
    // See pi1_planted_scan_and_golden for why 0xFF, not 0x00.
    let mut buf = vec![0xFFu8; 64 * 1024];
    let neo = make_neo();
    plant(&mut buf, 0x800, &neo);
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let registry = default_registry();
    let findings = registry.scan(mem, Some(RipKind::Picture), 0);

    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.plugin.as_str(), "neochrome");
    assert_eq!(f.len, neo.len());

    let png = registry.render_png(f.plugin, f.bytes().unwrap()).unwrap();
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (320, 200));
    assert_low_res_pixels(&decoded.to_rgba8());
}

#[test]
fn zeroed_and_patterned_ram_scan_is_empty() {
    let zeros = vec![0u8; 256 * 1024];
    let pattern: Vec<u8> = std::iter::repeat_n([0u8, 1u8], 128 * 1024)
        .flatten()
        .collect();
    for buf in [zeros, pattern] {
        let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
        let findings = default_registry().scan(mem, None, 2);
        assert!(findings.iter().all(|f| f.kind != RipKind::Picture));
    }
}

#[test]
fn extract_png_writes_standard_name() {
    // See pi1_planted_scan_and_golden for why 0xFF, not 0x00.
    let mut buf = vec![0xFFu8; 64 * 1024];
    let pic = make_pi(ScreenRes::Low);
    plant(&mut buf, 0x400, &pic);
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let registry = default_registry();
    let findings = registry.scan(mem, Some(RipKind::Picture), 0);
    let f = findings
        .into_iter()
        .find(|f| f.plugin.as_str() == "degas")
        .unwrap();
    let png = registry.render_png(f.plugin, f.bytes().unwrap()).unwrap();

    let dir = temp_dir("extract-png");
    let extracted = extract_png(&f, &png, &dir, 1).unwrap();
    assert_eq!(extracted.path, dir.join("rip-0001-degas-000400.png"));
    assert_eq!(extracted.size, png.len() as u64);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_pictures_two_findings() {
    // See pi1_planted_scan_and_golden for why 0xFF, not 0x00.
    let mut buf = vec![0xFFu8; 128 * 1024];
    plant(&mut buf, 0x400, &make_pi(ScreenRes::Low));
    plant(&mut buf, 0x9000, &make_neo());
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Picture), 0);

    assert_eq!(findings.len(), 2);
    assert_eq!(findings[0].plugin.as_str(), "degas");
    assert_eq!(findings[1].plugin.as_str(), "neochrome");
}
