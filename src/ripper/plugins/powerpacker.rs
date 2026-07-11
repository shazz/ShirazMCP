//! PowerPacker (`PP20`) plugins: the `powerpacker` Depacker and the
//! `powerpacker-region` Packer-kind detector (ADR-19). Header layout: P9
//! §4.B.1. PP20 carries no packed-length field, so a blob's end is
//! *derived* (§4.B.4) — see `derived_ends`. Decoding is in `pp_decode`
//! (line budget). Scope: `PP20` only — `PX20` (XOR-encrypted) and `PPMT`
//! are not recognized (ADR-19).

use super::pp_decode::decrunch;
use crate::ripper::registry::MAX_UNPACKED;
use crate::ripper::{Depacker, Finding, RipKind, RipPlugin};
use crate::types::{MemImage, Offset, PluginId};

const HEADER_LEN: usize = 8; // magic + efficiency table; trailer is at the end

struct PpCandidate {
    end: usize,
    out_len: usize,
    skip: u32,
}

/// §4.B.1 magic + efficiency-table gates (each byte in `4..=14`, killing
/// ASCII "PP20 ..." false positives hard).
fn efficiency(bytes: &[u8], o: usize) -> Option<[u8; 4]> {
    let head = bytes.get(o..o + HEADER_LEN)?;
    if &head[..4] != b"PP20" {
        return None;
    }
    let mut eff = [0u8; 4];
    for (slot, b) in eff.iter_mut().zip(&head[4..8]) {
        if !(4..=14).contains(b) {
            return None;
        }
        *slot = *b;
    }
    Some(eff)
}

/// §4.B.1 trailer gates at a candidate `end`.
fn trailer(bytes: &[u8], o: usize, end: usize) -> Option<(usize, u32)> {
    if end < o + 16 || !(end - o).is_multiple_of(4) || end > bytes.len() {
        return None;
    }
    let t = u32::from_be_bytes(bytes[end - 4..end].try_into().ok()?);
    let out_len = (t >> 8) as usize;
    let skip = t & 0xFF;
    if skip >= 32 || out_len == 0 || out_len > MAX_UNPACKED {
        return None;
    }
    Some((out_len, skip))
}

/// §4.B.4: the two candidate blob ends, zero-trim first, then buffer end,
/// deduplicated.
fn derived_ends(bytes: &[u8], o: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    if let Some(z) = bytes[o..].iter().rposition(|&b| b != 0).map(|i| o + i + 1) {
        if let Some(e1) = (z..z + 4).find(|e| (e - o).is_multiple_of(4) && *e <= bytes.len()) {
            ends.push(e1);
        }
    }
    let e2 = o + (bytes.len() - o) / 4 * 4;
    if !ends.contains(&e2) {
        ends.push(e2);
    }
    ends
}

/// All plausible magics with their sane-trailer end candidates, ascending.
fn candidates(bytes: &[u8]) -> Vec<(usize, [u8; 4], Vec<PpCandidate>)> {
    memchr::memmem::find_iter(bytes, b"PP20")
        .filter_map(|o| {
            let eff = efficiency(bytes, o)?;
            let cands: Vec<PpCandidate> = derived_ends(bytes, o)
                .into_iter()
                .filter_map(|end| {
                    trailer(bytes, o, end).map(|(out_len, skip)| PpCandidate { end, out_len, skip })
                })
                .collect();
            if cands.is_empty() {
                None
            } else {
                Some((o, eff, cands))
            }
        })
        .collect()
}

pub struct PowerPacker;

impl Depacker for PowerPacker {
    fn id(&self) -> PluginId {
        PluginId::new("powerpacker")
    }
    fn describe(&self) -> &'static str {
        "PowerPacker (PP20) depacker — unpacks the first valid blob found (blob end derived; see ADR-19)"
    }
    fn try_unpack(&self, data: &[u8]) -> Option<Vec<u8>> {
        candidates(data).into_iter().find_map(|(o, eff, cands)| {
            cands
                .into_iter()
                .find_map(|c| decrunch(&data[o + HEADER_LEN..c.end - 4], &eff, c.skip, c.out_len))
        })
    }
}

pub struct PowerPackerRegion;

