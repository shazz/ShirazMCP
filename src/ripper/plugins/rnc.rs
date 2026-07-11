//! RNC ProPack (`RNC\x01`/`RNC\x02`) plugins: the `rnc` Depacker and the
//! `rnc-region` Packer-kind detector (ADR-19). Header layout: P9 §4.C.1 —
//! the standard 18-byte header only; the OLD 12-byte-header `RNC\x01`
//! layout (`RNC_1_1.S`) is a different, incompatible scheme sharing the
//! same magic, and is rejected deterministically by the packed-payload
//! CRC-16 gate below (its bytes at +12.. are stream data, not a CRC), so
//! it never yields a candidate — deferred, see ADR-19. Method decoders
//! live in `rnc_m1`/`rnc_m2` (line budget). The XOR key and in-place
//! leeway machinery are dropped (§4.C.5/4.C.7).

use super::{rnc_m1, rnc_m2};
use crate::ripper::registry::MAX_UNPACKED;
use crate::ripper::{Depacker, Finding, RipKind, RipPlugin};
use crate::types::{MemImage, Offset, PluginId};

const HEADER_LEN: usize = 18;

/// CRC-16/ARC (§4.C.1): init 0, reflected, poly 0xA001.
/// Pinned by `crc16(b"123456789") == 0xBB3D`.
pub(crate) fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= u16::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

struct RncHeader {
    method: u8,
    unpacked: usize,
    packed: usize,
    crc_unp: u16,
}

/// §4.C.1 gates INCLUDING the packed-payload CRC check — near-free, kills
/// false positives dead and deterministically rejects the old 12-byte
/// layout.
fn parse_header(bytes: &[u8], o: usize) -> Option<RncHeader> {
    let head = bytes.get(o..o + HEADER_LEN)?;
    if &head[..3] != b"RNC" {
        return None;
    }
    let method = head[3];
    if method != 1 && method != 2 {
        return None;
    }
    let unpacked = u32::from_be_bytes(head[4..8].try_into().ok()?) as usize;
    let packed = u32::from_be_bytes(head[8..12].try_into().ok()?) as usize;
    let crc_unp = u16::from_be_bytes(head[12..14].try_into().ok()?);
    let crc_pack = u16::from_be_bytes(head[14..16].try_into().ok()?);
    if unpacked == 0 || unpacked > MAX_UNPACKED || packed < 2 {
        return None;
    }
    let end = o.checked_add(HEADER_LEN + packed)?;
    if end > bytes.len() {
        return None;
    }
    if crc16(&bytes[o + HEADER_LEN..end]) != crc_pack {
        return None;
    }
    Some(RncHeader {
        method,
        unpacked,
        packed,
        crc_unp,
    })
}

/// Two memmem passes (both magics), merged ascending — the
/// `pack_ice::candidates` shape verbatim.
fn candidates(bytes: &[u8]) -> Vec<(usize, RncHeader)> {
    let mut offsets: Vec<usize> = memchr::memmem::find_iter(bytes, b"RNC\x01")
        .chain(memchr::memmem::find_iter(bytes, b"RNC\x02"))
        .collect();
    offsets.sort_unstable();
    offsets
        .into_iter()
        .filter_map(|o| parse_header(bytes, o).map(|h| (o, h)))
        .collect()
}

pub struct Rnc;

impl Depacker for Rnc {
    fn id(&self) -> PluginId {
        PluginId::new("rnc")
    }
    fn describe(&self) -> &'static str {
        "RNC ProPack (RNC\\x01/RNC\\x02) depacker — CRC-16 gated, methods 1 and 2"
    }
    fn try_unpack(&self, data: &[u8]) -> Option<Vec<u8>> {
        candidates(data).into_iter().find_map(|(o, h)| {
            let payload = &data[o + HEADER_LEN..o + HEADER_LEN + h.packed];
            let out = match h.method {
                1 => rnc_m1::unpack(payload, h.unpacked),
                2 => rnc_m2::unpack(payload, h.unpacked),
                _ => None,
            }?;
            (out.len() == h.unpacked && crc16(&out) == h.crc_unp).then_some(out)
        })
    }
}

pub struct RncRegion;

