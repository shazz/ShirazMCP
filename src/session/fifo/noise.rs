//! Hatari-side log noise: lines Hatari's main loop emits unconditionally on
//! healthy idle paths, carrying zero information for the agent. Kept as a
//! single documented list — extend here, never inline elsewhere.
//!
//! Survey (P27 §1.1, verified against the REAL stock v2.4.1 tag —
//! `cd /home/matt/projects/hatari && git show v2.4.1:src/control.c`, NOT the
//! tattlemuss/hrdb fork checkout at that path; the tag itself is genuine
//! upstream, confirmed via `git log -1 v2.4.1` = "New release 2.4.1..."):
//! the only recurring main-loop noise is `control.c:411`'s
//! `perror("command FIFO read error")` on the non-blocking fifo's EAGAIN —
//! `Control_CheckUpdates` (`control.c:398`) reads up to 399 bytes
//! (`char buffer[400]`, `control.c:401`) from the fifo every
//! `Main_EventHandler` iteration (`main.c:521`), i.e. constantly while the
//! machine runs with nothing queued. `Control_Usage`'s
//! `"ERROR: unrecognized hatari command: ..."` and every other explicit
//! `"ERROR: ..."` line (`control.c:84,93,105,148,296`) are REAL failures and
//! must never match here.
//!
//! This is not cosmetic: this exact noise line's own text contains the
//! substring "error" case-insensitively, so before this filter existed it
//! could reach `debug::write_memwrite_line`'s case-insensitive `"ERROR"`
//! scan and be mistaken for Hatari rejecting the write — the reported
//! `write_memory` failure on a Running session (MCP_TODOS §6). Both paths
//! flow through the same `debug()` → `strip_protocol_lines` chokepoint
//! (`fifo/mod.rs`), so filtering it there fixes every caller at once.

/// Line prefixes (perror suffixes are locale/errno text — never match the
/// full line). `control.c:411`.
const NOISE_PREFIXES: [&str; 1] = ["command FIFO read error"];

/// True for lines that are pure Hatari idle-path noise (module doc survey).
pub(crate) fn is_hatari_noise(line: &str) -> bool {
    NOISE_PREFIXES.iter().any(|p| line.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_matches_eagain_line_any_suffix() {
        assert!(is_hatari_noise(
            "command FIFO read error: Resource temporarily unavailable"
        ));
        // Locale-dependent perror() suffix (French) — prefix match only.
        assert!(is_hatari_noise(
            "command FIFO read error: Ressource temporairement non disponible"
        ));
    }

    #[test]
    fn noise_requires_line_start() {
        assert!(!is_hatari_noise("note: command FIFO read error: x"));
    }

    #[test]
    fn noise_never_matches_real_errors() {
        assert!(!is_hatari_noise("ERROR: unrecognized event: 'foo'"));
        assert!(!is_hatari_noise(
            "ERROR: unrecognized hatari command: 'bogus'!"
        ));
        assert!(!is_hatari_noise("= %1 (hex)"));
        assert!(!is_hatari_noise(""));
    }
}
