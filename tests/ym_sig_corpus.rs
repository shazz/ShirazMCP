//! SNDH-archive regression for the ym_sig_db fingerprints (P24, ADR-41).
//! Needs the corpus (fetch recipe: refs/README.md). Run with:
//!   SHIRAZMCP_SNDH_CORPUS=refs/sndh_lf \
//!     cargo test --release --test ym_sig_corpus -- --ignored
//! Counts are pinned to the sndh2026_lf bundle (5,897 files); re-pin the
//! histogram when the yearly archive is refreshed.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape is the correct failure mode.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration test, not library code"
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use shiraz_mcp::ripper::plugins::ym_signatures::identify;

/// `SHIRAZMCP_SNDH_CORPUS` else `<repo>/refs/sndh_lf` (P24.md §7).
fn corpus_root() -> PathBuf {
    std::env::var("SHIRAZMCP_SNDH_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(format!("{}/refs/sndh_lf", env!("CARGO_MANIFEST_DIR"))))
}

/// Panics with the fetch recipe if the corpus directory is missing —
/// correct failure mode for a `#[ignore]`d, locally-gated test.
fn require_corpus() -> PathBuf {
    let root = corpus_root();
    if !root.is_dir() {
        panic!(
            "SNDH corpus not found at {} — fetch recipe: refs/README.md \
             (sndh_lf section); set SHIRAZMCP_SNDH_CORPUS to override",
            root.display()
        );
    }
    root
}

/// Recursive walk collecting every `*.sndh` (case-insensitive), sorted by
/// path for deterministic iteration order.
fn collect_sndh_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("sndh"))
        {
            out.push(path);
        }
    }
}

/// Best-match player name for a whole file (byte-based sweep, §7).
fn classify(bytes: &[u8]) -> &'static str {
    identify(bytes, 0, bytes.len()).player
}

/// (file relative to corpus root, offset, pattern byte length, player,
/// confidence) — the 11 new P24 entries + the rg-triplex re-verification.
const PROVENANCE: &[(&str, usize, usize, &str, f32)] = &[
    ("607/Good_Old_Times.sndh", 0xd80, 24, "Quartet", 0.85),
    (
        "Whittaker_David/APB.sndh",
        0x528,
        24,
        "David Whittaker",
        0.85,
    ),
    ("Count_Zero/Awesome.sndh", 0x322, 28, "TCB Tracker", 0.85),
    (
        "Mad_Max/Car_Race.sndh",
        0x1e8,
        24,
        "Jochen Hippel (Mad Max)",
        0.85,
    ),
    (
        "Mad_Max/Demos/Best_In_Galaxy/Ace_2.sndh",
        0xdc2,
        22,
        "Jochen Hippel (Mad Max)",
        0.85,
    ),
    (
        "Follin_Tim/Bionic_Commando.sndh",
        0xd1a,
        28,
        "Tim Follin",
        0.85,
    ),
    (
        "Whittaker_David/APB.sndh",
        0x4f2,
        16,
        "David Whittaker",
        0.82,
    ),
    (
        "Jess/A_Sixties_Slow.sndh",
        0x1a0,
        18,
        "Jess (Overlanders)",
        0.82,
    ),
    ("Tao/13_(UCM_13).sndh", 0x396, 16, "Tao (tSCc)", 0.82),
    ("Big_Alec/A_Case_For_Two.sndh", 0x34a, 20, "Big Alec", 0.82),
    (
        "Vidovic_Augustin/Music_Mon_Intro.sndh",
        0x5dc,
        14,
        "Music Mon",
        0.82,
    ),
    (
        "Damo/YM_Rucka.sndh",
        0xb64,
        18,
        "Reservoir Gods driver",
        0.70,
    ),
];

/// Pinned best-match histogram over the full sndh2026_lf archive (5,897
/// files); everything not listed is "unknown". Total named: 1,796.
const HISTOGRAM: &[(&str, usize)] = &[
    ("maxYMiser", 831),
    ("Jochen Hippel (Mad Max)", 352),
    ("Quartet", 293),
    ("Tao (tSCc)", 132),
    ("David Whittaker", 61),
    ("Jess (Overlanders)", 54),
    ("Music Mon", 26),
    ("TCB Tracker", 22),
    ("Big Alec", 18),
    ("Tim Follin", 6),
    ("Reservoir Gods driver", 1),
];

/// Per-driver-dir exclusivity: every file's best-match player in a
/// single-driver composer dir must be in the allowed set (§7.3). Quartet
/// is allowed for Count_Zero: Decade_Demo_Quartet.sndh is a Grazey
/// Quartet-2-SNDH conversion (§5.2 quartet-mute cross-hit, same class as
/// the documented Big_Alec/Whittaker_David cross-hits below).
const DRIVER_DIRS: &[(&str, &[&str])] = &[
    (
        "Count_Zero",
        &["TCB Tracker", "Quartet", "maxYMiser", "unknown"],
    ),
    ("Jess", &["Jess (Overlanders)", "maxYMiser", "unknown"]),
    ("Vidovic_Augustin", &["Music Mon", "maxYMiser", "unknown"]),
    ("Follin_Tim", &["Tim Follin", "maxYMiser", "unknown"]),
    (
        "Big_Alec",
        &[
            "Big Alec",
            "Jochen Hippel (Mad Max)",
            "maxYMiser",
            "unknown",
        ],
    ),
    (
        "Whittaker_David",
        &["David Whittaker", "Quartet", "maxYMiser", "unknown"],
    ),
];

#[test]
#[ignore = "needs the SNDH archive; set SHIRAZMCP_SNDH_CORPUS"]
fn corpus_provenance_pins() {
    let root = require_corpus();
    for &(rel, offset, len, player, conf) in PROVENANCE {
        let path = root.join(rel);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        assert!(
            offset + len <= bytes.len(),
            "{rel}@{offset:#x} + {len} exceeds file length {}",
            bytes.len()
        );
        let excerpt = &bytes[offset..offset + len];
        let id = identify(excerpt, 0, excerpt.len());
        assert_eq!(id.player, player, "{rel}@{offset:#x}");
        assert!(
            (id.confidence - conf).abs() < 1e-6,
            "{rel}@{offset:#x}: confidence {} != {conf}",
            id.confidence
        );
    }
}

#[test]
#[ignore = "needs the SNDH archive; set SHIRAZMCP_SNDH_CORPUS"]
fn corpus_best_match_histogram() {
    let root = require_corpus();
    let files = collect_sndh_files(&root);

    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    for path in &files {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        *counts.entry(classify(&bytes)).or_insert(0) += 1;
    }

    let named: usize = HISTOGRAM.iter().map(|&(_, n)| n).sum();
    let mut mismatches = Vec::new();
    for &(player, expected) in HISTOGRAM {
        let actual = counts.get(player).copied().unwrap_or(0);
        if actual != expected {
            mismatches.push(format!("{player}: expected {expected}, got {actual}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "histogram mismatch (total files {}, expected named {named}):\n{}",
        files.len(),
        mismatches.join("\n")
    );
    assert_eq!(files.len(), 5897, "archive file count drifted");
}

#[test]
#[ignore = "needs the SNDH archive; set SHIRAZMCP_SNDH_CORPUS"]
fn corpus_driver_dirs_exclusive() {
    let root = require_corpus();
    let mut violations = Vec::new();

    for &(dir, allowed) in DRIVER_DIRS {
        let files = collect_sndh_files(&root.join(dir));
        for path in files {
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
            let player = classify(&bytes);
            if !allowed.contains(&player) {
                violations.push(format!("{} -> {player}", path.display()));
            }
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}
