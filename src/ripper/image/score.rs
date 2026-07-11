//! Structural plausibility scoring for decoded ST planar pictures (P21 §4).
//! The DHS "Uhane" run produced 45 Degas PI1/PI2 false positives: the header
//! is a res word (0-2) plus 32 bytes passing `plausible_palette`, which any
//! RAM table of small big-endian words matches, and the 32000 bytes behind
//! it decode as planar noise. Reference decoders (RECOIL, Atari_ST_Sources
//! DEGAS.C) identify Degas by filename extension + exact length — a RAM
//! scan has neither, so this module scores the decoded pixels instead.
//! Thresholds carry `// verify live` markers — tune against the Uhane RAM
//! once the emulator frees up. Cost: ~20k ops/candidate, only on candidates
//! already past `plausible_palette` — 45 on the real run, sub-ms total; no
//! staged cheap-gate until live data says it's needed.

use super::{render::row_indices, ScreenRes, SCREEN_BYTES};

/// Structural plausibility of 32000 bytes as an ST planar picture (§P21.4).
#[derive(Debug, Clone, Copy)]
pub struct PictureScore {
    /// 0-100 composite (§P21.4.5) — what `Finding.confidence` carries.
    pub confidence: u8,
    /// Distinct palette indices in the sampled rows.
    pub used_colors: u8,
    /// Horizontal index-coherence above the 1/2^planes random baseline.
    pub h_excess: f32,
    /// Byte self-similarity at 1- and 2-line strides (max of the two).
    pub v_coherence: f32,
    /// Sampled Shannon entropy, bits/byte.
    pub entropy: f32,
    /// Share of the most common index (blank-screen detector).
    pub dominance: f32,
}

impl PictureScore {
    /// Accept/reject gates G1-G3 (§P21.4.4). Thresholds carry
    /// `// verify live` markers — tune against the Uhane RAM when the
    /// emulator frees up; keep fixtures in sync if they move.
    pub fn plausible(&self) -> bool {
        // G1: blank bodies ("cleared VRAM behind a resident palette").
        if self.used_colors < 2 {
            return false;
        }
        // G2: no spatial structure (random data, most packed streams, code).
        if self.v_coherence < 0.04 && self.h_excess < 0.05 {
            return false; // verify live
        }
        // G3: high-entropy packed data slipping G2 on a borderline h_excess.
        if self.entropy > 7.5 && self.v_coherence < 0.10 {
            return false; // verify live
        }
        true
    }
}

/// Score `screen` as an interleaved planar picture. None iff shorter than
/// SCREEN_BYTES (callers normally pre-slice; PC callers pass a fresh
/// reinterleaved buffer). Pure; ~20k ops (§P21.4.6).
pub fn score_screen(screen: &[u8], res: ScreenRes) -> Option<PictureScore> {
    let screen = screen.get(..SCREEN_BYTES)?;
    let (hist, h_coherence) = row_stats(screen, res);
    let planes = res.planes();
    let baseline = 1.0 / (1u32 << planes) as f32;
    let h_excess = (h_coherence - baseline).max(0.0);
    let v_coherence = v_coherence(screen, res.bytes_per_line());
    let entropy = sampled_entropy(screen);

    let total: u32 = hist.iter().sum();
    let used_colors = hist.iter().filter(|&&c| c > 0).count() as u8;
    let dominance = hist.iter().copied().max().unwrap_or(0) as f32 / total.max(1) as f32;

    let s_v = (v_coherence / 0.30).min(1.0);
    let s_h = (h_excess / 0.40).min(1.0);
    let s_pal = (used_colors as f32 / (1u32 << planes) as f32).min(1.0);
    let blank_penalty = 1.0 - ((dominance - 0.85).max(0.0) / 0.15) * 0.6;
    let confidence = (100.0 * (0.40 * s_v + 0.30 * s_h + 0.30 * s_pal) * blank_penalty)
        .round()
        .clamp(0.0, 100.0) as u8;

    Some(PictureScore {
        confidence,
        used_colors,
        h_excess,
        v_coherence,
        entropy,
        dominance,
    })
}

/// 25 sampled scanlines → (16-bucket histogram, pooled horizontal
/// coherence — fraction of equal adjacent-pixel-pairs).
fn row_stats(screen: &[u8], res: ScreenRes) -> ([u32; 16], f32) {
    let bpl = res.bytes_per_line();
    let step = (res.height() as usize / 25).max(1);
    let mut hist = [0u32; 16];
    let mut equal_pairs = 0u32;
    let mut total_pairs = 0u32;
    let mut indices = Vec::with_capacity(res.width() as usize);
    for y in (0..res.height() as usize).step_by(step) {
        indices.clear();
        row_indices(&screen[y * bpl..][..bpl], res, &mut indices);
        for &idx in &indices {
            hist[(idx & 15) as usize] += 1;
        }
        for pair in indices.windows(2) {
            total_pairs += 1;
            if pair[0] == pair[1] {
                equal_pairs += 1;
            }
        }
    }
    (hist, equal_pairs as f32 / total_pairs.max(1) as f32)
}