impl RipPlugin for PowerPackerRegion {
    fn id(&self) -> PluginId {
        PluginId::new("powerpacker-region")
    }
    fn kind(&self) -> RipKind {
        RipKind::Packer
    }
    fn describe(&self) -> &'static str {
        "PowerPacker (PP20) packed regions — header + derived end, no depack"
    }
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut next = 0usize;
        for (o, _eff, cands) in candidates(mem.bytes()) {
            if o < next {
                continue; // magic inside a region already reported
            }
            let Some(c) = cands.first() else { continue };
            let len = c.end - o;
            next = o + len;
            out.push(Finding::new(
                self.id(),
                RipKind::Packer,
                Offset::new(o),
                len,
                format!(
                    "PowerPacker packed region ('PP20'), {len} packed (end estimated) -> {} unpacked bytes",
                    c.out_len
                ),
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // P1 (P9 §6.2.2): "AAAAAA".
    const P1: &[u8] = &[
        0x50, 0x50, 0x32, 0x30, 0x09, 0x0A, 0x0C, 0x0D, 0x00, 0x1C, 0x10, 0x00, 0x00, 0x00, 0x06,
        0x08,
    ];

    fn plant(buf: &mut [u8], at: usize, data: &[u8]) {
        buf[at..at + data.len()].copy_from_slice(data);
    }

    #[test]
    fn efficiency_gate() {
        let mut ok = b"PP20".to_vec();
        ok.extend([9, 10, 12, 13]);
        assert_eq!(efficiency(&ok, 0), Some([9, 10, 12, 13]));

        let mut too_low = b"PP20".to_vec();
        too_low.extend([3, 10, 12, 13]);
        assert!(efficiency(&too_low, 0).is_none());

        let mut too_high = b"PP20".to_vec();
        too_high.extend([15, 10, 12, 13]);
        assert!(efficiency(&too_high, 0).is_none());

        assert!(efficiency(b"PP20 is a packer!", 0).is_none());
    }

    #[test]
    fn trailer_gates() {
        let with_trailer = |t: u32| {
            let mut b = vec![0u8; 16];
            b[..4].copy_from_slice(b"PP20");
            b[12..16].copy_from_slice(&t.to_be_bytes());
            b
        };
        assert!(trailer(&with_trailer((6 << 8) | 32), 0, 16).is_none()); // skip == 32
        assert!(trailer(&with_trailer(8), 0, 16).is_none()); // out_len == 0
                                                             // out_len is the top 24 bits of `t` (max 0xFF_FFFF, one less than
                                                             // MAX_UNPACKED) — the >MAX_UNPACKED gate can never fire from a
                                                             // real trailer word; it stays as defense-in-depth (§4.B.1).
        assert!(trailer(&with_trailer((6 << 8) | 8), 0, 12).is_none()); // end - o < 16

        let mut misaligned = vec![0u8; 20];
        misaligned[..4].copy_from_slice(b"PP20");
        misaligned[13..17].copy_from_slice(&((6u32 << 8) | 8).to_be_bytes());
        assert!(trailer(&misaligned, 0, 17).is_none()); // (17 - 0) % 4 != 0
    }

    #[test]
    fn derived_ends_order() {
        // Whole-file case: the trailer's non-zero bytes ARE the buffer's
        // last bytes, so e1 == e2 and dedup collapses to one entry.
        let mut whole_file = vec![0u8; 16];
        whole_file[..4].copy_from_slice(b"PP20");
        whole_file[12..16].copy_from_slice(&((6u32 << 8) | 8).to_be_bytes());
        assert_eq!(derived_ends(&whole_file, 0), vec![16]);

        // 4 trailing zero bytes beyond the real (zero-trimmed) end: e1
        // comes before the larger e2.
        let mut trailing_zeros = vec![0u8; 20];
        trailing_zeros[..4].copy_from_slice(b"PP20");
        trailing_zeros[12..16].copy_from_slice(&1u32.to_be_bytes());
        assert_eq!(derived_ends(&trailing_zeros, 0), vec![16, 20]);
    }

    #[test]
    fn try_unpack_at_odd_offset() {
        let mut buf = vec![0u8; 4096];
        plant(&mut buf, 0x101, P1);
        assert_eq!(PowerPacker.try_unpack(&buf), Some(b"AAAAAA".to_vec()));
    }

    #[test]
    fn detector_skips_interior_magic() {
        // A second magic planted inside the first's 16-byte region is
        // skipped by the cursor. (PP20's end-derivation scans the REST of
        // the buffer for zero-trimming, so — unlike Pack-Ice/Atomik — two
        // genuinely disjoint PP20 blobs in one buffer are not both
        // reliably found; that documented limitation is ADR-19's, proven
        // by the integration test's end-derivation cases, not here.)
        let mut overlap = vec![0u8; 4096];
        plant(&mut overlap, 0x20, P1);
        plant(&mut overlap, 0x24, P1);
        let mem = MemImage::new(overlap, crate::types::Addr::new(0));
        assert_eq!(PowerPackerRegion.scan(&mem).len(), 1);
    }

    #[test]
    fn detector_zero_ram_is_empty() {
        let zeros = MemImage::new(vec![0u8; 64 * 1024], crate::types::Addr::new(0));
        assert!(PowerPackerRegion.scan(&zeros).is_empty());
    }
}
