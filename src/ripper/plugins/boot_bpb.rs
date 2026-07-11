//! Boot-sector checksum + DOS BPB decode (§4.2/§4.3, ADR-17) — pure byte
//! parsing, split out of `bootsector.rs` for the line budget (same pattern
//! as `sndh_header.rs` / `pack_ice.rs`'s `ice_decode.rs`).

/// A sector is executable iff its big-endian word-sum equals this.
pub const BOOT_MAGIC_SUM: u16 = 0x1234;

/// The little-endian DOS BPB (§4.3); `$011`/`$013` sit at odd offsets on
/// purpose, per the format — not a bug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bpb {
    pub bps: u16,
    pub spc: u8,
    pub res: u16,
    pub nfats: u8,
    pub ndirs: u16,
    pub nsects: u16,
    pub media: u8,
    pub spf: u16,
    pub spt: u16,
    pub nsides: u16,
    pub nhid: u16,
}

/// Sum of 256 BE 16-bit words, mod 65536; executable iff `== BOOT_MAGIC_SUM`.
/// Requires `sector.len() >= 512` (callers guarantee it — the plugin only
/// ever calls this on a 512-byte slice).
pub fn checksum(sector: &[u8]) -> u16 {
    (0..256).fold(0u16, |sum, i| {
        sum.wrapping_add(u16::from_be_bytes([sector[2 * i], sector[2 * i + 1]]))
    })
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub fn decode_bpb(s: &[u8]) -> Bpb {
    Bpb {
        bps: le16(s, 0x00B),
        spc: s[0x00D],
        res: le16(s, 0x00E),
        nfats: s[0x010],
        ndirs: le16(s, 0x011),
        nsects: le16(s, 0x013),
        media: s[0x015],
        spf: le16(s, 0x016),
        spt: le16(s, 0x018),
        nsides: le16(s, 0x01A),
        nhid: le16(s, 0x01C),
    }
}

/// Keeps a zeroed RAM buffer from being reported as a boot sector (§4.3).
pub fn bpb_plausible(b: &Bpb) -> bool {
    matches!(b.bps, 128 | 256 | 512 | 1024 | 2048 | 4096 | 8192)
        && (1..=64).contains(&b.spc)
        && (1..=2).contains(&b.nfats)
        && b.nsects >= 1
        && b.spf >= 1
        && (b.media == 0 || (0xF0..=0xFF).contains(&b.media))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_all_zero_is_zero() {
        assert_eq!(checksum(&[0u8; 512]), 0);
    }

    #[test]
    fn checksum_rejects_wrong_sum() {
        let mut s = [0u8; 512];
        s[0] = 0x12;
        s[1] = 0x35;
        assert_ne!(checksum(&s), BOOT_MAGIC_SUM);
    }

    #[test]
    fn bpb_plausible_rejects_zeroed_sector() {
        let b = decode_bpb(&[0u8; 512]);
        assert!(!bpb_plausible(&b));
    }

    #[test]
    fn bpb_plausible_accepts_standard_geometry() {
        let mut s = vec![0u8; 512];
        s[0x00B..0x00D].copy_from_slice(&512u16.to_le_bytes());
        s[0x00D] = 2;
        s[0x00E..0x010].copy_from_slice(&1u16.to_le_bytes());
        s[0x010] = 2;
        s[0x013..0x015].copy_from_slice(&720u16.to_le_bytes());
        s[0x015] = 0xF9;
        s[0x016..0x018].copy_from_slice(&5u16.to_le_bytes());
        let b = decode_bpb(&s);
        assert!(bpb_plausible(&b));
    }

    #[test]
    fn bpb_plausible_rejects_bad_media() {
        let mut s = vec![0u8; 512];
        s[0x00B..0x00D].copy_from_slice(&512u16.to_le_bytes());
        s[0x00D] = 2;
        s[0x010] = 1;
        s[0x013..0x015].copy_from_slice(&720u16.to_le_bytes());
        s[0x015] = 0x42; // not 0 and not 0xF0..=0xFF
        s[0x016..0x018].copy_from_slice(&5u16.to_le_bytes());
        let b = decode_bpb(&s);
        assert!(!bpb_plausible(&b));
    }
}