/// Byte self-similarity at 1- and 2-line strides (max of the two): real
/// pictures repeat vertically, and 2x2 ordered dither repeats at period 2.
/// Stride 53 is prime, coprime to 160/320/80 — no phase-lock with the grid.
fn v_coherence(screen: &[u8], bpl: usize) -> f32 {
    let match_ratio = |stride: usize| -> f32 {
        let mut matches = 0u32;
        let mut total = 0u32;
        let mut k = 0usize;
        while k + stride < SCREEN_BYTES {
            total += 1;
            if screen[k] == screen[k + stride] {
                matches += 1;
            }
            k += 53;
        }
        matches as f32 / total.max(1) as f32
    };
    match_ratio(bpl).max(match_ratio(2 * bpl))
}

/// Shannon entropy in bits/byte, sampled every 8th byte (4000 samples).
fn sampled_entropy(screen: &[u8]) -> f32 {
    let mut hist = [0u32; 256];
    let mut n = 0u32;
    for &b in screen.iter().step_by(8) {
        hist[b as usize] += 1;
        n += 1;
    }
    let n = n.max(1) as f32;
    -hist
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f32 / n;
            p * p.log2()
        })
        .sum::<f32>()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn blank_fails_used_colors() {
        let screen = vec![0u8; SCREEN_BYTES];
        let s = score_screen(&screen, ScreenRes::Low).unwrap();
        assert_eq!(s.used_colors, 1);
        assert!(!s.plausible());
    }

    #[test]
    fn solid_fails_used_colors() {
        let screen = vec![0xFFu8; SCREEN_BYTES];
        let s = score_screen(&screen, ScreenRes::Low).unwrap();
        assert!(!s.plausible());
    }

    #[test]
    fn random_fails_structure() {
        let screen = xorshift_bytes(1, SCREEN_BYTES);
        let s = score_screen(&screen, ScreenRes::Low).unwrap();
        assert!(!s.plausible());
        assert!(s.entropy > 7.5);
    }

    #[test]
    fn random_med_fails_structure() {
        let screen = xorshift_bytes(2, SCREEN_BYTES);
        let s = score_screen(&screen, ScreenRes::Med).unwrap();
        assert!(!s.plausible());
    }

    /// Every line = 80x0xFF + 80x0x00 (colors 15|0) — gate-passing low-res
    /// body shared with degas.rs's `body_halves` fixture.
    fn body_halves() -> Vec<u8> {
        let mut out = vec![0u8; SCREEN_BYTES];
        for line in out.chunks_mut(160) {
            line[..80].fill(0xFF);
        }
        out
    }

    #[test]
    fn halves_passes() {
        let s = score_screen(&body_halves(), ScreenRes::Low).unwrap();
        assert!(s.plausible());
        assert_eq!(s.used_colors, 2);
        assert!((60..=90).contains(&s.confidence), "{}", s.confidence);
    }

    /// Plane 0 lines alternate 0xAA/0x55 bytes, planes 1-3 zero: a 2x2
    /// ordered-dither pattern that only the v_coherence 2-line arm scores
    /// (the plane-0 word repeats every group of 8 bytes across the line).
    fn dither_body() -> Vec<u8> {
        let mut out = vec![0u8; SCREEN_BYTES];
        for (line_idx, line) in out.chunks_mut(160).enumerate() {
            let fill = if line_idx % 2 == 0 { 0xAA } else { 0x55 };
            for group in line.chunks_mut(8) {
                group[0] = fill;
                group[1] = fill;
            }
        }
        out
    }

    #[test]
    fn dither_passes_via_v2() {
        let s = score_screen(&dither_body(), ScreenRes::Low).unwrap();
        assert!(s.plausible());
        assert!((30..=60).contains(&s.confidence), "{}", s.confidence);
    }

    /// Zeros with a xorshift byte planted every 64th position — mostly one
    /// color, dominance-penalized.
    fn dominant_junk() -> Vec<u8> {
        let mut out = vec![0u8; SCREEN_BYTES];
        let noise = xorshift_bytes(3, SCREEN_BYTES / 64 + 1);
        for (i, chunk) in out.chunks_mut(64).enumerate() {
            chunk[0] = noise[i];
        }
        out
    }

    #[test]
    fn dominant_junk_penalized() {
        let s = score_screen(&dominant_junk(), ScreenRes::Low).unwrap();
        if s.plausible() {
            assert!(s.confidence <= 60, "{}", s.confidence);
        }
    }

    #[test]
    fn short_screen_is_none() {
        let screen = vec![0u8; SCREEN_BYTES - 1];
        assert!(score_screen(&screen, ScreenRes::Low).is_none());
    }
}
