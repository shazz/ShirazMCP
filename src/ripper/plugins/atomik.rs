//! Atomik 3.5 (`ATM5`) plugins: the `atomik` Depacker and the
//! `atomik-region` Packer-kind detector (ADR-19). Header layout and sanity
//! gates: P9 §4.A.1. Decoding is in `atomik_decode` (line budget; still
//! one registry line per plugin). Scope: `ATM5` only — see
//! `atomik_decode`'s module doc and ADR-19 for the drops (`ATOM` magic,
//! picture-mode post-processing).

use super::atomik_decode::decrunch;
use crate::ripper::registry::MAX_UNPACKED;
use crate::ripper::{Depacker, Finding, RipKind, RipPlugin};
use crate::types::{MemImage, Offset, PluginId};

const HEADER_LEN: usize = 12;

struct AtomikHeader {
    decrunched: usize,
    stream_len: usize,
}

/// §4.A.1 sanity gates. None = not a usable blob at `o`.
fn parse_header(bytes: &[u8], o: usize) -> Option<AtomikHeader> {
    let head = bytes.get(o..o + HEADER_LEN)?;
    if &head[..4] != b"ATM5" {
        return None;
    }
    let decrunched = u32::from_be_bytes(head[4..8].try_into().ok()?) as usize;
    let stream_len = u32::from_be_bytes(head[8..12].try_into().ok()?) as usize;
    if decrunched == 0
        || decrunched > MAX_UNPACKED
        || stream_len < 6
        || o.checked_add(HEADER_LEN + stream_len)
            .is_none_or(|end| end > bytes.len())
    {
        return None;
    }
    Some(AtomikHeader {
        decrunched,
        stream_len,
    })
}

/// All plausible blobs, in ascending offset order — one memmem pass, NO
/// alignment assumption (packed data inside files is arbitrary-offset).
fn candidates(bytes: &[u8]) -> Vec<(usize, AtomikHeader)> {
    memchr::memmem::find_iter(bytes, b"ATM5")
        .filter_map(|o| parse_header(bytes, o).map(|h| (o, h)))
        .collect()
}

pub struct Atomik;

impl Depacker for Atomik {
    fn id(&self) -> PluginId {
        PluginId::new("atomik")
    }
    fn describe(&self) -> &'static str {
        "Atomik 3.5 (ATM5) depacker — unpacks the first valid blob found"
    }
    /// Same whole-buffer contract as Pack-Ice (ADR-14): first candidate
    /// whose stream decodes cleanly wins.
    fn try_unpack(&self, data: &[u8]) -> Option<Vec<u8>> {
        candidates(data).into_iter().find_map(|(o, h)| {
            decrunch(
                &data[o + HEADER_LEN..o + HEADER_LEN + h.stream_len],
                h.decrunched,
            )
        })
    }
}

pub struct AtomikRegion;

impl RipPlugin for AtomikRegion {
    fn id(&self) -> PluginId {
        PluginId::new("atomik-region")
    }
    fn kind(&self) -> RipKind {
        RipKind::Packer
    }
    fn describe(&self) -> &'static str {
        "Atomik 3.5 (ATM5) packed regions — header report only, no depack"
    }
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut next = 0usize;
        for (o, h) in candidates(mem.bytes()) {
            if o < next {
                continue; // magic inside a region already reported
            }
            let len = HEADER_LEN + h.stream_len;
            next = o + len;
            out.push(Finding::new(
                self.id(),
                RipKind::Packer,
                Offset::new(o),
                len,
                format!(
                    "Atomik packed region ('ATM5'), {len} packed -> {} unpacked bytes",
                    h.decrunched
                ),
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Full blobs (header + stream), P9 §6.1.2.
    const A1: &[u8] = &[
        0x41, 0x54, 0x4D, 0x35, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x07, 0x41, 0x42, 0x02,
        0x00, 0x00, 0x08, 0x00,
    ];
    const A2: &[u8] = &[
        0x41, 0x54, 0x4D, 0x35, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x08, 0x00, 0x80, 0x41,
        0x06, 0x00, 0x03, 0x11, 0x00,
    ];

    fn plant(buf: &mut [u8], at: usize, data: &[u8]) {
        buf[at..at + data.len()].copy_from_slice(data);
    }

    #[test]
    fn parse_header_ok_and_rejects() {
        let h = parse_header(A1, 0).unwrap();
        assert_eq!(h.decrunched, 2);
        assert_eq!(h.stream_len, 7);

        assert!(parse_header(b"ATX5\x00\x00\x00\x02\x00\x00\x00\x07", 0).is_none());
        assert!(parse_header(&A1[..11], 0).is_none());
        let mut zero = A1.to_vec();
        zero[4..8].copy_from_slice(&0u32.to_be_bytes());
        assert!(parse_header(&zero, 0).is_none());
        let mut huge = A1.to_vec();
        huge[4..8].copy_from_slice(&(MAX_UNPACKED as u32 + 1).to_be_bytes());
        assert!(parse_header(&huge, 0).is_none());
        let mut short_stream = A1.to_vec();
        short_stream[8..12].copy_from_slice(&5u32.to_be_bytes());
        assert!(parse_header(&short_stream, 0).is_none());
        let mut overrun = A1.to_vec();
        overrun[8..12].copy_from_slice(&1000u32.to_be_bytes());
        assert!(parse_header(&overrun, 0).is_none());
    }

    #[test]
    fn try_unpack_at_odd_offset() {
        let mut buf = vec![0u8; 4096];
        plant(&mut buf, 0x101, A1);
        assert_eq!(Atomik.try_unpack(&buf), Some(b"AB".to_vec()));
    }

    #[test]
    fn try_unpack_skips_corrupt_finds_valid() {
        let mut buf = vec![0u8; 4096];
        let mut corrupt = A2.to_vec();
        *corrupt.last_mut().unwrap() = 0x01; // pic byte != 0 -> refused
        plant(&mut buf, 0x40, &corrupt);
        plant(&mut buf, 0x800, A1);
        assert_eq!(Atomik.try_unpack(&buf), Some(b"AB".to_vec()));
    }

    #[test]
    fn detector_reports_and_skips_interior_magic() {
        let mut overlap = vec![0u8; 4096];
        plant(&mut overlap, 0x20, A2);
        plant(&mut overlap, 0x2C, A1); // right after A2's 12-byte header
        let mem = MemImage::new(overlap, crate::types::Addr::new(0));
        let findings = AtomikRegion.scan(&mem);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].offset, Offset::new(0x20));

        let mut disjoint = vec![0u8; 4096];
        plant(&mut disjoint, 0x20, A2);
        plant(&mut disjoint, 0x40, A1);
        let mem = MemImage::new(disjoint, crate::types::Addr::new(0));
        let findings = AtomikRegion.scan(&mem);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].len, 20);
        assert_eq!(findings[1].len, 19);
    }

    #[test]
    fn detector_zero_ram_is_empty() {
        let zeros = MemImage::new(vec![0u8; 64 * 1024], crate::types::Addr::new(0));
        assert!(AtomikRegion.scan(&zeros).is_empty());
        let teaser: Vec<u8> = std::iter::repeat_n([0x41u8, 0x54], 32 * 1024)
            .flatten()
            .collect();
        let mem = MemImage::new(teaser, crate::types::Addr::new(0));
        assert!(AtomikRegion.scan(&mem).is_empty());
    }
}
