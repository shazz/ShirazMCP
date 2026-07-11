//! Boot-sector detector integration (P7.md §10). No real virus disks are
//! committed — byte-pinned builders, exactly like the P4 graphics, P5
//! depack, and P6 music fixtures (deterministic, reviewable, no binary
//! blobs). The "virus-like" sector is a few hand-written instructions that
//! *touch* watched vectors, not real virus code — malware-free.

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "test code, not library code")]

use std::sync::Arc;

use shiraz_mcp::ripper::plugins::boot_heur::TellKind;
use shiraz_mcp::ripper::plugins::bootsector::{self, BOOT_MAGIC_SUM, SECTOR_LEN};
use shiraz_mcp::ripper::{default_registry, extract, ExtractFormat, RipKind};
use shiraz_mcp::types::{Addr, MemImage};

/// Write bytes at `off` into a sector buffer.
fn put(s: &mut [u8], off: usize, bytes: &[u8]) {
    s[off..off + bytes.len()].copy_from_slice(bytes);
}

/// Tune word $1FE so the big-endian word-sum becomes exactly $1234.
fn set_checksum(s: &mut [u8]) {
    let mut sum = 0u16;
    for i in 0..255 {
        sum = sum.wrapping_add(u16::from_be_bytes([s[2 * i], s[2 * i + 1]]));
    }
    let adj = BOOT_MAGIC_SUM.wrapping_sub(sum);
    put(s, 0x1FE, &adj.to_be_bytes());
    assert_eq!(bootsector::checksum(s), BOOT_MAGIC_SUM);
}

/// A standard-geometry DOS BPB (little-endian), no boot code, NOT executable.
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

/// Executable loader: bra.s to $1E, benign code (nop, rts), checksum tuned.
fn bootable_loader_sector() -> Vec<u8> {
    let mut s = plain_fat_sector();
    put(&mut s, 0x000, &[0x60, 0x1C]); // bra.s $1E
    put(&mut s, 0x01E, &[0x4E, 0x71]); // nop (stand-in for loader body)
    put(&mut s, 0x020, &[0x4E, 0x75]); // rts
    set_checksum(&mut s);
    s
}

/// Executable + writes hdv_bpb ($472) and resvector ($42A) + a self-copy loop.
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
fn disk_mode_plain_fat() {
    let mem = Arc::new(MemImage::new(plain_fat_sector(), Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Boot), 0);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].plugin.as_str(), "bootsector");
    assert_eq!(findings[0].offset.raw(), 0);
    assert_eq!(findings[0].len, SECTOR_LEN);

    let mem = MemImage::new(plain_fat_sector(), Addr::new(0));
    let r = bootsector::analyze_sector(&mem, 0);
    assert!(!r.executable);
    assert_eq!(r.class, bootsector::BootClass::PlainData);
    assert_eq!(r.bpb.bps, 512);
}

#[test]
fn disk_mode_loader() {
    let mem = Arc::new(MemImage::new(bootable_loader_sector(), Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Boot), 0);
    assert_eq!(findings.len(), 1);

    let mem = MemImage::new(bootable_loader_sector(), Addr::new(0));
    let r = bootsector::analyze_sector(&mem, 0);
    assert!(r.executable);
    assert_eq!(r.class, bootsector::BootClass::Loader);
}

#[test]
fn disk_mode_suspicious() {
    let mem = Arc::new(MemImage::new(virus_like_sector(), Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Boot), 0);
    assert_eq!(findings.len(), 1);

    let mem = MemImage::new(virus_like_sector(), Addr::new(0));
    let r = bootsector::analyze_sector(&mem, 0);
    assert!(r.executable);
    assert_eq!(r.class, bootsector::BootClass::Suspicious);
    assert!(r.tells.iter().any(|t| t.kind == TellKind::VectorWrite));
    assert!(r.tells.iter().any(|t| t.kind == TellKind::SelfCopy));
}

#[test]
fn ram_mode_dskbufp() {
    let mut buf = vec![0u8; 65536];
    put(&mut buf, bootsector::DSKBUFP, &0x0000_1000u32.to_be_bytes());
    put(&mut buf, 0x1000, &bootable_loader_sector());
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Boot), 0);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].offset.raw(), 0x1000);
    assert_eq!(findings[0].len, SECTOR_LEN);
    assert!(findings[0].label.contains("executable"));
}

#[test]
fn ram_mode_clean_not_found() {
    let mem = Arc::new(MemImage::new(vec![0u8; 65536], Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Boot), 0);
    assert!(findings.is_empty());
}

#[test]
fn boot_kind_filter_excludes_others() {
    let mem = Arc::new(MemImage::new(bootable_loader_sector(), Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Boot), 0);
    assert!(findings.iter().all(|f| f.plugin.as_str() == "bootsector"));
}

#[test]
fn extract_raw_is_512_bytes() {
    let mem = Arc::new(MemImage::new(bootable_loader_sector(), Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Boot), 0);
    let dir = std::env::temp_dir().join("shiraz-mcp-bootsector-test-raw");
    let extracted = extract(&findings[0], ExtractFormat::Raw, &dir, 1).unwrap();
    assert_eq!(extracted.size, SECTOR_LEN as u64);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_prg_wraps() {
    let mem = Arc::new(MemImage::new(bootable_loader_sector(), Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Boot), 0);
    let dir = std::env::temp_dir().join("shiraz-mcp-bootsector-test-prg");
    let extracted = extract(&findings[0], ExtractFormat::Prg, &dir, 1).unwrap();
    assert_eq!(extracted.size, SECTOR_LEN as u64 + 32);
    let bytes = std::fs::read(&extracted.path).unwrap();
    assert_eq!(&bytes[..2], &[0x60, 0x1A]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// // verify live: PLAN §P7 acceptance — a running session with a floppy
/// image inserted. Another agent owns Hatari now. When free: boot a disk,
/// savebin whole RAM (ADR-9), then:
///   SHIRAZMCP_RAM_DUMP=/path/to/dump.bin cargo test --test bootsector -- --ignored bootsector_live
#[test]
#[ignore = "needs a real RAM dump with a floppy inserted; set SHIRAZMCP_RAM_DUMP"]
fn bootsector_live_ram_dump_derefs_dskbufp() {
    let path = std::env::var("SHIRAZMCP_RAM_DUMP").expect("set SHIRAZMCP_RAM_DUMP");
    let bytes = std::fs::read(path).expect("readable dump");
    let mem = Arc::new(MemImage::new(bytes, Addr::new(0)));
    let findings = default_registry().scan(mem, Some(RipKind::Boot), 0);
    // May legitimately be empty if _dskbufp does not hold the sector post-boot
    // (that is the // verify live unknown). If non-empty, it must be exactly one.
    assert!(findings.len() <= 1, "at most one _dskbufp boot sector");
}
