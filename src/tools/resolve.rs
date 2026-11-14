//! Address resolution — the ONE seam where a tool-supplied string may mean
//! either a number or a symbol name (P26 §3/§6.4). Hatari is the only
//! name→address authority (`evaluate <name>`, ADR-45): this helper never
//! consults the server-side symbol store, so a name always means what a
//! breakpoint condition or `evaluate` would mean, keeping autoload/free/
//! program-exit coherent with no dual-authority drift.

use rmcp::ErrorData as McpError;

use crate::debug::{self, DebugChannel};
use crate::types::Addr;

/// Resolve an agent-supplied address string: numeric forms first
/// (`$hex`/`0xhex`/decimal — `Addr::parse`, 100% unchanged), then, ONLY if
/// that fails AND the string is a plausible Hatari identifier, ask Hatari
/// to evaluate it as a register/variable/symbol name. A string that parses
/// as a number is NEVER reinterpreted as a symbol — `Addr::parse` is a
/// strict superset gate, so hex/dec behavior of the three consuming tools
/// (disasm/profile/breakpoint's arm-hint path) is unchanged.
///
/// Algorithm (pinned, §6.4):
/// 1. `Addr::parse(s)` → `Ok(a)` ⇒ return `Ok(a)`. Numbers win, always.
/// 2. `is_symbol_candidate(s)`: not a candidate ⇒ `invalid_params` — a
///    leading-digit string like `"12x4"` is a malformed NUMBER, never a
///    symbol, and must never reach Hatari.
/// 3. `debug::evaluate(chan, s)`: resolved + in-bus ⇒ `Ok`; resolved but
///    `>= BUS_LEN` ⇒ `invalid_params`; unresolved ⇒ `invalid_params` with a
///    hint to load a symbol map.
pub(crate) async fn resolve_addr(chan: &mut impl DebugChannel, s: &str) -> Result<Addr, McpError> {
    let limit = chan.bus_limit();
    if let Ok(a) = Addr::parse(s) {
        return in_bus(a, limit);
    }
    if !is_symbol_candidate(s) {
        return Err(McpError::invalid_params(
            format!("not a valid address ($hex, 0xhex or decimal) and not a symbol name: {s}"),
            None,
        ));
    }
    match debug::evaluate(chan, s).await {
        Ok(v) if u64::from(v) < limit => Ok(Addr::new(v)),
        Ok(v) => Err(McpError::invalid_params(
            format!("symbol '{s}' = ${v:x} is outside the {}", bus_name(limit)),
            None,
        )),
        Err(_) => Err(McpError::invalid_params(
            format!(
                "'{s}' is neither a number nor a known register/variable/symbol. If it is a \
                 label, load the symbol map first: symbols(action=\"load\") for a GEMDOS-HD \
                 .PRG with a symbol table (vasm -Ftos default), or symbols(action=\"load\", \
                 path=…) for an nm-style .sym file."
            ),
            None,
        )),
    }
}

fn bus_name(limit: u64) -> &'static str {
    if limit == u64::from(Addr::BUS_LEN) {
        "24-bit ST bus (start with addr24=false for 32-bit addresses)"
    } else {
        "32-bit address space"
    }
}

/// A parsed numeric address must fit the session's bus (24-bit unless
/// `addr24=false`): Hatari would otherwise silently MASK it to 24 bits.
fn in_bus(a: Addr, limit: u64) -> Result<Addr, McpError> {
    if u64::from(a.raw()) >= limit {
        let msg = format!("{a} is outside the {}", bus_name(limit));
        return Err(McpError::invalid_params(msg, None));
    }
    Ok(a)
}

/// `resolve_addr` + the disasm even-address gate (same message as the old
/// `parse_even_addr`: "disasm address must be even").
pub(crate) async fn resolve_even_addr(
    chan: &mut impl DebugChannel,
    s: &str,
) -> Result<Addr, McpError> {
    let addr = resolve_addr(chan, s).await?;
    if addr.raw() % 2 != 0 {
        return Err(McpError::invalid_params(
            "disasm address must be even",
            None,
        ));
    }
    Ok(addr)
}

