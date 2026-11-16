//! Drive Hatari's CPU profiler (`profile on/save/off`) and parse the
//! per-address save file. File format pinned live (P2.md §3.4/§3.5) and
//! fixture-committed (tests/fixtures/hatari_profile.txt).

use std::path::Path;

use crate::debug::DebugChannel;
use crate::error::{Error, Result};
use crate::types::Addr;

/// One profiled address from `profile save`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfiledAddr {
    pub addr: Addr,
    /// Executed-instruction count at this address.
    pub count: u64,
    /// Measured CPU cycles spent at this address (Hatari core, includes
    /// ST bus rounding/contention as emulated).
    pub cycles: u64,
}

pub async fn profile_on(chan: &mut impl DebugChannel) -> Result<()> {
    let out = chan.debug("profile on").await?;
    confirm(out, "Profiling enabled.", "profile on")
}

pub async fn profile_off(chan: &mut impl DebugChannel) -> Result<()> {
    let out = chan.debug("profile off").await?;
    confirm(out, "Profiling disabled.", "profile off")
}

pub async fn profile_save(chan: &mut impl DebugChannel, path: &Path) -> Result<()> {
    let out = chan
        .debug(&format!("profile save {}", path.display()))
        .await?;
    if out.contains("CPU addresses.") {
        console_fallback(path, &out)?;
        Ok(())
    } else {
        Err(Error::Parse {
            context: "profile save".to_string(),
            input: out,
        })
    }
}

/// Hatari 2.6 quirk (seen repeatedly on the GP500 project, local patch): the
/// `profile save` FILE can contain only headers and `[...]` placeholders while
/// the per-address dump is echoed on the debugger console. If the file has no
/// data lines but the console output does, persist the console copy so every
/// caller (profile_frame, profile) parses real data instead of zeros.
fn console_fallback(path: &Path, out: &str) -> Result<()> {
    let file_has_data = std::fs::read_to_string(path)
        .map(|c| !parse_profile_file(&c).is_empty())
        .unwrap_or(false);
    if !file_has_data && !parse_profile_file(out).is_empty() {
        std::fs::write(path, out).map_err(Error::Io)?;
    }
    Ok(())
}

fn confirm(out: String, needle: &str, context: &str) -> Result<()> {
    if out.contains(needle) {
        Ok(())
    } else {
        Err(Error::Parse {
            context: context.to_string(),
            input: out,
        })
    }
}

/// Parse the save file (§3.5). Data line test: first whitespace token is
/// 1..=8 ASCII lowercase-hex chars AND the line contains "% (" AND ends
/// with ')'. Fields: substring between the LAST '(' and the trailing ')',
/// split on ',', trimmed → [count, cycles, i_misses, d_hits]; parse the
/// first two as u64. Lines failing the test are skipped silently (headers,
/// "[...]", caller-graph comments). Returns entries in file order.
pub fn parse_profile_file(contents: &str) -> Vec<ProfiledAddr> {
    contents.lines().filter_map(parse_data_line).collect()
}

fn parse_data_line(line: &str) -> Option<ProfiledAddr> {
    let trimmed = line.trim_end();
    if !trimmed.contains("% (") || !trimmed.ends_with(')') {
        return None;
    }
    let addr_tok = trimmed.split_whitespace().next()?;
    if addr_tok.is_empty()
        || addr_tok.len() > 8
        || !addr_tok
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return None;
    }
    let open = trimmed.rfind('(')?;
    let fields = &trimmed[open + 1..trimmed.len() - 1];
    let mut parts = fields.split(',').map(str::trim);
    let count = parts.next()?.parse::<u64>().ok()?;
    let cycles = parts.next()?.parse::<u64>().ok()?;
    let addr = u32::from_str_radix(addr_tok, 16).ok()?;
    Some(ProfiledAddr {
        addr: Addr::new(addr),
        count,
        cycles,
    })
}

/// Sum (instructions, cycles) over entries with from <= addr < to.
pub fn sum_range(entries: &[ProfiledAddr], from: Addr, to: Addr) -> (u64, u64) {
    entries
        .iter()
        .filter(|e| e.addr.raw() >= from.raw() && e.addr.raw() < to.raw())
        .fold((0, 0), |(c, cy), e| (c + e.count, cy + e.cycles))
}

