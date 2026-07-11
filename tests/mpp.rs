//! MPP golden multi-palette decode tests + planted-rip engine tests (P13
//! §7). No Hatari: all fixtures are generated deterministically by the
//! builders below — no committed binaries; a byte-pinned builder IS the
//! golden reference (ADR-12 fixture policy, same rationale as
//! `tests/graphics.rs`).

#![allow(clippy::unwrap_used, reason = "test code, not library code")]

use std::sync::Arc;

use shiraz_mcp::ripper::plugins::mpp::Mpp;
use shiraz_mcp::ripper::{default_registry, RipKind, RipPlugin};
use shiraz_mcp::types::{Addr, MemImage};

const RED: u16 = 0b111_000_000;
const GREEN: u16 = 0b000_111_000;
const BLUE: u16 = 0b000_000_111;
const WHITE: u16 = 0b111_111_111;

/// ST 3-bit -> 8-bit, the same integer math as the decoder (v*255/7).
fn st8(v: u16) -> u8 {
    (v * 255 / 7) as u8
}
fn rgba9(w: u16) -> [u8; 4] {
    [st8(w >> 6 & 7), st8(w >> 3 & 7), st8(w & 7), 255]
}

/// MSB-first bit writer — the exact mirror of mpp_decode's BitReader.
struct BitWriter {
    buf: Vec<u8>,
    bit: usize,
}
impl BitWriter {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            bit: 0,
        }
    }
    fn push(&mut self, v: u16, n: usize) {
        for i in (0..n).rev() {
            if self.bit == 0 {
                self.buf.push(0);
            }
            *self.buf.last_mut().unwrap() |= (((v >> i) & 1) as u8) << (7 - self.bit);
            self.bit = (self.bit + 1) % 8;
        }
    }
    /// Pad to the 16-bit boundary the palette-section size formula rounds to.
    fn finish(mut self) -> Vec<u8> {
        if !self.buf.len().is_multiple_of(2) {
            self.buf.push(0);
        }
        self.buf
    }
}

fn mpp_header(mode: u8, flags: u8) -> Vec<u8> {
    let mut out = b"MPP".to_vec();
    out.extend([mode, flags, 0, 0, 0]); // reserved
    out.extend(0u32.to_be_bytes()); // extra_len 0
    out
}

/// Mode-2 ST/STE file, parameterized by entry bit width + flags byte.
/// `entry(y, k)` = stream value for line y, stream index k: k 0..=14 are the
/// line-start reloads (slots 1..=15); k 15..=46 are in-line changes 0..=31;
/// k 47..=53 are changes 33..=39 — change 32 is the forced-black border and
/// is NOT in the stream (§4.4).
fn build_mode2(
    bits: usize,
    flags: u8,
    entry: impl Fn(usize, usize) -> u16,
    bitmap_line: &[u8; 160],
) -> Vec<u8> {
    let mut out = mpp_header(2, flags);
    let mut w = BitWriter::new();
    for y in 0..199 {
        for k in 0..54 {
            w.push(entry(y, k), bits);
        }
    }
    let pal = w.finish();
    out.extend(pal);
    for _ in 0..199 {
        out.extend_from_slice(bitmap_line);
    }
    out
}

/// Mode-3 ST file. Stream indices per line: k 0..=9 line-start (slots
/// 6..=15), k 10..=47 in-line changes 0..=37 (none forced in mode 3).
fn build_mode3(entry: impl Fn(usize, usize) -> u16, bitmap_line: &[u8; 208]) -> Vec<u8> {
    let mut out = mpp_header(3, 0);
    let mut w = BitWriter::new();
    for y in 0..273 {
        for k in 0..48 {
            w.push(entry(y, k), 9);
        }
    }
    let pal = w.finish();
    assert_eq!(pal.len(), 14742); // pinned §4.5 (exact multiple, 0 pad bits)
    out.extend(pal);
    for _ in 0..273 {
        out.extend_from_slice(bitmap_line);
    }
    assert_eq!(out.len(), 71538); // pinned §4.5
    out
}

#[test]
fn mode2_golden_multi_palette() {
    let entry = |y: usize, k: usize| match y {
        0 => {
            if k == 16 {
                BLUE
            } else {
                RED
            }
        }
        1 => GREEN,
        198 => WHITE,
        _ => 0,
    };
    let mut line = [0u8; 160];
    line[0] = 0xFF;
    line[1] = 0xFF;
    let file = build_mode2(9, 0, entry, &line);
    assert_eq!(file.len(), 43942); // pinned §4.5
    let png = Mpp.render_png(&file).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (320, 199));
    let px = |x, y| img.get_pixel(x, y).0;
    assert_eq!(px(0, 0), rgba9(RED)); //     line-start slot 1
    assert_eq!(px(11, 0), rgba9(RED)); //    last pixel before change 1
    assert_eq!(px(12, 0), rgba9(BLUE)); //   in-line change 1 fires AT x=12
    assert_eq!(px(15, 0), rgba9(BLUE));
    assert_eq!(px(16, 0), rgba9(RED)); //    idx 0: change 0 set slot 0 at x=4
    assert_eq!(px(259, 0), rgba9(RED)); //   slot 0 as of x<260
    assert_eq!(px(260, 0), [0, 0, 0, 255]); // change 32: forced border black
    assert_eq!(px(0, 1), rgba9(GREEN)); //   line 1 reloaded its own palette
    assert_eq!(px(260, 1), [0, 0, 0, 255]);
    assert_eq!(px(0, 100), [0, 0, 0, 255]);
    assert_eq!(px(0, 198), rgba9(WHITE)); // stream in sync after 199 lines
}

