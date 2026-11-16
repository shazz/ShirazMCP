//! Typed wrappers for Hatari's `b` (conditional CPU breakpoint) command.
//! Wire formats pinned live 2026-07-09 (P2.md §3.1–§3.3).

use crate::debug::DebugChannel;
use crate::error::{Error, Result};

/// One entry of the `b` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakpointEntry {
    /// Hatari's 1-based index (shifts on removal — valid only until the
    /// table next changes).
    pub index: u32,
    /// Normalized expression incl. options, e.g. "pc = $d200 :once".
    pub expression: String,
}

/// Result of arming: Hatari's index + its normalized expression (WITHOUT
/// options — the add echo prints the bare condition; the fire report and
/// the list append " :once").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Armed {
    pub index: u32,
    pub expression: String,
}

/// `b {condition} :once` — arm a one-shot breakpoint. All managed
/// breakpoints are :once by design (§8.1): nothing stays armed behind the
/// session's back.
pub async fn arm_once(chan: &mut impl DebugChannel, condition: &str) -> Result<Armed> {
    let out = chan.debug(&format!("b {condition} :once")).await?;
    parse_armed(&out)
}

/// `b` — list conditional CPU breakpoints (empty vec when none).
pub async fn list(chan: &mut impl DebugChannel) -> Result<Vec<BreakpointEntry>> {
    parse_list(&chan.debug("b").await?)
}

/// `b {index}` — remove one breakpoint.
pub async fn remove(chan: &mut impl DebugChannel, index: u32) -> Result<()> {
    let out = chan.debug(&format!("b {index}")).await?;
    let needle = format!("Removed CPU breakpoint {index}:");
    if out.contains(&needle) {
        Ok(())
    } else {
        Err(Error::Parse {
            context: "breakpoint remove".to_string(),
            input: out,
        })
    }
}

/// `b all` — clear the table.
pub async fn clear(chan: &mut impl DebugChannel) -> Result<()> {
    let out = chan.debug("b all").await?;
    // §3.3 `// verify live`: an empty table may print nothing at all — "no
    // ERROR line" is accepted as success in that case.
    if out.contains("CPU breakpoints: 0") || (out.trim().is_empty() && !out.contains("ERROR")) {
        Ok(())
    } else {
        Err(Error::Parse {
            context: "breakpoint clear".to_string(),
            input: out,
        })
    }
}

/// The 4th whitespace-separated token of "CPU condition breakpoint N with
/// …" (`line` already has the fixed prefix stripped).
fn parse_index(rest: &str, line: &str) -> Result<u32> {
    rest.split_whitespace()
        .next()
        .and_then(|t| t.parse::<u32>().ok())
        .ok_or_else(|| Error::Parse {
            context: "breakpoint add index".to_string(),
            input: line.to_string(),
        })
}

pub(crate) fn parse_armed(out: &str) -> Result<Armed> {
    let mut lines = out.lines();
    while let Some(line) = lines.next() {
        if line.contains("ERROR") {
            return Err(Error::Parse {
                context: "breakpoint add".to_string(),
                input: line.to_string(),
            });
        }
        if let Some(rest) = line.strip_prefix("CPU condition breakpoint ") {
            let index = parse_index(rest, line)?;
            let expression = lines.next().map(str::trim).ok_or_else(|| Error::Parse {
                context: "breakpoint add expression".to_string(),
                input: out.to_string(),
            })?;
            return Ok(Armed {
                index,
                expression: expression.to_string(),
            });
        }
    }
    Err(Error::Parse {
        context: "breakpoint add".to_string(),
        input: out.to_string(),
    })
}

pub(crate) fn parse_list(out: &str) -> Result<Vec<BreakpointEntry>> {
    if out.trim() == "No conditional CPU breakpoints." {
        return Ok(vec![]);
    }
    Ok(out
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.ends_with("breakpoints:") || trimmed == "CPU breakpoints: 0" {
                return None;
            }
            let colon = trimmed.find(':')?;
            let index = trimmed[..colon].trim().parse::<u32>().ok()?;
            let expression = trimmed[colon + 1..].trim();
            if expression.is_empty() {
                return None;
            }
            Some(BreakpointEntry {
                index,
                expression: expression.to_string(),
            })
        })
        .collect())
}

