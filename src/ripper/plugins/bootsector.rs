//! Boot-sector detector (§4, ADR-17): the `$1234` big-endian executability
//! checksum, the little-endian DOS BPB, and a plain/loader/suspicious
//! classification. Virus tells live in `boot_heur` (line budget). Works on
//! a 512-byte disk sector (offset 0) or the TOS disk buffer inside a
//! whole-RAM snapshot (`_dskbufp` at `$4C6`).

use crate::ripper::plugins::boot_bpb;
use crate::ripper::plugins::boot_heur::{self, Tell, TellKind};
use crate::ripper::{Finding, RipKind, RipPlugin};
use crate::types::{Addr, MemImage, Offset, PluginId};

pub use boot_bpb::{checksum, Bpb, BOOT_MAGIC_SUM};

pub const SECTOR_LEN: usize = 512;
/// `_dskbufp`: BE long at `$4C6`, the TOS disk buffer address. // verify live.
pub const DSKBUFP: usize = 0x4C6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootClass {
    PlainData,
    Loader,
    Suspicious,
}

/// Full sector analysis — recomputed from finding bytes (§8), never scraped
/// from `label`.
pub struct BootReport {
    pub checksum: u16,
    pub executable: bool,
    pub bpb: Bpb,
    pub bpb_plausible: bool,
    pub class: BootClass,
    pub tells: Vec<Tell>,
    pub confidence: f32,
}

/// §4.4: a lone `DiskTrap` never forces `Suspicious` — legit loaders touch disk too.
fn classify(executable: bool, tells: &[Tell]) -> BootClass {
    if !executable {
        return BootClass::PlainData;
    }
    let strong = tells
        .iter()
        .any(|t| matches!(t.kind, TellKind::VectorWrite | TellKind::SelfCopy));
    if strong {
        BootClass::Suspicious
    } else {
        BootClass::Loader
    }
}

/// Analyze `mem[off..off+512]`. Never errors: garbage yields checksum +
/// implausible BPB + `PlainData` + no tells.
pub fn analyze_sector(mem: &MemImage, off: usize) -> BootReport {
    let s = &mem.bytes()[off..off + SECTOR_LEN];
    let checksum = checksum(s);
    let executable = checksum == BOOT_MAGIC_SUM;
    let bpb = boot_bpb::decode_bpb(s);
    let bpb_plausible = boot_bpb::bpb_plausible(&bpb);
    let (tells, confidence) = boot_heur::analyze_code(mem, off + boot_heur::boot_code_start(s));
    let class = classify(executable, &tells);
    BootReport {
        checksum,
        executable,
        bpb,
        bpb_plausible,
        class,
        tells,
        confidence,
    }
}

pub struct BootSector;
impl RipPlugin for BootSector {
    fn id(&self) -> PluginId {
        PluginId::new("bootsector")
    }
    fn kind(&self) -> RipKind {
        RipKind::Boot
    }
    fn describe(&self) -> &'static str {
        "Atari ST boot sectors ($1234 checksum + DOS BPB + loader/virus heuristics)"
    }
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        candidate_offset(mem)
            .into_iter()
            .filter(|&(off, always)| always || worth_reporting(mem, off))
            .map(|(off, _)| {
                let label = label(&analyze_sector(mem, off));
                Finding::new(
                    self.id(),
                    RipKind::Boot,
                    Offset::new(off),
                    SECTOR_LEN,
                    label,
                )
            })
            .collect()
    }
}

/// Candidate offset + "report unconditionally" flag (§6.1): a disk-sized
/// buffer always reports; RAM only dereferences `_dskbufp` when plausible
/// (below `$800` is the TOS vector area, never a real disk buffer).
/// // verify live: _dskbufp persistence of the sector post-boot is unverified.
fn candidate_offset(mem: &MemImage) -> Option<(usize, bool)> {
    if mem.len() == SECTOR_LEN {
        return Some((0, true));
    }
    if mem.base() != Addr::new(0) || mem.len() < 0x4CA {
        return None;
    }
    let off = be32(mem.bytes(), DSKBUFP)? as usize;
    (off + SECTOR_LEN <= mem.len() && off >= 0x800).then_some((off, false))
}

