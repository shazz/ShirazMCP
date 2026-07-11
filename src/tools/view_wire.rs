//! Pure wire-parsing helpers for `view_memory_as_screen` (P4) — split out of
//! view.rs to stay under the 200-line budget (same "wire.rs" split as
//! rip.rs/rip_wire.rs). No session/emulator access; unit-tested standalone.

use rmcp::ErrorData as McpError;

use crate::ripper::image::ScreenRes;

pub(crate) fn res_from_str(s: &str) -> Result<ScreenRes, McpError> {
    match s.to_ascii_lowercase().as_str() {
        "low" => Ok(ScreenRes::Low),
        "med" | "medium" => Ok(ScreenRes::Med),
        "high" => Ok(ScreenRes::High),
        _ => Err(McpError::invalid_params(
            format!("unknown res '{s}': use low|medium|high"),
            None,
        )),
    }
}

/// Exactly 16 entries; each "$0777" / "0x777" / decimal, masked to ≤ 0xFFF
/// (the upper nibble is unused hardware-side).
pub(crate) fn parse_palette_arg(v: Vec<String>) -> Result<[u16; 16], McpError> {
    if v.len() != 16 {
        return Err(McpError::invalid_params(
            format!("palette needs exactly 16 words, got {}", v.len()),
            None,
        ));
    }
    let mut out = [0u16; 16];
    for (o, s) in out.iter_mut().zip(&v) {
        *o = parse_word(s)? & 0x0FFF;
    }
    Ok(out)
}

fn parse_word(s: &str) -> Result<u16, McpError> {
    let bad = || McpError::invalid_params(format!("bad palette word {s:?}"), None);
    let (digits, radix) = match (
        s.strip_prefix('$'),
        s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")),
    ) {
        (Some(h), _) => (h, 16),
        (None, Some(h)) => (h, 16),
        _ => (s, 10),
    };
    u16::from_str_radix(digits, radix).map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn res_from_str_all() {
        assert_eq!(res_from_str("low").unwrap(), ScreenRes::Low);
        assert_eq!(res_from_str("med").unwrap(), ScreenRes::Med);
        assert_eq!(res_from_str("medium").unwrap(), ScreenRes::Med);
        assert_eq!(res_from_str("HIGH").unwrap(), ScreenRes::High);
        assert!(res_from_str("hi").is_err());
    }

    fn words(n: usize, v: &str) -> Vec<String> {
        std::iter::repeat_n(v.to_string(), n).collect()
    }

    #[test]
    fn parse_palette_arg_shapes() {
        let mixed: Vec<String> = (0..16)
            .map(|i| match i % 3 {
                0 => format!("${i:03X}"),
                1 => format!("0x{i:03X}"),
                _ => i.to_string(),
            })
            .collect();
        assert!(parse_palette_arg(mixed).is_ok());

        assert!(parse_palette_arg(words(15, "$000")).is_err());
        assert!(parse_palette_arg(words(17, "$000")).is_err());
        assert!(parse_palette_arg(words(16, "banana")).is_err());

        let masked = parse_palette_arg(words(16, "$1777")).unwrap();
        assert_eq!(masked[0], 0x0777);
    }
}
