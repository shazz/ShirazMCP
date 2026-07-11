//! Per-call-unique sentinel values (ADR-11 addendum). Split out of `mod.rs`
//! for the line ceiling once the fixed `$DEADBEEF` sentinel grew a
//! uniqueness scheme.
//!
//! The original design used ONE fixed value (`$DEADBEEF`) for every
//! sentinel round-trip. That is unsound under real contention: `debug()`
//! retries its sentinel write every `SENTINEL_RETRY_BUDGET` if Hatari is
//! slow to answer, so a single logical call can leave MULTIPLE `$deadbeef`
//! echoes queued; the NEXT call's `read_until` can then terminate on one of
//! those stale echoes instead of its own, returning before Hatari actually
//! processed the real command (confirmed live: under CPU contention this
//! reliably corrupted the first command after `pause()`, truncating
//! `savebin`/`memory::acquire` reads — the freeze-drain fix alone,
//! `HatariProcess::drain_stdin_entry_text`, does not close this, since a
//! stale echo can satisfy the drain round too). Each call now gets a fresh
//! value instead: `is_this_sentinel_line` can only match the CURRENT call's
//! own echo, so a stale echo from an earlier call's retry can never
//! terminate it early.

use std::sync::atomic::{AtomicU32, Ordering};

/// Fixed, recognizable high half of every sentinel value — lets
/// `is_sentinel_family_line` recognize (and strip) ANY sentinel echo,
/// regardless of which call produced it, without needing to know the
/// specific value in play.
const SENTINEL_BASE: u32 = 0xDEAD_0000;

/// Lowercase-hex textual form of `SENTINEL_BASE`'s fixed high half — every
/// sentinel echo's hex field starts with this, whatever the low 16 bits.
const SENTINEL_FAMILY_PREFIX: &str = "$dead";

/// Process-wide monotonic counter. A single `AtomicU32` shared by every
/// `HatariProcess` in this OS process is fine — the counter exists purely
/// as a uniqueness discriminant, not shared session state; wrapping after
/// 65536 calls without a stale echo from 65536 calls ago still being
/// unread in the log is not a realistic collision.
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh sentinel value for one logical call (all retries WITHIN that
/// call reuse it — only different calls must differ).
pub(super) fn next_value() -> u32 {
    SENTINEL_BASE | (COUNTER.fetch_add(1, Ordering::Relaxed) & 0xFFFF)
}

/// The bare expression, WITHOUT any wrapper prefix (`debug`/
/// `read_sentinel_with_retry` supply the prefix per call).
pub(super) fn expr(value: u32) -> String {
    format!("evaluate ${value:08x}")
}

/// What Hatari's RESULT line contains when THIS value's expression
/// evaluates (ADR-7): keyed on the hex field only — the decimal field is
/// NOT simply `#value (dec)` for values with the sign bit set (Hatari
/// prints the dual signed/unsigned form `#.../-... (dec)`; confirmed live).
/// The hex field is stable either way and, combined with the `"= %"`
/// prefix, is already unique to this call.
fn echo(value: u32) -> String {
    format!("${value:08x} (hex)")
}

/// A log line completes THIS call's sentinel iff it starts with `"= %"`
/// (after trimming) AND contains this call's specific echo.
pub(super) fn is_this_sentinel_line(line: &str, value: u32) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("= %") && trimmed.contains(&echo(value))
}

/// A log line looks like SOME sentinel's result, regardless of which call's
/// value it carries — used to strip stray echoes (a retry, or a queued
/// write replayed across a freeze/thaw cycle) out of returned command
/// output, matching on the shared `SENTINEL_BASE` prefix only.
pub(super) fn is_sentinel_family_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("= %") && trimmed.contains(SENTINEL_FAMILY_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_value_is_unique_across_calls() {
        let a = next_value();
        let b = next_value();
        assert_ne!(a, b, "consecutive calls must not reuse a sentinel value");
        assert_eq!(a & 0xFFFF_0000, SENTINEL_BASE);
        assert_eq!(b & 0xFFFF_0000, SENTINEL_BASE);
    }

    #[test]
    fn this_sentinel_line_matches_only_its_own_value() {
        let line = "= %1101 (bin), #3735943681 (dec), $dead0001 (hex)";
        assert!(is_this_sentinel_line(line, 0xDEAD_0001));
        assert!(!is_this_sentinel_line(line, 0xDEAD_0002));
    }

    #[test]
    fn this_sentinel_line_matches_signed_unsigned_dual_form() {
        // Real Hatari output for a value with the sign bit set (mirrors the
        // original $DEADBEEF fixture — confirmed live, P1.md's canonical
        // fixture only ever exercised a small positive `evaluate pc`).
        let line = "= %11011110101011011011111011101111 (bin), \
                     #3735928559/-559038737 (dec), $deadbeef (hex)";
        assert!(is_this_sentinel_line(line, 0xDEAD_BEEF));
    }

    #[test]
    fn this_sentinel_line_ignores_command_echo() {
        assert!(!is_this_sentinel_line("> evaluate $DEAD0001", 0xDEAD_0001));
        assert!(!is_this_sentinel_line(
            "> hatari-debug evaluate $DEAD0001",
            0xDEAD_0001
        ));
    }

    #[test]
    fn family_line_matches_any_sentinel_value() {
        assert!(is_sentinel_family_line(
            "= %1 (bin), #1 (dec), $dead0001 (hex)"
        ));
        assert!(is_sentinel_family_line(
            "= %1 (bin), #1 (dec), $deadbeef (hex)"
        ));
    }

    #[test]
    fn family_line_ignores_unrelated_result() {
        assert!(!is_sentinel_family_line("= %1 (bin), #1 (dec), $1 (hex)"));
        assert!(!is_sentinel_family_line("> evaluate $DEAD0001"));
    }
}
