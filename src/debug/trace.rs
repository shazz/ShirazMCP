//! `:trace` breakpoints (print a match report per hit, never stop) and the
//! hit-count parser behind `count_hits` (ADR-47). Wire format pinned from
//! Hatari's `breakcond.c` (`BreakCond_MatchBreakPoints` / `BreakCond_Print`,
//! identical in 2.4.1 and 2.6.1): every hit writes
//! `"{index}. CPU breakpoint condition(s) matched {hits} times."` followed
//! by a TAB-indented `"\t{expression} :trace"` line, where `hits` is the
//! breakpoint's CUMULATIVE counter since it was created.

use crate::debug::breakpoint::{parse_armed, Armed};
use crate::debug::DebugChannel;
use crate::error::Result;

/// The option suffix Hatari appends to a trace breakpoint's expression in
/// match reports and in the `b` listing.
pub const TRACE_SUFFIX: &str = " :trace";

/// `b {condition} :trace` — arm a non-stopping trace breakpoint. The
/// returned `expression` is Hatari's normalized condition (no options).
pub async fn arm_trace(chan: &mut impl DebugChannel, condition: &str) -> Result<Armed> {
    let out = chan.debug(&format!("b {condition}{TRACE_SUFFIX}")).await?;
    parse_armed(&out)
}

/// Hits of OUR trace breakpoint — table `index` (from `arm_trace`) AND
/// printed `expression` (WITHOUT the ` :trace` suffix) — read from
/// Hatari's own cumulative "matched N times" counter: the MAX counter seen
/// for that breakpoint. Matching the index too keeps an identical
/// pre-existing user breakpoint (lower index, older counter) from being
/// mistaken for ours; the index is stable for the whole window because
/// nothing below our entries is removed until the window has closed.
/// Robust to lost or duplicated lines and never fooled by definition
/// echoes (`"CPU condition breakpoint 1 with …"`) or listings
/// (`"   1:\tpc = $… :trace"`) — those carry no "matched" header. 0 when
/// the breakpoint never fired.
pub fn trace_hits(output: &str, index: u32, expression: &str) -> u64 {
    let wanted = format!("{expression}{TRACE_SUFFIX}");
    let mut lines = output.lines();
    let mut best = 0;
    while let Some(line) = lines.next() {
        let Some((idx, hits)) = matched_header(line) else {
            continue;
        };
        if idx == index && lines.next().map(str::trim) == Some(wanted.as_str()) {
            best = best.max(hits);
        }
    }
    best
}

/// `"3. CPU breakpoint condition(s) matched 17 times."` → (3, 17).
fn matched_header(line: &str) -> Option<(u32, u64)> {
    let (head, rest) = line
        .trim()
        .split_once(" breakpoint condition(s) matched ")?;
    let index = head.split_once('.')?.0.trim().parse().ok()?;
    let hits = rest.strip_suffix(" times.")?.trim().parse().ok()?;
    Some((index, hits))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        responses: Vec<String>,
        sent: Vec<String>,
    }

    impl DebugChannel for Fake {
        async fn debug(&mut self, cmd: &str) -> Result<String> {
            self.sent.push(cmd.to_string());
            Ok(self.responses.remove(0))
        }
    }

    /// A realistic `count_hits` window: two trace breakpoints, the arm
    /// echoes and a listing (which a naive `grep -c '$e00800'` counts),
    /// several hits, then the managed VBL `:once` breakpoint firing.
    const RUN_LOG: &str = "\
CPU condition breakpoint 2 with 1 condition(s) added:
\tpc = $e00800
-> Trace (just show breakpoint info, instead of dropping to debugger).
CPU condition breakpoint 3 with 1 condition(s) added:
\tpc = $1234
-> Trace (just show breakpoint info, instead of dropping to debugger).
3 conditional CPU breakpoints:
   1:\tpc = $e00800 :once
   2:\tpc = $e00800 :trace
   3:\tpc = $1234 :trace
2. CPU breakpoint condition(s) matched 1 times.
\tpc = $e00800 :trace
2. CPU breakpoint condition(s) matched 2 times.
\tpc = $e00800 :trace
3. CPU breakpoint condition(s) matched 1 times.
\tpc = $1234 :trace
2. CPU breakpoint condition(s) matched 3 times.
\tpc = $e00800 :trace
4. CPU breakpoint condition(s) matched 1 times.
\tVBL > 101 :once
Removed CPU breakpoint 4:
\tVBL > 101 :once";

    #[test]
    fn trace_hits_uses_cumulative_counter_not_line_count() {
        assert_eq!(trace_hits(RUN_LOG, 2, "pc = $e00800"), 3);
        assert_eq!(trace_hits(RUN_LOG, 3, "pc = $1234"), 1);
        // A naive substring count over-counts (echo + listing + hits).
        assert!(RUN_LOG.matches("pc = $e00800").count() > 3);
    }

    #[test]
    fn trace_hits_zero_when_never_fired_or_not_trace() {
        assert_eq!(trace_hits(RUN_LOG, 5, "pc = $5678"), 0);
        // The :once breakpoint's report is not a trace hit.
        assert_eq!(trace_hits(RUN_LOG, 4, "VBL > 101"), 0);
        assert_eq!(trace_hits("", 1, "pc = $1234"), 0);
    }

    #[test]
    fn trace_hits_survives_lost_lines() {
        // Only the last report survived (e.g. a truncated log): the
        // cumulative counter still gives the exact total.
        let log = "7. CPU breakpoint condition(s) matched 4096 times.\n\tpc = $1234 :trace";
        assert_eq!(trace_hits(log, 7, "pc = $1234"), 4096);
    }

    /// A pre-existing identical user trace breakpoint (index 1, with an
    /// older and higher counter) must not leak into ours (index 3).
    #[test]
    fn trace_hits_ignores_identical_user_breakpoint() {
        let log = "1. CPU breakpoint condition(s) matched 900 times.\n\tpc = $1234 :trace\n\
3. CPU breakpoint condition(s) matched 1 times.\n\tpc = $1234 :trace\n\
1. CPU breakpoint condition(s) matched 901 times.\n\tpc = $1234 :trace\n\
3. CPU breakpoint condition(s) matched 2 times.\n\tpc = $1234 :trace";
        assert_eq!(trace_hits(log, 3, "pc = $1234"), 2);
        assert_eq!(trace_hits(log, 1, "pc = $1234"), 901);
    }

    #[test]
    fn matched_header_parses_header_only() {
        assert_eq!(
            matched_header("12. CPU breakpoint condition(s) matched 17 times."),
            Some((12, 17))
        );
        assert_eq!(matched_header("   1:\tpc = $1234 :trace"), None);
        assert_eq!(matched_header("matched lots times."), None);
    }

    #[tokio::test]
    async fn arm_trace_sends_trace_option() {
        let echo = "CPU condition breakpoint 2 with 1 condition(s) added:\n\tpc = $1234\n\
-> Trace (just show breakpoint info, instead of dropping to debugger).";
        let mut fake = Fake {
            responses: vec![echo.to_string()],
            sent: vec![],
        };
        let armed = arm_trace(&mut fake, "pc = $1234").await.unwrap();
        assert_eq!(armed.index, 2);
        assert_eq!(armed.expression, "pc = $1234");
        assert_eq!(fake.sent, vec!["b pc = $1234 :trace".to_string()]);
    }
}
