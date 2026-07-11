//! P21 §10.3 — the Uhane false-positive regression. Models the DHS "Uhane"
//! shape that produced 45 Degas PI1/PI2 false positives (P21 §0): a
//! plausible palette table in RAM followed by non-picture bytes, without
//! needing the live demo. Deterministic xorshift32 PRNG local to this file;
//! `common::{make_pi, plant}` plus a local PAL/pal_bytes copy (graphics.rs
//! precedent).

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "test code, not library code")]

use std::sync::Arc;

use shiraz_mcp::ripper::image::{ScreenRes, SCREEN_BYTES};
use shiraz_mcp::ripper::{default_registry, Finding, RipKind};
use shiraz_mcp::types::{Addr, MemImage};

mod common;
use common::{make_pi, plant};

const PAL: [u16; 16] = [
    0x000, 0x700, 0x070, 0x007, 0x770, 0x707, 0x077, 0x777, 0x333, 0x400, 0x040, 0x004, 0x440,
    0x404, 0x044, 0x555,
];

fn pal_bytes() -> Vec<u8> {
    PAL.iter().flat_map(|w| w.to_be_bytes()).collect()
}

/// Deterministic xorshift32 PRNG — no dependency, reproducible fixtures.
fn xorshift_bytes(seed: u32, n: usize) -> Vec<u8> {
    let mut state = seed.max(1);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(n);
    out
}

/// 32000 zeros with a xorshift byte planted every 64th position — mostly
/// one color, the shape `dominant_junk_penalized` (score.rs) covers in
/// isolation; may pass the gates, but must rank low.
fn zero_heavy_junk(seed: u32) -> Vec<u8> {
    let mut out = vec![0u8; SCREEN_BYTES];
    let noise = xorshift_bytes(seed, SCREEN_BYTES / 64 + 1);
    for (i, chunk) in out.chunks_mut(64).enumerate() {
        chunk[0] = noise[i];
    }
    out
}

/// First 512 bytes = incrementing BE words, rest 0xFF — table-like RAM
/// content (sine tables, palettes) that can alias a Degas body. Kept to 512
/// bytes so palette-gate-passing interior offsets stay few.
fn table_like() -> Vec<u8> {
    let mut out = vec![0xFFu8; SCREEN_BYTES];
    for k in 0..256usize {
        out[2 * k..2 * k + 2].copy_from_slice(&(k as u16).to_be_bytes());
    }
    out
}

/// One planted candidate: a Degas PI header (low-res, plausible palette)
/// followed by `body`.
fn candidate(body: Vec<u8>) -> Vec<u8> {
    let mut out = 0u16.to_be_bytes().to_vec();
    out.extend(pal_bytes());
    out.extend(body);
    out
}

/// 768 KiB, 0xFF fill, 20 planted candidate sites at `4096 + i*32768`
/// (spacing > `PI_TOTAL` = 32034, so footprints never overlap — P21 §10.3
/// erratum): sites 0..8 xorshift noise (must be rejected), 8..14 zero-heavy
/// junk (may pass, must rank low), 14..20 table-like (may pass,
/// dominance-penalized).
fn uhane_like(seed: u32) -> Vec<u8> {
    let mut buf = vec![0xFFu8; 768 * 1024];
    for i in 0..20u32 {
        let body = if i < 8 {
            xorshift_bytes(seed + i, SCREEN_BYTES)
        } else if i < 14 {
            zero_heavy_junk(seed + i)
        } else {
            table_like()
        };
        let site = 4096 + i as usize * 32768;
        plant(&mut buf, site, &candidate(body));
    }
    buf
}

fn degas_findings(buf: Vec<u8>) -> Vec<Finding> {
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    default_registry()
        .scan(mem, Some(RipKind::Picture), 0)
        .into_iter()
        .filter(|f| f.plugin.as_str() == "degas")
        .collect()
}

#[test]
fn noise_only_is_capped_and_low_confidence() {
    let findings = degas_findings(uhane_like(1));
    assert!(findings.len() <= 8, "{} findings", findings.len());
    for f in &findings {
        assert!(f.confidence < Some(90), "{:?}", f.confidence);
    }
    let noise_offsets: Vec<usize> = (0..8u32).map(|i| 4096 + i as usize * 32768).collect();
    for f in &findings {
        assert!(!noise_offsets.contains(&f.offset.raw()));
    }
}

#[test]
fn real_picture_outranks_noise() {
    let mut buf = uhane_like(2);
    plant(&mut buf, 0xA8000, &make_pi(ScreenRes::Low));
    let findings = degas_findings(buf);
    assert!(findings.len() <= 8, "{} findings", findings.len());
    let first = &findings[0];
    assert_eq!(first.offset.raw(), 0xA8000);
    assert!(first.confidence >= Some(95), "{:?}", first.confidence);
}

#[test]
fn packed_like_region_rejected() {
    let mut buf = vec![0xFFu8; 64 * 1024];
    plant(&mut buf, 0x400, &candidate(xorshift_bytes(7, SCREEN_BYTES)));
    assert!(degas_findings(buf).is_empty());
}

#[test]
fn blank_screen_rejected() {
    // The currently-accepted shape P21 exists to kill (G1): a valid res
    // word + palette behind an all-zero screen. Pre-P21 code finds this —
    // that is the red test proving the fix.
    let mut buf = vec![0xFFu8; 64 * 1024];
    plant(&mut buf, 0x400, &candidate(vec![0u8; SCREEN_BYTES]));
    assert!(degas_findings(buf).is_empty());
}
