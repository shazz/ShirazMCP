//! Sentinel-synced command send/await over whichever control plane is live
//! (ADR-11 §2.1: the fifo when Running, Hatari's stdin when Paused). Split
//! into `mod.rs` (protocol) + `log_tail.rs` (log polling) — §3 file-size
//! cleanup, no behavior change beyond §2.5's stray-sentinel filter.

pub mod log_tail;
mod noise;
mod sentinel;

use std::time::Duration;

use tokio::io::{AsyncWrite, AsyncWriteExt};

pub use log_tail::LogTail;

use crate::error::{Error, Result};

/// Prefix that turns a raw debugger CLI line into a fifo wrapper command.
/// Stdin (`ControlChannel::DebugStdin`) uses no prefix — DebugUI reads raw
/// CLI lines directly once the CPU is parked (ADR-11 §2.1).
pub(crate) const HATARI_DEBUG_PREFIX: &str = "hatari-debug ";

/// Per-attempt budget for one sentinel-write + poll cycle inside `debug()`.
/// Some Hatari debugger commands (observed: `m`) consume the very next fifo
/// line if it lands in the same read() batch as their own — confirmed
/// against a live Hatari 2.4.1 session (the canonical fixture never chains
/// an `m` command with a sentinel, so this never showed up there). Instead
/// of one fixed "wait long enough" sleep, the sentinel is re-written on
/// each attempt, each spaced by a fresh bounded `read_until` poll: this
/// completes as soon as ANY attempt lands, still bounded by `timeout_ms`,
/// and adds no new blind-sleep call site (reuses `read_until`'s poll).
const SENTINEL_RETRY_BUDGET: Duration = Duration::from_millis(300);

/// The `debug(cmd)` primitive (ADR-7), generic over the control plane
/// (ADR-11): `W` is `tokio::fs::File` for the fifo or `tokio::process::
/// ChildStdin` while frozen. Writes `{prefix}{cmd}` then the sentinel
/// (retried per `SENTINEL_RETRY_BUDGET`, same prefix), tails the log until
/// the sentinel's result line appears, and returns the intervening output
/// with protocol lines (echoes, blanks, stray sentinel echoes) filtered.
pub async fn debug<W: AsyncWrite + Unpin + Send>(
    writer: &mut W,
    log: &mut LogTail,
    prefix: &str,
    cmd: &str,
    timeout_ms: u64,
    what: &str,
) -> Result<String> {
    write_line(writer, &format!("{prefix}{cmd}")).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let lines = read_sentinel_with_retry(writer, log, prefix, what, deadline, timeout_ms).await?;
    Ok(strip_protocol_lines(lines))
}

