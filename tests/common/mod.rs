//! Shared integration-test fixtures (P5 §6.4 / P9 §6.0): the Degas PI1
//! builder and the byte-plant helper, moved here byte-identical once a
//! third consumer (`depack_atomik.rs`) appeared — `depack_rescan.rs`'s own
//! comment set this trigger. `graphics.rs` keeps its own copies (entangled
//! with PC/RLE tests, a later cleanup).
#![allow(
    dead_code,
    reason = "shared test fixtures; each test crate uses a subset"
)]

use shiraz_mcp::ripper::image::{ScreenRes, SCREEN_BYTES};

pub const PAL: [u16; 16] = [
    0x000, 0x700, 0x070, 0x007, 0x770, 0x707, 0x077, 0x777, 0x333, 0x400, 0x040, 0x004, 0x440,
    0x404, 0x044, 0x555,
];

pub fn pal_bytes() -> Vec<u8> {
    PAL.iter().flat_map(|w| w.to_be_bytes()).collect()
}

pub fn planar_body(res: ScreenRes, idx: impl Fn(u32, u32) -> u8) -> Vec<u8> {
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

pub fn bands(res: ScreenRes) -> Vec<u8> {
    match res {
        ScreenRes::Low => planar_body(res, |x, _y| (x / 20) as u8),
        ScreenRes::Med => planar_body(res, |x, _y| (x / 160) as u8),
        ScreenRes::High => planar_body(res, |x, _y| ((x / 40) % 2) as u8),
    }
}

pub fn make_pi(res: ScreenRes) -> Vec<u8> {
    let res_word: u16 = match res {
        ScreenRes::Low => 0,
        ScreenRes::Med => 1,
        ScreenRes::High => 2,
    };
    let mut out = res_word.to_be_bytes().to_vec();
    out.extend(pal_bytes());
    out.extend(bands(res));
    out
}

pub fn plant(buf: &mut [u8], at: usize, data: &[u8]) {
    buf[at..at + data.len()].copy_from_slice(data);
}
