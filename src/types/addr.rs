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
    /// insensitive prefix/digits), or plain decimal `65280`. Accepts the full
    /// 32-bit range (TT-RAM / 68060 sessions run with `addr24=false`);
    /// rejects empty/garbage input. Whether the value fits the SESSION's bus
    /// is `HatariConfig::check_addr`'s job - this parser has no session.
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
        Ok(Self(raw))
    }

    /// The 24-bit hardware-map key for a raw bus address, or `None` when the
    /// address is NOT in the ST's 24-bit space: `< BUS_LEN` is itself, and
    /// `$FF000000+` is the sign-extended 32-bit alias of the top 16 MB
    /// (`$FFFF8240` == `$FF8240`). Anything between (TT-RAM at `$01000000`+,
    /// 68060 address space) is NOT an I/O or ROM address - masking it to 24
    /// bits would mis-annotate `$01FF8240` as a palette register.
    pub const fn bus24(raw: u32) -> Option<u32> {
        if raw < Self::BUS_LEN {
            Some(raw)
        } else if raw >= 0xFF00_0000 {
            Some(raw & 0x00FF_FFFF)
        } else {
            None
        }
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
    fn addr_parse_accepts_32_bit() {
        assert_eq!(Addr::parse("$1000000").unwrap().raw(), 0x100_0000);
        assert_eq!(Addr::parse("0xFFFFFFFF").unwrap().raw(), u32::MAX);
        assert!(Addr::parse("$100000000").is_err());
    }

    #[test]
    fn bus24_maps_only_real_24_bit_addresses() {
        assert_eq!(Addr::bus24(0xFF8240), Some(0xFF8240));
        assert_eq!(Addr::bus24(0xFFFF_8240), Some(0xFF8240));
        assert_eq!(Addr::bus24(0x01FF_8240), None); // TT-RAM, not a register
        assert_eq!(Addr::bus24(0x100_0000), None);
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