/// Every matched expression (with options) in a run's output (§3.2): the
/// TAB-indented line following each "… breakpoint condition(s) matched …"
/// header, in log order. Plural since ADR-47: `:trace` breakpoints print
/// the same header on every hit WITHOUT stopping, so the breakpoint that
/// actually froze the machine is not necessarily the first one reported.
pub(crate) fn fired_expressions(entry_output: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut lines = entry_output.lines();
    while let Some(line) = lines.next() {
        if line.contains("breakpoint condition(s) matched") {
            if let Some(expr) = lines.next() {
                found.push(expr.trim().to_string());
            }
        }
    }
    found
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

    const ARM_BLOCK: &str = "CPU condition breakpoint 1 with 1 condition(s) added:\n\tpc > 0\n-> Break only once, and delete breakpoint afterwards.";

    const FIRE_BLOCK: &str = "1. CPU breakpoint condition(s) matched 1 times.\n\tpc > 0 :once\nRemoved CPU breakpoint 1:\n\tpc > 0 :once\n\n----------------------------------------------------------------------\nYou have entered debug mode. Type c to continue emulation, h for help.\nCPU=$d106, VBL=35936, FrameCycles=8680, HBL=16, LineCycles=488, DSP=N/A\n0000d106 2742 eec4                move.l d2,(a3,-$113c) == $00041b04 [00000000]";

    #[tokio::test]
    async fn arm_once_parses_index_and_expression() {
        let mut fake = Fake {
            responses: vec![ARM_BLOCK.to_string()],
            sent: vec![],
        };
        let armed = arm_once(&mut fake, "pc > 0").await.unwrap();
        assert_eq!(
            armed,
            Armed {
                index: 1,
                expression: "pc > 0".to_string()
            }
        );
        assert_eq!(fake.sent, vec!["b pc > 0 :once".to_string()]);
    }

    #[tokio::test]
    async fn arm_once_error_line_fails() {
        let mut fake = Fake {
            responses: vec!["ERROR: invalid breakpoint condition".to_string()],
            sent: vec![],
        };
        let err = arm_once(&mut fake, "garbage").await.unwrap_err();
        assert!(matches!(err, Error::Parse { .. }));
    }

    #[tokio::test]
    async fn list_empty() {
        let mut fake = Fake {
            responses: vec!["No conditional CPU breakpoints.".to_string()],
            sent: vec![],
        };
        assert_eq!(list(&mut fake).await.unwrap(), vec![]);
    }

    #[tokio::test]
    async fn list_one() {
        let block = "1 conditional CPU breakpoints:\n   1:\tpc = $d200 :once";
        let mut fake = Fake {
            responses: vec![block.to_string()],
            sent: vec![],
        };
        assert_eq!(
            list(&mut fake).await.unwrap(),
            vec![BreakpointEntry {
                index: 1,
                expression: "pc = $d200 :once".to_string()
            }]
        );
    }

    #[tokio::test]
    async fn remove_ok() {
        let mut fake = Fake {
            responses: vec!["Removed CPU breakpoint 1:\n\tpc = $d200 :once".to_string()],
            sent: vec![],
        };
        remove(&mut fake, 1).await.unwrap();
        assert_eq!(fake.sent, vec!["b 1".to_string()]);
    }

    #[tokio::test]
    async fn remove_missing_fails() {
        let mut fake = Fake {
            responses: vec!["ERROR: breakpoint 9 not found".to_string()],
            sent: vec![],
        };
        let err = remove(&mut fake, 9).await.unwrap_err();
        assert!(matches!(err, Error::Parse { .. }));
    }

    #[tokio::test]
    async fn clear_ok() {
        let block = "Removed CPU breakpoint 1:\n\tpc = $d200 :once\nCPU breakpoints: 0";
        let mut fake = Fake {
            responses: vec![block.to_string()],
            sent: vec![],
        };
        clear(&mut fake).await.unwrap();
        assert_eq!(fake.sent, vec!["b all".to_string()]);
    }

    #[test]
    fn fired_expressions_extracts() {
        assert_eq!(fired_expressions(FIRE_BLOCK), vec!["pc > 0 :once"]);
    }

    #[test]
    fn fired_expressions_empty_when_absent() {
        assert!(fired_expressions("nothing interesting here").is_empty());
    }

    /// ADR-47: trace hits precede the stopping `:once` report — the
    /// managed run must still recognise its own breakpoint.
    #[test]
    fn fired_expressions_lists_trace_hits_then_stop() {
        let out = format!(
            "2. CPU breakpoint condition(s) matched 1 times.\n\tpc = $1234 :trace\n{FIRE_BLOCK}"
        );
        assert_eq!(
            fired_expressions(&out),
            vec!["pc = $1234 :trace", "pc > 0 :once"]
        );
    }
}
