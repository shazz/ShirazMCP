//! ZX0 v2 (Einar Saukas) depacker — the first HEADERLESS scheme (ADR-25).
//! There is no magic to scan for, so the auto-engine gate is
//! whole-buffer-exact decode at offset 0 (`try_unpack`); an in-RAM stream at
//! an arbitrary address goes through the `depack` tool's prefix-tolerant
//! seam (`try_unpack_prefix`). v1 "classic" and the backwards (`-b`)
//! variant are deferred — see `zx0_decode` and ADR-25 §4.0.

use super::zx0_decode::decrunch;
use crate::ripper::Depacker;
use crate::types::PluginId;

pub struct Zx0;

impl Depacker for Zx0 {
    fn id(&self) -> PluginId {
        PluginId::new("zx0")
    }
    fn describe(&self) -> &'static str {
        "ZX0 v2 depacker — headerless: auto-rip unpacks only a whole-buffer-exact \
         stream; use the depack tool with an explicit address for streams inside RAM"
    }
    /// Auto-engine gate (ADR-25): offset 0, FULL consumption, non-empty.
    fn try_unpack(&self, data: &[u8]) -> Option<Vec<u8>> {
        let (out, consumed) = decrunch(data)?;
        (consumed == data.len() && !out.is_empty()).then_some(out)
    }
    /// Explicit path: trailing bytes after the stream are fine.
    fn try_unpack_prefix(&self, data: &[u8]) -> Option<(Vec<u8>, usize)> {
        decrunch(data).filter(|(out, _)| !out.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ripper::plugins::pack_ice::PackIce;

    const Z1: &[u8] = &[0xD5, 0x41, 0x55, 0x60];

    // Pack-Ice's V1_ICE vector (15 bytes, P5 §6.2) — a magic-scanning
    // scheme's blob, used to pin the trait's default `try_unpack_prefix`.
    const V1_ICE: &[u8] = &[
        0x49, 0x43, 0x45, 0x21, 0x00, 0x00, 0x00, 0x0F, 0x00, 0x00, 0x00, 0x06, 0x01, 0x41, 0xB7,
    ];

    #[test]
    fn exact_gate() {
        assert_eq!(Zx0.try_unpack(Z1), Some(b"A".to_vec()));
        let mut with_trailer = Z1.to_vec();
        with_trailer.push(0x00);
        assert_eq!(Zx0.try_unpack(&with_trailer), None);
        assert_eq!(Zx0.try_unpack(&[]), None);
    }

    #[test]
    fn prefix_override() {
        let mut buf = Z1.to_vec();
        buf.extend(b"junk");
        assert_eq!(Zx0.try_unpack_prefix(&buf), Some((b"A".to_vec(), Z1.len())));
    }

    #[test]
    fn default_prefix_method() {
        assert_eq!(
            PackIce.try_unpack_prefix(V1_ICE),
            Some((b"AAAAAA".to_vec(), V1_ICE.len()))
        );
    }
}