/// Hatari's identifier scanner (evaluate.c v2.4.1:204-263, §2.1): first
/// char `[A-Za-z_]`, rest `[A-Za-z0-9_]`, length 1..=63 (`char name[64]`).
/// A leading digit is a malformed NUMBER, never a symbol — this gate keeps
/// `"12x4"` from ever reaching Hatari.
fn is_symbol_candidate(s: &str) -> bool {
    if s.is_empty() || s.len() > 63 {
        return false;
    }
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;

    struct Fake {
        responses: Vec<Result<String>>,
        sent: Vec<String>,
    }

    /// A 32-bit session (`addr24=false`): the channel reports a 4 GiB bus.
    struct Wide(Fake);

    impl DebugChannel for Wide {
        async fn debug(&mut self, cmd: &str) -> Result<String> {
            self.0.debug(cmd).await
        }
        fn bus_limit(&self) -> u64 {
            1 << 32
        }
    }

    impl DebugChannel for Fake {
        async fn debug(&mut self, cmd: &str) -> Result<String> {
            self.sent.push(cmd.to_string());
            self.responses.remove(0)
        }
    }

    fn evaluate_ok(v: u32) -> Result<String> {
        Ok(format!("= %0 (bin), #{v} (dec), ${v:x} (hex)"))
    }

    fn evaluate_unresolved() -> Result<String> {
        Ok("Invalid value 'x'.".to_string())
    }

    #[tokio::test]
    async fn hex_number_never_round_trips() {
        let mut fake = Fake {
            responses: vec![],
            sent: vec![],
        };
        let addr = resolve_addr(&mut fake, "$FF8240").await.unwrap();
        assert_eq!(addr, Addr::new(0xFF8240));
        assert!(fake.sent.is_empty());
    }

    #[tokio::test]
    async fn decimal_number_never_round_trips() {
        let mut fake = Fake {
            responses: vec![],
            sent: vec![],
        };
        let addr = resolve_addr(&mut fake, "1024").await.unwrap();
        assert_eq!(addr, Addr::new(1024));
        assert!(fake.sent.is_empty());
    }

    #[tokio::test]
    async fn known_symbol_resolves_via_evaluate() {
        let mut fake = Fake {
            responses: vec![evaluate_ok(0x70000)],
            sent: vec![],
        };
        let addr = resolve_addr(&mut fake, "BlitSphere").await.unwrap();
        assert_eq!(addr, Addr::new(0x70000));
        assert_eq!(fake.sent, vec!["evaluate BlitSphere".to_string()]);
    }

    #[tokio::test]
    async fn unknown_symbol_hints_at_symbols_tool() {
        let mut fake = Fake {
            responses: vec![evaluate_unresolved()],
            sent: vec![],
        };
        let err = resolve_addr(&mut fake, "NoSuch").await.unwrap_err();
        assert!(err.message.contains("symbols(action=\"load\")"));
    }

    #[tokio::test]
    async fn malformed_number_never_reaches_hatari() {
        let mut fake = Fake {
            responses: vec![],
            sent: vec![],
        };
        let err = resolve_addr(&mut fake, "12x4").await.unwrap_err();
        assert!(fake.sent.is_empty());
        assert!(err.message.contains("not a valid address"));
    }

    #[tokio::test]
    async fn overlong_identifier_never_reaches_hatari() {
        let mut fake = Fake {
            responses: vec![],
            sent: vec![],
        };
        let long = "_a".repeat(32); // 64 chars, over the 63-char limit
        let err = resolve_addr(&mut fake, &long).await.unwrap_err();
        assert!(fake.sent.is_empty());
        assert!(err.message.contains("not a symbol name"));
    }

    #[tokio::test]
    async fn dashed_name_is_unreachable_and_never_sent() {
        let mut fake = Fake {
            responses: vec![],
            sent: vec![],
        };
        let err = resolve_addr(&mut fake, "with-dash").await.unwrap_err();
        assert!(fake.sent.is_empty());
        assert!(err.message.contains("not a symbol name"));
    }

    #[tokio::test]
    async fn symbol_beyond_bus_is_rejected() {
        let mut fake = Fake {
            responses: vec![evaluate_ok(0xFF00_0000)],
            sent: vec![],
        };
        let err = resolve_addr(&mut fake, "TooFar").await.unwrap_err();
        assert!(err.message.contains("outside the 24-bit ST bus"));
    }

    #[tokio::test]
    async fn numeric_beyond_24_bit_is_rejected_on_st_bus() {
        let mut fake = Fake {
            responses: vec![],
            sent: vec![],
        };
        let err = resolve_addr(&mut fake, "$1000000").await.unwrap_err();
        assert!(err.message.contains("24-bit ST bus"));
        assert!(fake.sent.is_empty());
    }

    #[tokio::test]
    async fn tt_ram_address_and_symbol_resolve_on_wide_bus() {
        let mut wide = Wide(Fake {
            responses: vec![evaluate_ok(0x0140_0000)],
            sent: vec![],
        });
        let a = resolve_addr(&mut wide, "$01000000").await.unwrap();
        assert_eq!(a, Addr::new(0x0100_0000));
        let s = resolve_addr(&mut wide, "TtLabel").await.unwrap();
        assert_eq!(s, Addr::new(0x0140_0000));
        let top = resolve_addr(&mut wide, "$FFFFFFFE").await.unwrap();
        assert_eq!(top.raw(), 0xFFFF_FFFE);
    }

    #[tokio::test]
    async fn resolve_even_addr_rejects_odd_symbol() {
        let mut fake = Fake {
            responses: vec![evaluate_ok(0x70001)],
            sent: vec![],
        };
        let err = resolve_even_addr(&mut fake, "OddLabel").await.unwrap_err();
        assert!(err.message.contains("must be even"));
    }

    #[tokio::test]
    async fn resolve_even_addr_accepts_even_number() {
        let mut fake = Fake {
            responses: vec![],
            sent: vec![],
        };
        let addr = resolve_even_addr(&mut fake, "$1000").await.unwrap();
        assert_eq!(addr, Addr::new(0x1000));
    }

    #[test]
    fn is_symbol_candidate_accepts_identifier() {
        assert!(is_symbol_candidate("BlitSphere"));
        assert!(is_symbol_candidate("_x9"));
    }

    #[test]
    fn is_symbol_candidate_rejects_leading_digit_and_bad_chars() {
        assert!(!is_symbol_candidate("12x4"));
        assert!(!is_symbol_candidate("with-dash"));
        assert!(!is_symbol_candidate(""));
    }
}
