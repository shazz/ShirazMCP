//! `symbols`/`screenshot` parsing — split out of `parse/mod.rs` (§3
//! file-size cleanup), grouped as the "outcome line" family. No behavior
//! change from the pre-split module.

use super::parse_err;
use crate::debug::SymbolLoad;
use crate::error::Result;

/// Parse `symbols prg`: `"Loaded N symbols (M TEXT) from '...'."` on
/// success, or a `"symbol table missing"` line (ADR-8's no-symbols case).
pub fn parse_symbols(out: &str) -> Result<SymbolLoad> {
    for line in out.lines() {
        if line.contains("symbol table missing") {
            return Ok(SymbolLoad::NoSymbols);
        }
        if let Some(rest) = line.strip_prefix("Loaded ") {
            if let Some(loaded) = parse_loaded_counts(rest) {
                return Ok(loaded);
            }
        }
    }
    Err(parse_err("symbols prg", out))
}

fn parse_loaded_counts(rest: &str) -> Option<SymbolLoad> {
    let total: u32 = rest.split_whitespace().next()?.parse().ok()?;
    let text_tok = rest.split('(').nth(1)?.split_whitespace().next()?;
    let text: u32 = text_tok.parse().ok()?;
    Some(SymbolLoad::Loaded { total, text })
}

/// The '…' path inside "Loaded N symbols (M …) from '<path>'." (P26 §5.1)
/// — None when absent. Tolerates both v2.4.1 "(M TEXT)" and HEAD "(M for
/// code)" since it only looks at the trailing `from '...'.` clause.
pub fn parse_loaded_path(out: &str) -> Option<String> {
    let line = out.lines().find(|l| l.starts_with("Loaded "))?;
    let after_from = line.rsplit_once("from '")?.1;
    let path = after_from.strip_suffix("'.")?;
    Some(path.to_string())
}

/// Parse `screenshot` output: a line containing both "Screen dump to" and
/// "succeeded" means success.
pub fn parse_screenshot(out: &str) -> Result<()> {
    let ok = out
        .lines()
        .any(|l| l.contains("Screen dump to") && l.contains("succeeded"));
    if ok {
        Ok(())
    } else {
        Err(parse_err("screenshot", out))
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture_block;
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/hatari_session.log");

    #[test]
    fn symbols_error_line_is_no_symbols() {
        let block = fixture_block(FIXTURE, "symbols prg");
        assert_eq!(parse_symbols(&block).unwrap(), SymbolLoad::NoSymbols);
    }

    #[test]
    fn symbols_loaded_line_parses() {
        let line = "Loaded 184 symbols (40 TEXT) from 'x'.";
        assert_eq!(
            parse_symbols(line).unwrap(),
            SymbolLoad::Loaded {
                total: 184,
                text: 40
            }
        );
    }

    #[test]
    fn loaded_path_v241_shape() {
        let line = "Loaded 184 symbols (40 TEXT) from 'x'.";
        assert_eq!(parse_loaded_path(line).as_deref(), Some("x"));
    }

    #[test]
    fn loaded_path_head_shape() {
        let line = "Loaded 184 symbols (40 for code) from '/a/b.prg'.";
        assert_eq!(parse_loaded_path(line).as_deref(), Some("/a/b.prg"));
    }

    #[test]
    fn loaded_path_absent_without_from() {
        assert_eq!(parse_loaded_path("Loaded 0 symbols."), None);
    }

    #[test]
    fn screenshot_success_line() {
        let block = fixture_block(FIXTURE, "screenshot /tmp/hatari_shot.png");
        assert!(parse_screenshot(&block).is_ok());
    }

    #[test]
    fn screenshot_missing_is_parse_error() {
        assert!(parse_screenshot("nope").is_err());
    }
}