/// Drop wrapper-command echoes (`"> ..."`), blank lines, any line that
/// itself looks like ANY sentinel's result (§2.5 — a queued/replayed
/// sentinel, a retry, or a fifo write that queued while frozen and replayed
/// on resume, must never be mistaken for a later command's real output;
/// `parse_evaluate` takes the first `"= %"` line, so an unfiltered stray
/// sentinel echo could be returned as if it were a real value; matches the
/// whole sentinel FAMILY, not just the current call's value, so stale
/// echoes from earlier calls are caught too), and — P27/ADR-43 — Hatari's
/// own idle-path log noise (`noise::is_hatari_noise`: the EAGAIN
/// `"command FIFO read error"` line `Control_CheckUpdates` perrors on every
/// poll of an idle fifo). The noise clause runs before ADR-28's
/// `cap_output_tail`, so noise can never evict real output from a capped
/// tail window.
fn strip_protocol_lines(lines: Vec<String>) -> String {
    lines
        .into_iter()
        .filter(|l| {
            !l.starts_with("> ")
                && !l.trim().is_empty()
                && !sentinel::is_sentinel_family_line(l)
                && !noise::is_hatari_noise(l)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Each call gets ITS OWN sentinel value (ADR-11 addendum, `sentinel.rs`):
/// retries within this call reuse it (so whichever attempt lands still
/// completes it), but no OTHER call's echo — stale or not — can terminate
/// this one early.
async fn read_sentinel_with_retry<W: AsyncWrite + Unpin + Send>(
    writer: &mut W,
    log: &mut LogTail,
    prefix: &str,
    what: &str,
    deadline: tokio::time::Instant,
    total_timeout_ms: u64,
) -> Result<Vec<String>> {
    let value = sentinel::next_value();
    let mut collected = Vec::new();
    loop {
        write_line(writer, &format!("{prefix}{}", sentinel::expr(value))).await?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(Error::Timeout {
                what: what.to_string(),
                waited_ms: total_timeout_ms,
            });
        }
        let attempt_ms = remaining.min(SENTINEL_RETRY_BUDGET).as_millis() as u64;
        let is_marker = |l: &str| sentinel::is_this_sentinel_line(l, value);
        match log
            .read_until(is_marker, what, attempt_ms, &mut collected)
            .await
        {
            Ok(()) => return Ok(collected),
            Err(Error::Timeout { .. }) => continue,
            Err(e) => return Err(e),
        }
    }
}

/// ADR-11 §run-to-break: write `c` (thaw) then ONE queued sentinel probe.
/// While the machine runs, the probe sits unread in Hatari's stdin pipe;
/// the next debugger entry (a firing breakpoint) consumes and echoes it.
/// The sentinel echo in the log therefore means "a breakpoint fired and
/// the CPU is parked again". Returns the intervening log lines (breakpoint
/// match report, auto profiler stats, …), protocol-stripped.
///
/// Deliberately NOT `read_sentinel_with_retry`: re-writing the probe would
/// queue N copies that all replay at entry. One probe (its own fresh
/// sentinel value, ADR-11 addendum), one full-budget `read_until`. On
/// `Err(Timeout)` the machine is still RUNNING and one stale probe stays
/// queued — harmless: its value is unique to this call, so no LATER call
/// can ever mistake it for its own.
pub async fn continue_and_wait<W: AsyncWrite + Unpin + Send>(
    stdin: &mut W,
    log: &mut LogTail,
    timeout_ms: u64,
    what: &str,
) -> Result<String> {
    let value = sentinel::next_value();
    write_line(stdin, "c").await?;
    write_line(stdin, &sentinel::expr(value)).await?;
    let mut collected = Vec::new();
    let is_marker = |l: &str| sentinel::is_this_sentinel_line(l, value);
    log.read_until(is_marker, what, timeout_ms, &mut collected)
        .await?;
    Ok(strip_protocol_lines(collected))
}

/// Write one raw wrapper line ("hatari-shortcut <x>", ADR-11's `b pc > 0
/// :once` arm, or `"c"` to thaw) — no sentinel round-trip (these commands
/// produce no reliable log echo of their own; callers confirm liveness
/// separately).
pub async fn write_wrapper<W: AsyncWrite + Unpin + Send>(writer: &mut W, line: &str) -> Result<()> {
    write_line(writer, line).await
}

/// Writes `line` and its trailing newline in ONE `write_all` call — Hatari
/// treats each `read()` on the fifo/stdin as a complete, self-contained
/// command batch (`Control_CheckUpdates`'s "assume whole command can be
/// read in one go"). Two separate `write_all` calls (content, then `"\n"`)
/// leave a real gap: `tokio::fs::File`/`ChildStdin` writes go through
/// `spawn_blocking`, so the task actually suspends between the two calls,
/// and Hatari's own poll can fire in that window — it then reads the
/// command text with NO trailing newline (still parses fine, since
/// `Control_ProcessBuffer` splits on the first space, not the newline),
/// and the newline arrives alone on the NEXT poll as an empty command,
/// which Hatari logs as `"unrecognized hatari command: ''"`. Confirmed
/// live: this raced roughly every other `pause()`/`resume()` cycle before
/// this fix.
async fn write_line<W: AsyncWrite + Unpin + Send>(writer: &mut W, line: &str) -> Result<()> {
    let fail = |e: std::io::Error| Error::Fifo {
        context: line.to_string(),
        source: e,
    };
    writer
        .write_all(format!("{line}\n").as_bytes())
        .await
        .map_err(fail)?;
    writer.flush().await.map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sentinel VALUE matching (per-call uniqueness, family stripping) is
    // covered by `sentinel::tests` — this module only tests the protocol
    // plumbing built on top of it.

    #[test]
    fn strip_protocol_lines_drops_echoes_and_blanks() {
        let lines = vec![
            "> evaluate pc".to_string(),
            "= %1 (hex)".to_string(),
            String::new(),
        ];
        assert_eq!(strip_protocol_lines(lines), "= %1 (hex)");
    }

    /// §2.5: a stray sentinel result line mixed into the collected output
    /// (e.g. a queued fifo write replayed after a freeze/thaw cycle) must
    /// be dropped, not returned as if it were the command's real output.
    #[test]
    fn strip_protocol_lines_drops_stray_sentinel_echo() {
        let stale_sentinel =
            "= %11011110101011011011111011101111 (bin), #3735928559 (dec), $deadbeef (hex)"
                .to_string();
        let lines = vec![
            "hello".to_string(),
            stale_sentinel,
            "= %220 (dec), $dc (hex)".to_string(),
        ];
        assert_eq!(
            strip_protocol_lines(lines),
            "hello\n= %220 (dec), $dc (hex)"
        );
    }

    /// P27 §2.4: a mixed batch of one real line and Hatari's idle-path EAGAIN
    /// noise must leave only the real line — every `debug()`/
    /// `continue_and_wait()` caller inherits this from the one clause above.
    #[test]
    fn strip_protocol_lines_drops_hatari_noise() {
        let lines = vec![
            "command FIFO read error: Resource temporarily unavailable".to_string(),
            "Wrote 0x100000 bytes.".to_string(),
            "command FIFO read error: Resource temporarily unavailable".to_string(),
        ];
        assert_eq!(strip_protocol_lines(lines), "Wrote 0x100000 bytes.");
    }

    /// MCP_TODOS §6 regression pin: the noise line's own text contains
    /// "error" case-insensitively, so before this filter existed it could
    /// reach `debug::write_memwrite_line`'s case-insensitive `"ERROR"` scan
    /// and be mistaken for a real Hatari write rejection — `write_memory`
    /// failing on a Running session with
    /// `parse error (memwrite): unexpected input "command FIFO read error…"`.
    /// A memwrite response consisting purely of noise (the EAGAIN line polls
    /// straight through Running-state command output) must now strip to
    /// nothing, so the caller's error scan never sees it.
    #[test]
    fn strip_protocol_lines_drops_noise_before_memwrite_error_scan() {
        let lines = vec![
            "command FIFO read error: Resource temporarily unavailable".to_string(),
            "command FIFO read error: Resource temporarily unavailable".to_string(),
        ];
        assert_eq!(strip_protocol_lines(lines), "");
    }
}