#[test]
fn mode3_palette_persists_across_lines() {
    let entry = |y: usize, _k: usize| if y == 0 { RED } else { GREEN };
    let mut line = [0u8; 208];
    // group 0 and group 6 have plane-0 AND plane-1 words 0xFFFF -> index 3
    // at x 0..=15 and 96..=111.
    line[0..4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    line[48..52].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    let file = build_mode3(entry, &line);
    let png = Mpp.render_png(&file).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (416, 273));
    let px = |x, y| img.get_pixel(x, y).0;
    assert_eq!(px(0, 0), [0, 0, 0, 255]); //  slot 3 untouched before x=81
    assert_eq!(px(96, 0), rgba9(RED)); //     line-0 change 3 took effect
    assert_eq!(px(0, 1), rgba9(RED)); //      PERSISTED across the line boundary
    assert_eq!(px(96, 1), rgba9(GREEN)); //   line-1 change 3 overwrote it
}

#[test]
fn ste12_flat_gray() {
    let entry = |_y: usize, _k: usize| 0x888u16;
    // All planes set -> pixel index 15 everywhere, which line-start reload
    // and the cc==15/cc==31 in-line changes both keep gray; unlike index 0
    // (slot 0), index 15 is never hit by mode 2's forced-black border (cc
    // 32 only ever rewrites slot 0), so this fixture is genuinely flat.
    let line = [0xFFu8; 160];
    let file = build_mode2(12, 1, entry, &line);
    assert_eq!(file.len(), 47972); // pinned §4.5
    let png = Mpp.render_png(&file).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    let px = |x, y| img.get_pixel(x, y).0;
    assert_eq!(px(0, 0), [17, 17, 17, 255]);
    assert_eq!(px(160, 100), [17, 17, 17, 255]);
    assert_eq!(px(319, 198), [17, 17, 17, 255]);
}

#[test]
fn planted_scan_and_render_path() {
    let entry = |y: usize, k: usize| match y {
        0 => {
            if k == 16 {
                BLUE
            } else {
                RED
            }
        }
        1 => GREEN,
        198 => WHITE,
        _ => 0,
    };
    let mut line = [0u8; 160];
    line[0] = 0xFF;
    line[1] = 0xFF;
    let file = build_mode2(9, 0, entry, &line);

    let mut buf = vec![0xFFu8; 128 * 1024];
    let at = 0x1001;
    buf[at..at + file.len()].copy_from_slice(&file);
    let mem = Arc::new(MemImage::new(buf, Addr::new(0x40000)));
    let findings = default_registry().scan(mem, Some(RipKind::Picture), 0);
    let mpp_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.plugin.as_str() == "mpp")
        .collect();
    assert_eq!(mpp_findings.len(), 1);
    let f = mpp_findings[0];
    assert_eq!(f.offset.raw(), at);
    assert_eq!(f.len, 43942);
    assert_eq!(f.addr(), Some(Addr::new(0x41001)));
    assert!(f.label.contains("MPP mode 2"));
    assert!(f.label.contains("blitter"));

    let png = default_registry()
        .render_png(f.plugin, f.bytes().unwrap())
        .unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (320, 199));
    assert_eq!(img.get_pixel(12, 0).0, rgba9(BLUE));
}

#[test]
fn zero_and_patterned_ram_yield_nothing() {
    let zeros = vec![0u8; 256 * 1024];
    let pattern: Vec<u8> = std::iter::repeat_n([0x4Du8, 0x50u8], 128 * 1024)
        .flatten()
        .collect();
    for buf in [zeros, pattern] {
        let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
        let findings = default_registry().scan(mem, Some(RipKind::Picture), 0);
        assert!(findings.iter().all(|f| f.plugin.as_str() != "mpp"));
    }
}

#[test]
fn truncated_plant_yields_nothing() {
    let entry = |_y: usize, _k: usize| 0u16;
    let line = [0u8; 160];
    let file = build_mode2(9, 0, entry, &line);
    let truncated = &file[..file.len() - 100];
    let mut buf = vec![0u8; truncated.len() + 4096];
    let at = buf.len() - truncated.len();
    buf[at..].copy_from_slice(truncated);
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Picture), 0);
    assert!(findings.iter().all(|f| f.plugin.as_str() != "mpp"));
}

#[test]
fn render_truncated_is_none() {
    let entry = |_y: usize, _k: usize| 0u16;
    let line = [0u8; 160];
    let file = build_mode2(9, 0, entry, &line);
    assert!(Mpp.render_png(&file[..20000]).is_none());
}

#[test]
fn scan_skips_interior_magics() {
    let entry = |_y: usize, _k: usize| 0u16;
    let line = [0u8; 160];
    let mut file = build_mode2(9, 0, entry, &line);
    // Plant an extra "MPP" inside the bitmap region (well past the header).
    let bitmap_start = file.len() - 160 * 199;
    file[bitmap_start + 10..bitmap_start + 13].copy_from_slice(b"MPP");

    let mut buf = vec![0xFFu8; 40 + file.len() + 64];
    buf[40..40 + file.len()].copy_from_slice(&file);
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let findings = Mpp.scan(&mem);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].offset.raw(), 40);
}