fn be32(bytes: &[u8], o: usize) -> Option<u32> {
    let b = bytes.get(o..o + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Suppresses zeroed/junk `_dskbufp` windows in a normal RAM `rip`.
fn worth_reporting(mem: &MemImage, off: usize) -> bool {
    let r = analyze_sector(mem, off);
    r.executable || r.bpb_plausible
}

/// Human label; structured facts are recomputed by the tool layer — never
/// parse this string.
fn label(r: &BootReport) -> String {
    let exec = if r.executable {
        "executable"
    } else {
        "not executable"
    };
    format!(
        "Boot sector: {} (checksum=${:04X}, {exec}; {} tells)",
        class_str(r.class),
        r.checksum,
        r.tells.len()
    )
}

fn class_str(c: BootClass) -> &'static str {
    match c {
        BootClass::PlainData => "plain-data",
        BootClass::Loader => "loader",
        BootClass::Suspicious => "suspicious",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(s: &mut [u8], off: usize, bytes: &[u8]) {
        s[off..off + bytes.len()].copy_from_slice(bytes);
    }

    fn set_checksum(s: &mut [u8]) {
        let mut sum = 0u16;
        for i in 0..255 {
            sum = sum.wrapping_add(u16::from_be_bytes([s[2 * i], s[2 * i + 1]]));
        }
        let adj = BOOT_MAGIC_SUM.wrapping_sub(sum);
        put(s, 0x1FE, &adj.to_be_bytes());
        assert_eq!(checksum(s), BOOT_MAGIC_SUM);
    }

    fn plain_fat_sector() -> Vec<u8> {
        let mut s = vec![0u8; SECTOR_LEN];
        put(&mut s, 0x00B, &512u16.to_le_bytes());
        s[0x00D] = 2;
        put(&mut s, 0x00E, &1u16.to_le_bytes());
        s[0x010] = 2;
        put(&mut s, 0x011, &112u16.to_le_bytes());
        put(&mut s, 0x013, &720u16.to_le_bytes());
        s[0x015] = 0xF9;
        put(&mut s, 0x016, &5u16.to_le_bytes());
        put(&mut s, 0x018, &9u16.to_le_bytes());
        put(&mut s, 0x01A, &2u16.to_le_bytes());
        put(&mut s, 0x01C, &0u16.to_le_bytes());
        s
    }

    fn bootable_loader_sector() -> Vec<u8> {
        let mut s = plain_fat_sector();
        put(&mut s, 0x000, &[0x60, 0x1C]); // bra.s $1E
        put(&mut s, 0x01E, &[0x4E, 0x71]); // nop
        put(&mut s, 0x020, &[0x4E, 0x75]); // rts
        set_checksum(&mut s);
        s
    }

    fn virus_like_sector() -> Vec<u8> {
        let mut s = plain_fat_sector();
        put(&mut s, 0x000, &[0x60, 0x1C]); // bra.s $1E
        put(&mut s, 0x01E, &[0x21, 0xC8, 0x04, 0x72]); // move.l a0,$472.w
        put(&mut s, 0x022, &[0x21, 0xC9, 0x04, 0x2A]); // move.l a1,$42A.w
        put(&mut s, 0x026, &[0x22, 0xD8]); // move.l (a0)+,(a1)+
        put(&mut s, 0x028, &[0x51, 0xC8, 0xFF, 0xFC]); // dbra d0,$26
        put(&mut s, 0x02C, &[0x4E, 0x75]); // rts
        set_checksum(&mut s);
        s
    }

    #[test]
    fn checksum_hits_1234_when_adjusted() {
        let mut s = vec![0u8; SECTOR_LEN];
        set_checksum(&mut s);
        let mem = MemImage::new(s, Addr::new(0));
        assert!(analyze_sector(&mem, 0).executable);
    }

    #[test]
    fn checksum_false_after_flip() {
        let mut s = vec![0u8; SECTOR_LEN];
        set_checksum(&mut s);
        s[0x100] ^= 0xFF;
        let mem = MemImage::new(s, Addr::new(0));
        let r = analyze_sector(&mem, 0);
        assert!(!r.executable);
        assert_eq!(r.class, BootClass::PlainData);
    }

    #[test]
    fn plain_fat_bpb_decodes() {
        let mem = MemImage::new(plain_fat_sector(), Addr::new(0));
        let r = analyze_sector(&mem, 0);
        assert_eq!(r.bpb.bps, 512);
        assert_eq!(r.bpb.spc, 2);
        assert_eq!(r.bpb.res, 1);
        assert_eq!(r.bpb.nfats, 2);
        assert_eq!(r.bpb.ndirs, 112);
        assert_eq!(r.bpb.nsects, 720);
        assert_eq!(r.bpb.media, 0xF9);
        assert_eq!(r.bpb.spf, 5);
        assert_eq!(r.bpb.spt, 9);
        assert_eq!(r.bpb.nsides, 2);
        assert_eq!(r.bpb.nhid, 0);
        assert!(r.bpb_plausible);
    }

    #[test]
    fn zero_sector_implausible() {
        let mem = MemImage::new(vec![0u8; SECTOR_LEN], Addr::new(0));
        let r = analyze_sector(&mem, 0);
        assert_eq!(r.checksum, 0);
        assert!(!r.executable);
        assert!(!r.bpb_plausible);
        assert_eq!(r.class, BootClass::PlainData);
        assert!(r.tells.is_empty());
    }

    #[test]
    fn class_loader_when_executable_no_strong_tell() {
        let mem = MemImage::new(bootable_loader_sector(), Addr::new(0));
        let r = analyze_sector(&mem, 0);
        assert!(r.executable);
        assert_eq!(r.class, BootClass::Loader);
        assert!(r
            .tells
            .iter()
            .all(|t| !matches!(t.kind, TellKind::VectorWrite | TellKind::SelfCopy)));
    }

    #[test]
    fn class_suspicious_on_vector_write() {
        let mem = MemImage::new(virus_like_sector(), Addr::new(0));
        let r = analyze_sector(&mem, 0);
        assert!(r.executable);
        assert_eq!(r.class, BootClass::Suspicious);
        assert!(r.tells.iter().any(|t| t.kind == TellKind::VectorWrite));
    }

    #[test]
    fn candidate_disk_mode_always_reports() {
        let mem = MemImage::new(vec![0u8; SECTOR_LEN], Addr::new(0));
        let findings = BootSector.scan(&mem);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].offset, Offset::new(0));
        assert_eq!(findings[0].len, SECTOR_LEN);
    }

    #[test]
    fn candidate_ram_derefs_dskbufp() {
        let mut buf = vec![0u8; 65536];
        put(&mut buf, DSKBUFP, &0x0000_1000u32.to_be_bytes());
        put(&mut buf, 0x1000, &bootable_loader_sector());
        let mem = MemImage::new(buf, Addr::new(0));
        let findings = BootSector.scan(&mem);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].offset, Offset::new(0x1000));
        assert_eq!(findings[0].len, SECTOR_LEN);
    }

    #[test]
    fn candidate_ram_zero_dskbufp_none() {
        let mem = MemImage::new(vec![0u8; 65536], Addr::new(0));
        assert!(BootSector.scan(&mem).is_empty());
    }

    #[test]
    fn candidate_ram_below_800_rejected() {
        let mut buf = vec![0u8; 65536];
        put(&mut buf, DSKBUFP, &0x0000_0400u32.to_be_bytes());
        let mem = MemImage::new(buf, Addr::new(0));
        assert!(BootSector.scan(&mem).is_empty());
    }
}
