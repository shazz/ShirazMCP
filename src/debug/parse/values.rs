//! `evaluate`/`m` (memdump) parsing — split out of `parse/mod.rs` (§3
//! file-size cleanup), grouped as the "numeric extraction" family. No
//! behavior change from the pre-split module.

use super::parse_err;
use crate::error::Result;

/// Parse `evaluate` output: `"= %<bin> (bin), #<dec> (dec), $<hex> (hex)"`.
pub fn parse_evaluate(out: &str) -> Result<u32> {
    let line = out
        .lines()
        .find(|l| l.starts_with("= %"))
        .ok_or_else(|| parse_err("evaluate", out))?;
    let after_hash = line
        .split('#')
        .nth(1)
        .ok_or_else(|| parse_err("evaluate", line))?;
    let dec = after_hash
        .split(' ')
        .next()
        .ok_or_else(|| parse_err("evaluate", line))?;
    dec.parse().map_err(|_| parse_err("evaluate", line))
}

/// Parse `m` memdump output into bytes. Lines without `": "` are skipped
/// (headers/echo leakage); the ASCII gutter is excluded by taking only the
/// first 47 chars after the address (16 hex byte pairs + 15 separators).
pub fn parse_memdump(out: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for line in out.lines() {
        let Some((_, rest)) = line.split_once(": ") else {
            continue;
        };
        let hex_field: String = rest.chars().take(47).collect();
        for tok in hex_field.split_whitespace() {
            if let Ok(b) = u8::from_str_radix(tok, 16) {
                bytes.push(b);
            }
        }
    }
    if bytes.is_empty() {
        return Err(parse_err("memdump", out));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::super::fixture_block;
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/hatari_session.log");

    #[test]
    fn evaluate_parses_decimal_field() {
        let block = fixture_block(FIXTURE, "evaluate pc");
        assert_eq!(parse_evaluate(&block).unwrap(), 0xdc88);
    }

    #[test]
    fn evaluate_garbage_is_parse_error() {
        assert!(parse_evaluate("= huh").is_err());
    }

    #[test]
    fn memdump_parses_fixture_line() {
        let block = fixture_block(FIXTURE, "m $f8000 8");
        assert_eq!(parse_memdump(&block).unwrap(), vec![0u8; 16]);
    }

    #[test]
    fn memdump_parses_32_bit_address_prefix() {
        let line = "01000010: CA FE BA BE 00 00 00 00 00 00 00 00 00 00 00 00   ................";
        assert_eq!(
            &parse_memdump(line).unwrap()[..4],
            &[0xCA, 0xFE, 0xBA, 0xBE]
        );
    }

    #[test]
    fn memdump_synthetic_nonzero() {
        let line = "00001000: DE AD BE EF 00 00 00 00 00 00 00 00 00 00 00 00   ................";
        let bytes = parse_memdump(line).unwrap();
        assert_eq!(&bytes[..4], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn memdump_ascii_gutter_ignored() {
        let line = "00001000: DE AD BE EF 00 00 00 00 00 00 00 00 00 00 00 00   00 11.......";
        let bytes = parse_memdump(line).unwrap();
        assert_eq!(bytes.len(), 16);
    }

    #[test]
    fn memdump_empty_is_parse_error() {
        assert!(parse_memdump("no colon here").is_err());
    }
}
