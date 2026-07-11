//! Pack-Ice (ICE!/Ice!) plugins: the `pack-ice` Depacker and the
//! `pack-ice-region` Packer-kind detector (ADR-14). Header layout and
//! sanity gates: P5 §4.1, from Toyoyo's ice_info.c/ice.h. Decoding is in
//! `ice_decode` (line budget; still one registry line per plugin).

use super::ice_decode::decrunch;
use crate::ripper::registry::MAX_UNPACKED;
use crate::ripper::{Depacker, Finding, RipKind, RipPlugin};
use crate::types::{MemImage, Offset, PluginId};

/// Both header variants share the 12-byte layout (§4.1).
const HEADER_LEN: usize = 12;

struct IceHeader {
    magic: &'static str, // "ICE!" or "Ice!" — for labels
    crunched: usize,
    decrunched: usize,
}

/// §4.1 sanity gates. None = not a usable blob at `o` (wrong/short header,
/// absurd lengths, or region truncated by the buffer edge).
fn parse_header(bytes: &[u8], o: usize) -> Option<IceHeader> {
    let head = bytes.get(o..o + HEADER_LEN)?;
    let magic = match &head[..4] {
        b"ICE!" => "ICE!",
        b"Ice!" => "Ice!",
        _ => return None,
    };
    let crunched = u32::from_be_bytes(head[4..8].try_into().ok()?) as usize;
    let decrunched = u32::from_be_bytes(head[8..12].try_into().ok()?) as usize;
    if crunched < HEADER_LEN + 1
        || decrunched == 0
        || decrunched > MAX_UNPACKED
        || o.checked_add(crunched).is_none_or(|end| end > bytes.len())
    {
        return None;
    }
    Some(IceHeader {
        magic,
        crunched,
        decrunched,
    })
}

/// All plausible blobs, in ascending offset order. Two memmem passes (both
/// magics), merged; NO alignment assumption — packed data inside files is
/// arbitrary-offset (unit-tested at an odd offset).
fn candidates(bytes: &[u8]) -> Vec<(usize, IceHeader)> {
    let mut offsets: Vec<usize> = memchr::memmem::find_iter(bytes, b"ICE!")
        .chain(memchr::memmem::find_iter(bytes, b"Ice!"))
        .collect();
    offsets.sort_unstable();
    offsets
        .into_iter()
        .filter_map(|o| parse_header(bytes, o).map(|h| (o, h)))
        .collect()
}

pub struct PackIce;

impl Depacker for PackIce {
    fn id(&self) -> PluginId {
        PluginId::new("pack-ice")
    }
    fn describe(&self) -> &'static str {
        "Pack-Ice (ICE!/Ice!) depacker — unpacks the first valid blob found"
    }
    /// The engine hands the WHOLE buffer (P3 contract): scan for magics,
    /// depack the first candidate whose stream decodes cleanly. Multiple
    /// packed regions in one buffer: only the first is recursed (trait
    /// returns one buffer — ADR-14 consequence); the detector still
    /// reports all of them.
    fn try_unpack(&self, data: &[u8]) -> Option<Vec<u8>> {
        candidates(data)
            .into_iter()
            .find_map(|(o, h)| decrunch(&data[o + HEADER_LEN..o + h.crunched], h.decrunched))
    }
}

pub struct PackIceRegion;

