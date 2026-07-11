//! `Addr`, split out of `types.rs` (§3 file-size cleanup) — no behavior
//! change from the pre-split module.

use std::fmt;

/// A 68000/Atari-ST bus address. Newtype so raw `u32`s never cross a public
/// signature boundary (PLAN.md conventions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Addr(u32);

impl Addr {
    /// Exclusive end of the 68000/ST 24-bit address bus (ADR-31) — the one
    /// shared ceiling for "anywhere on the bus" checks (`parse`'s own
    /// range gate, `watch`'s literal region, `basepage`'s os_beg bound).
    pub const BUS_LEN: u32 = 0x0100_0000;

    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Parse an agent-supplied address: `$FF8240`, `0xFF8240` (case-
    /// insensitive prefix/digits), or plain decimal `65280`. Rejects values
    /// >= `BUS_LEN` (24-bit ST bus) and empty/garbage input.
    pub fn parse(s: &str) -> crate::error::Result<Self> {
        let bad = || crate::error::Error::Parse {
            context: "address".to_string(),
            input: s.to_string(),
        };
        let (digits, radix) = if let Some(hex) = s.strip_prefix('$') {
            (hex, 16)
        } else if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            (hex, 16)
        } else {
            (s, 10)
        };
        let raw = u32::from_str_radix(digits, radix).map_err(|_| bad())?;
        if raw >= Self::BUS_LEN {
            return Err(bad());
        }
        Ok(Self(raw))
    }
}

/// Display: ST debugger convention — `$` + UPPERCASE hex, zero-padded to a
/// MINIMUM of 6 digits; values above 0xFF_FFFF print all their digits
/// (never truncated). Given in full because the body IS the contract:
impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "${:06X}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addr_display_pads_to_six() {
        let a = Addr::new(0x12);
        assert_eq!(format!("{a}"), "$000012");
    }

    #[test]
    fn addr_display_typical_register() {
        let a = Addr::new(0xFF8240);
        assert_eq!(format!("{a}"), "$FF8240");
    }

    #[test]
    fn addr_display_wide_not_truncated() {
        let a = Addr::new(0x1AB_CDEF);
        assert_eq!(format!("{a}"), "$1ABCDEF");
    }

    #[test]
    fn addr_raw_roundtrip() {
        assert_eq!(Addr::new(42).raw(), 42);
    }

    #[test]
    fn addr_parse_dollar_hex() {
        assert_eq!(Addr::parse("$FF8240").unwrap().raw(), 0xFF8240);
    }

    #[test]
    fn addr_parse_0x_hex() {
        assert_eq!(Addr::parse("0xff8240").unwrap().raw(), 0xFF8240);
    }

    #[test]
    fn addr_parse_decimal() {
        assert_eq!(Addr::parse("1024").unwrap().raw(), 1024);
    }

    #[test]
    fn addr_parse_rejects_wide() {
        assert!(matches!(
            Addr::parse("$1000000"),
            Err(crate::error::Error::Parse { .. })
        ));
    }

    #[test]
    fn addr_parse_rejects_garbage() {
        assert!(matches!(
            Addr::parse("banana"),
            Err(crate::error::Error::Parse { .. })
        ));
        assert!(matches!(
            Addr::parse(""),
            Err(crate::error::Error::Parse { .. })
        ));
    }
}