impl RipPlugin for RncRegion {
    fn id(&self) -> PluginId {
        PluginId::new("rnc-region")
    }
    fn kind(&self) -> RipKind {
        RipKind::Packer
    }
    fn describe(&self) -> &'static str {
        "RNC ProPack packed regions — header + packed CRC verified, no depack"
    }
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut next = 0usize;
        for (o, h) in candidates(mem.bytes()) {
            if o < next {
                continue; // magic inside a region already reported
            }
            let len = HEADER_LEN + h.packed;
            next = o + len;
            out.push(Finding::new(
                self.id(),
                RipKind::Packer,
                Offset::new(o),
                len,
                format!(
                    "RNC ProPack method {} packed region, {len} packed -> {} unpacked bytes (packed CRC ok)",
                    h.method, h.unpacked
                ),
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // M1/M2 streams + CRCs (P9 §6.3.2).
    const M1_STREAM: &[u8] = &[
        0x88, 0x88, 0x10, 0x03, 0x20, 0x04, 0x00, 0x12, 0x41, 0x00, 0x00,
    ];
    const M2_STREAM: &[u8] = &[0x07, 0x41, 0x42, 0x43, 0x3C, 0x02, 0x00];

    fn wrap(method: u8, stream: &[u8], unpacked: u32, crc_unp: u16, crc_pack: u16) -> Vec<u8> {
        let mut out = b"RNC".to_vec();
        out.push(method);
        out.extend(unpacked.to_be_bytes());
        out.extend((stream.len() as u32).to_be_bytes());
        out.extend(crc_unp.to_be_bytes());
        out.extend(crc_pack.to_be_bytes());
        out.extend([0x00, 0x01]);
        out.extend_from_slice(stream);
        out
    }

    fn m1_blob() -> Vec<u8> {
        wrap(1, M1_STREAM, 6, 0x5687, 0x5FFD)
    }
    fn m2_blob() -> Vec<u8> {
        wrap(2, M2_STREAM, 6, 0xDF43, 0x36D5)
    }

    fn plant(buf: &mut [u8], at: usize, data: &[u8]) {
        buf[at..at + data.len()].copy_from_slice(data);
    }

    #[test]
    fn crc16_arc_check_value() {
        assert_eq!(crc16(b"123456789"), 0xBB3D);
    }

    #[test]
    fn parse_header_gates() {
        let good = m1_blob();
        assert!(parse_header(&good, 0).is_some());

        let mut bad_magic = good.clone();
        bad_magic[0] = b'X';
        assert!(parse_header(&bad_magic, 0).is_none());

        let mut bad_method = good.clone();
        bad_method[3] = 0x03;
        assert!(parse_header(&bad_method, 0).is_none());

        let mut zero_unpacked = good.clone();
        zero_unpacked[4..8].copy_from_slice(&0u32.to_be_bytes());
        assert!(parse_header(&zero_unpacked, 0).is_none());

        let mut huge_unpacked = good.clone();
        huge_unpacked[4..8].copy_from_slice(&(MAX_UNPACKED as u32 + 1).to_be_bytes());
        assert!(parse_header(&huge_unpacked, 0).is_none());

        let mut tiny_packed = good.clone();
        tiny_packed[8..12].copy_from_slice(&1u32.to_be_bytes());
        assert!(parse_header(&tiny_packed, 0).is_none());

        let mut overrun = good.clone();
        overrun[8..12].copy_from_slice(&1000u32.to_be_bytes());
        assert!(parse_header(&overrun, 0).is_none());

        let mut flipped_payload = good.clone();
        let last = flipped_payload.len() - 1;
        flipped_payload[last] ^= 0xFF;
        assert!(parse_header(&flipped_payload, 0).is_none());
    }

    #[test]
    fn try_unpack_at_odd_offset() {
        let mut buf = vec![0u8; 4096];
        plant(&mut buf, 0x101, &m1_blob());
        assert_eq!(Rnc.try_unpack(&buf), Some(b"AAAAAA".to_vec()));

        let mut buf2 = vec![0u8; 4096];
        plant(&mut buf2, 0x101, &m2_blob());
        assert_eq!(Rnc.try_unpack(&buf2), Some(b"ABCABC".to_vec()));
    }

    #[test]
    fn detector_zero_ram_is_empty() {
        let zeros = MemImage::new(vec![0u8; 64 * 1024], crate::types::Addr::new(0));
        assert!(RncRegion.scan(&zeros).is_empty());
        let teaser: Vec<u8> = std::iter::repeat_n([b'R', b'N'], 32 * 1024)
            .flatten()
            .collect();
        let mem = MemImage::new(teaser, crate::types::Addr::new(0));
        assert!(RncRegion.scan(&mem).is_empty());
    }

    #[test]
    fn detector_reports_valid_region() {
        let mut buf = vec![0u8; 4096];
        plant(&mut buf, 0x20, &m1_blob());
        let mem = MemImage::new(buf, crate::types::Addr::new(0));
        let findings = RncRegion.scan(&mem);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].offset, Offset::new(0x20));
        assert_eq!(findings[0].len, HEADER_LEN + M1_STREAM.len());
        assert!(findings[0].label.contains("method 1"));
    }
}