/// Sum (instructions, cycles) over ALL profiled addresses — the
/// whole-window total (RAM + TOS + cartridge, incl. interrupt handlers).
pub fn sum_all(entries: &[ProfiledAddr]) -> (u64, u64) {
    entries
        .iter()
        .fold((0, 0), |(c, cy), e| (c + e.count, cy + e.cycles))
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

    const FIXTURE: &str = include_str!("../../tests/fixtures/hatari_profile.txt");

    #[test]
    fn console_fallback_rewrites_stub_file_only() {
        let dir = std::env::temp_dir().join(format!("shiraz-fb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("stub.txt");
        std::fs::write(&stub, "Hatari profile\n[...]\n[...]\n").unwrap();
        // stub file + console with data -> file replaced by the console dump
        console_fallback(&stub, FIXTURE).unwrap();
        let got = std::fs::read_to_string(&stub).unwrap();
        assert!(!parse_profile_file(&got).is_empty());
        // file with data is never overwritten, even if the console has none
        let good = dir.join("good.txt");
        std::fs::write(&good, FIXTURE).unwrap();
        console_fallback(&good, "Disassembled 0 CPU addresses.").unwrap();
        assert_eq!(std::fs::read_to_string(&good).unwrap(), FIXTURE);
        // stub file + console without data -> left alone (error surfaces as zeros, as before)
        let stub2 = dir.join("stub2.txt");
        std::fs::write(&stub2, "[...]\n").unwrap();
        console_fallback(&stub2, "Disassembled 0 CPU addresses.").unwrap();
        assert_eq!(std::fs::read_to_string(&stub2).unwrap(), "[...]\n");
    }

    #[tokio::test]
    async fn profile_on_off_save_parse_confirmations() {
        let mut fake = Fake {
            responses: vec!["Profiling enabled.".to_string()],
            sent: vec![],
        };
        profile_on(&mut fake).await.unwrap();

        let mut fake = Fake {
            responses: vec!["Profiling disabled.".to_string()],
            sent: vec![],
        };
        profile_off(&mut fake).await.unwrap();

        let mut fake = Fake {
            responses: vec!["Disassembled 230 (of active 230) CPU addresses.".to_string()],
            sent: vec![],
        };
        profile_save(&mut fake, Path::new("/tmp/p.txt"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn profile_on_wrong_text_fails() {
        let mut fake = Fake {
            responses: vec!["huh?".to_string()],
            sent: vec![],
        };
        let err = profile_on(&mut fake).await.unwrap_err();
        assert!(matches!(err, Error::Parse { .. }));
    }

    #[test]
    fn profile_file_parse() {
        let entries = parse_profile_file(FIXTURE);
        // P2.md §10.3's table says "13 entries" but the §9 fixture text
        // ("commit exactly this") contains 8 RAM-side data lines plus 3
        // ROM-side data lines = 11 — the committed fixture is authoritative
        // (an internal spec miscount, not a live-Hatari mismatch).
        assert_eq!(entries.len(), 11);
        assert_eq!(
            entries[0],
            ProfiledAddr {
                addr: Addr::new(0xCE86),
                count: 1,
                cycles: 8
            }
        );
        let rte = entries.last().unwrap();
        assert_eq!(
            *rte,
            ProfiledAddr {
                addr: Addr::new(0xE00800),
                count: 9,
                cycles: 180
            }
        );
    }

    #[test]
    fn profile_sum_all() {
        let entries = parse_profile_file(FIXTURE);
        // 8 RAM-side lines (count=1 each => 8; cycles 8+4+4+12+16+16+12+12=84)
        // plus 3 ROM-side lines (count=9 each => 27; cycles 72+90+180=342).
        assert_eq!(sum_all(&entries), (8 + 27, 84 + 342));
    }

    #[test]
    fn profile_sum_range() {
        let entries = parse_profile_file(FIXTURE);
        // P2.md §10.3 says "(8, 88)" but the §9 fixture's 8 RAM-side data
        // lines (8+4+4+12+16+16+12+12) actually sum to 84 — same kind of
        // internal spec miscount as the 13-vs-11 entry count above; the
        // committed fixture is authoritative.
        assert_eq!(
            sum_range(&entries, Addr::new(0xCE86), Addr::new(0xD200)),
            (8, 84)
        );
        assert_eq!(
            sum_range(&entries, Addr::new(0xE00800), Addr::new(0xE00802)),
            (9, 180)
        );
    }
}