impl RipPlugin for PackIceRegion {
    fn id(&self) -> PluginId {
        PluginId::new("pack-ice-region")
    }
    fn kind(&self) -> RipKind {
        RipKind::Packer
    }
    fn describe(&self) -> &'static str {
        "Pack-Ice (ICE!/Ice!) packed regions — header report only, no depack"
    }
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut next = 0usize;
        for (o, h) in candidates(mem.bytes()) {
            if o < next {
                continue; // magic inside a region already reported
            }
            next = o + h.crunched;
            out.push(Finding::new(
                self.id(),
                RipKind::Packer,
                Offset::new(o),
                h.crunched,
                format!(
                    "Pack-Ice packed region ('{}'), {} packed -> {} unpacked bytes",
                    h.magic, h.crunched, h.decrunched
                ),
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Full blobs (header + stream), P5 §6.2.
    const V1_ICE: &[u8] = &[
        0x49, 0x43, 0x45, 0x21, 0x00, 0x00, 0x00, 0x0F, 0x00, 0x00, 0x00, 0x06, 0x01, 0x41, 0xB7,
    ];
    const V1_ICE_LOWER: &[u8] = &[
        0x49, 0x63, 0x65, 0x21, 0x00, 0x00, 0x00, 0x0F, 0x00, 0x00, 0x00, 0x06, 0x01, 0x41, 0xB7,
    ];
    const V3_ICE: &[u8] = &[
        0x49, 0x43, 0x45, 0x21, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x04, 0x0B, 0x41, 0x42,
        0xC1,
    ];

    fn plant(buf: &mut [u8], at: usize, data: &[u8]) {
        buf[at..at + data.len()].copy_from_slice(data);
    }

    #[test]
    fn parse_header_both_magics() {
        let ice = parse_header(V1_ICE, 0).unwrap();
        assert_eq!(ice.crunched, 15);
        assert_eq!(ice.decrunched, 6);
        assert_eq!(ice.magic, "ICE!");
        let lower = parse_header(V1_ICE_LOWER, 0).unwrap();
        assert_eq!(lower.magic, "Ice!");
    }

    #[test]
    fn parse_header_rejects() {
        assert!(parse_header(b"ICX!\x00\x00\x00\x0F\x00\x00\x00\x06\x01\x41\xB7", 0).is_none());
        assert!(parse_header(&V1_ICE[..11], 0).is_none());
        let mut crunched12 = V1_ICE.to_vec();
        crunched12[4..8].copy_from_slice(&12u32.to_be_bytes());
        assert!(parse_header(&crunched12, 0).is_none());
        let mut zero_decrunched = V1_ICE.to_vec();
        zero_decrunched[8..12].copy_from_slice(&0u32.to_be_bytes());
        assert!(parse_header(&zero_decrunched, 0).is_none());
        let mut huge_decrunched = V1_ICE.to_vec();
        huge_decrunched[8..12].copy_from_slice(&(MAX_UNPACKED as u32 + 1).to_be_bytes());
        assert!(parse_header(&huge_decrunched, 0).is_none());
        assert!(parse_header(&V1_ICE[..14], 0).is_none());
    }

    #[test]
    fn try_unpack_at_odd_offset() {
        let mut buf = vec![0u8; 4096];
        plant(&mut buf, 0x101, V1_ICE);
        assert_eq!(PackIce.try_unpack(&buf), Some(b"AAAAAA".to_vec()));
    }

    #[test]
    fn try_unpack_skips_corrupt_finds_valid() {
        let mut buf = vec![0u8; 4096];
        let mut corrupt = V1_ICE.to_vec();
        *corrupt.last_mut().unwrap() = 0x00;
        plant(&mut buf, 0x40, &corrupt);
        plant(&mut buf, 0x800, V3_ICE);
        assert_eq!(PackIce.try_unpack(&buf), Some(b"ABAB".to_vec()));
    }

    #[test]
    fn detector_reports_and_skips_interior_magic() {
        // (a) overlapping: V3_ICE planted inside V1_ICE's region is skipped.
        let mut overlap = vec![0u8; 4096];
        plant(&mut overlap, 0x20, V1_ICE);
        plant(&mut overlap, 0x2C, V3_ICE);
        let mem = MemImage::new(overlap, crate::types::Addr::new(0));
        let findings = PackIceRegion.scan(&mem);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].offset, Offset::new(0x20));

        // (b) disjoint: both reported.
        let mut disjoint = vec![0u8; 4096];
        plant(&mut disjoint, 0x20, V1_ICE);
        plant(&mut disjoint, 0x40, V3_ICE);
        let mem = MemImage::new(disjoint, crate::types::Addr::new(0));
        let findings = PackIceRegion.scan(&mem);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].len, 15);
        assert_eq!(findings[1].len, 16);
        assert!(findings[0].label.contains("15 packed -> 6 unpacked"));
        assert!(findings[1].label.contains("16 packed -> 4 unpacked"));
    }

    #[test]
    fn detector_zero_ram_is_empty() {
        let zeros = MemImage::new(vec![0u8; 64 * 1024], crate::types::Addr::new(0));
        assert!(PackIceRegion.scan(&zeros).is_empty());
        let teaser: Vec<u8> = std::iter::repeat_n([0x49u8, 0x43], 32 * 1024)
            .flatten()
            .collect();
        let mem = MemImage::new(teaser, crate::types::Addr::new(0));
        assert!(PackIceRegion.scan(&mem).is_empty());
    }
}
