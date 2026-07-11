//! Fixture-plugin proof of the ripper engine (P3.md §7.2). No Hatari: these
//! fixture plugins implement the public `RipPlugin`/`Depacker`/`Deprotect`
//! traits from OUTSIDE the crate, which itself proves the extension seam is
//! externally usable (ADR-2).

// [clippy::unwrap_used] this whole binary is test code (see
// tests/mcp_handshake.rs for the same rationale) — a panic on unexpected
// shape is the correct failure mode.
#![allow(clippy::unwrap_used, reason = "test code, not library code")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use shiraz_mcp::error::{Error, Result};
use shiraz_mcp::ripper::registry::MAX_UNPACKED;
use shiraz_mcp::ripper::{
    default_registry, extract, Depacker, Deprotect, ExtractFormat, Finding, ProtectionInfo,
    Registry, RipKind, RipPlugin,
};
use shiraz_mcp::types::{Addr, MemImage, Offset, PluginId};

// ---- fixture plugins ----

/// Toy picture format: magic "TOYF" + u16 BE payload_len + payload bytes.
struct ToyPic;
impl RipPlugin for ToyPic {
    fn id(&self) -> PluginId {
        PluginId::new("toy-pic")
    }
    fn kind(&self) -> RipKind {
        RipKind::Picture
    }
    fn describe(&self) -> &'static str {
        "fixture: TOYF magic picture"
    }
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        memchr::memmem::find_iter(mem.bytes(), b"TOYF")
            .filter_map(|pos| toyf_finding(mem.bytes(), pos))
            .collect()
    }
}

fn toyf_finding(bytes: &[u8], pos: usize) -> Option<Finding> {
    let len_bytes = bytes.get(pos + 4..pos + 6)?;
    let payload_len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
    let total = 6 + payload_len;
    if pos + total > bytes.len() {
        return None;
    }
    Some(Finding::new(
        PluginId::new("toy-pic"),
        RipKind::Picture,
        Offset::new(pos),
        total,
        format!("toy picture, {payload_len} byte payload"),
    ))
}

/// Toy boot format: magic "TOYB", fixed len 512 (skip if it would overrun).
struct ToyBoot;
impl RipPlugin for ToyBoot {
    fn id(&self) -> PluginId {
        PluginId::new("toy-boot")
    }
    fn kind(&self) -> RipKind {
        RipKind::Boot
    }
    fn describe(&self) -> &'static str {
        "fixture: TOYB magic boot sector"
    }
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        memchr::memmem::find_iter(mem.bytes(), b"TOYB")
            .filter(|&pos| pos + 512 <= mem.len())
            .map(|pos| {
                Finding::new(
                    PluginId::new("toy-boot"),
                    RipKind::Boot,
                    Offset::new(pos),
                    512,
                    "toy boot sector".to_string(),
                )
            })
            .collect()
    }
}

/// Toy depacker: "TPCK" + u32 BE unpacked_len, then unpacked_len bytes each
/// offset by +1 (mod 256). Deliberately NOT an involution (unlike XOR with
/// a fixed key): `recursion_depth_is_bounded` packs this twice, and a
/// self-inverse transform would let two applications cancel out and leak
/// the original plaintext magic straight into the doubly-packed buffer,
/// defeating the depth-bound test.
struct ToyPack;
impl Depacker for ToyPack {
    fn id(&self) -> PluginId {
        PluginId::new("toy-pack")
    }
    fn describe(&self) -> &'static str {
        "fixture: TPCK +1 depacker"
    }
    fn try_unpack(&self, data: &[u8]) -> Option<Vec<u8>> {
        let prefix = data.get(..4)?;
        if prefix != b"TPCK" {
            return None;
        }
        let len_bytes = data.get(4..8)?;
        let len = u32::from_be_bytes(len_bytes.try_into().ok()?) as usize;
        let body = data.get(8..8 + len)?;
        Some(body.iter().map(|b| b.wrapping_sub(1)).collect())
    }
}

/// Helper: build a `ToyPack`-packed buffer around `payload`.
fn toy_pack(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(b"TPCK");
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend(payload.iter().map(|b| b.wrapping_add(1)));
    out
}

/// Rendezvous pair proving parallel detector execution: each scan
/// increments a shared counter then spins until it reads >= 2 or 2s
/// elapse; sets `ok` only if the rendezvous succeeded. Sequential
/// execution can never succeed; concurrent execution always does (needs a
/// rayon pool with >= 2 threads — true on any dev/CI machine).
struct Rendezvous {
    gate: Arc<AtomicUsize>,
    ok: Arc<AtomicBool>,
    name: &'static str,
}
impl RipPlugin for Rendezvous {
    fn id(&self) -> PluginId {
        PluginId::new(self.name)
    }
    fn kind(&self) -> RipKind {
        RipKind::Raw
    }
    fn describe(&self) -> &'static str {
        "fixture: rendezvous"
    }
    fn scan(&self, _mem: &MemImage) -> Vec<Finding> {
        self.gate.fetch_add(1, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.gate.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::hint::spin_loop();
        }
        if self.gate.load(Ordering::SeqCst) >= 2 {
            self.ok.store(true, Ordering::SeqCst);
        }
        Vec::new()
    }
}

/// Spy deprotector: detects the literal "PROT"; `strip` sets a shared flag
/// and returns `Err` (never expected to be called — ADR-6).
struct SpyDeprotect {
    stripped: Arc<AtomicBool>,
}
impl Deprotect for SpyDeprotect {
    fn id(&self) -> PluginId {
        PluginId::new("toy-prot")
    }
    fn describe(&self) -> &'static str {
        "fixture: PROT spy deprotector"
    }
    fn detect(&self, data: &[u8]) -> Option<ProtectionInfo> {
        let pos = memchr::memmem::find(data, b"PROT")?;
        Some(ProtectionInfo {
            name: "toy-prot-scheme",
            offset: Offset::new(pos),
            len: 4,
        })
    }
    fn strip(&self, _data: &[u8]) -> Result<Vec<u8>> {
        self.stripped.store(true, Ordering::SeqCst);
        Err(Error::Ripper {
            plugin: self.id(),
            message: "spy strip must never be called".to_string(),
        })
    }
}

/// Bomb depacker: "BOMB" prefix → an oversized unpacked buffer.
struct BombPack;
impl Depacker for BombPack {
    fn id(&self) -> PluginId {
        PluginId::new("toy-bomb")
    }
    fn describe(&self) -> &'static str {
        "fixture: depack bomb"
    }
    fn try_unpack(&self, data: &[u8]) -> Option<Vec<u8>> {
        if data.starts_with(b"BOMB") {
            Some(vec![0; MAX_UNPACKED + 1])
        } else {
            None
        }
    }
}

/// Liar detector: always reports one out-of-bounds finding — must be
/// dropped by the engine at attach.
struct Liar;
impl RipPlugin for Liar {
    fn id(&self) -> PluginId {
        PluginId::new("toy-liar")
    }
    fn kind(&self) -> RipKind {
        RipKind::Raw
    }
    fn describe(&self) -> &'static str {
        "fixture: out-of-bounds liar"
    }
    fn scan(&self, mem: &MemImage) -> Vec<Finding> {
        vec![Finding::new(
            self.id(),
            RipKind::Raw,
            Offset::new(0),
            mem.len() + 1000,
            "lie".to_string(),
        )]
    }
}

// ---- shared helpers ----

fn img(bytes: Vec<u8>) -> Arc<MemImage> {
    Arc::new(MemImage::new(bytes, Addr::new(0)))
}

fn plant(buf: &mut [u8], at: usize, data: &[u8]) {
    buf[at..at + data.len()].copy_from_slice(data);
}

fn toyf(payload_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + payload_len);
    out.extend_from_slice(b"TOYF");
    out.extend_from_slice(&(payload_len as u16).to_be_bytes());
    out.extend(std::iter::repeat_n(0xC5u8, payload_len));
    out
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shiraz-mcp-p3-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---- tests ----

#[test]
fn detectors_find_planted_magic() {
    let mut buf = vec![0u8; 64 * 1024];
    let toyf_bytes = toyf(16);
    plant(&mut buf, 0x1234, &toyf_bytes);
    plant(&mut buf, 0x8000, b"TOYB");
    let registry = Registry::new()
        .with_detector(Box::new(ToyPic))
        .with_detector(Box::new(ToyBoot));
    let findings = registry.scan(img(buf), None, 0);

    assert_eq!(findings.len(), 2);
    let pic = findings
        .iter()
        .find(|f| f.plugin.as_str() == "toy-pic")
        .unwrap();
    assert_eq!(pic.kind, RipKind::Picture);
    assert_eq!(pic.offset.raw(), 0x1234);
    assert_eq!(pic.len, 22);
    assert!(pic.via().is_empty());
    assert_eq!(pic.addr(), Some(Addr::new(0x1234)));
    assert_eq!(pic.bytes().unwrap(), toyf_bytes.as_slice());

    let boot = findings
        .iter()
        .find(|f| f.plugin.as_str() == "toy-boot")
        .unwrap();
    assert_eq!(boot.kind, RipKind::Boot);
    assert_eq!(boot.len, 512);
}

#[test]
fn kind_filter_selects_only_matching() {
    let mut buf = vec![0u8; 64 * 1024];
    plant(&mut buf, 0x1234, &toyf(16));
    plant(&mut buf, 0x8000, b"TOYB");
    let registry = Registry::new()
        .with_detector(Box::new(ToyPic))
        .with_detector(Box::new(ToyBoot));
    let findings = registry.scan(img(buf), Some(RipKind::Boot), 2);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].plugin.as_str(), "toy-boot");
}

#[test]
fn detectors_run_in_parallel() {
    let gate = Arc::new(AtomicUsize::new(0));
    let ok_a = Arc::new(AtomicBool::new(false));
    let ok_b = Arc::new(AtomicBool::new(false));
    let registry = Registry::new()
        .with_detector(Box::new(Rendezvous {
            gate: gate.clone(),
            ok: ok_a.clone(),
            name: "rv-a",
        }))
        .with_detector(Box::new(Rendezvous {
            gate,
            ok: ok_b.clone(),
            name: "rv-b",
        }));
    registry.scan(img(vec![0u8; 1024]), None, 0);
    assert!(ok_a.load(Ordering::SeqCst));
    assert!(ok_b.load(Ordering::SeqCst));
}

#[test]
fn depack_rescan_records_provenance() {
    let mut inner = vec![0u8; 64 * 1024];
    plant(&mut inner, 0x40, &toyf(16));
    let packed = toy_pack(&inner);
    let registry = Registry::new()
        .with_detector(Box::new(ToyPic))
        .with_depacker(Box::new(ToyPack));
    let findings = registry.scan(img(packed), None, 2);

    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].via(), [PluginId::new("toy-pack")]);
    assert_eq!(findings[0].offset.raw(), 0x40);
    assert!(findings[0].addr().is_none());
    assert_eq!(findings[0].bytes().unwrap(), toyf(16).as_slice());
}

#[test]
fn recursion_depth_is_bounded() {
    let mut inner = vec![0u8; 64 * 1024];
    plant(&mut inner, 0x40, &toyf(16));
    let double_packed = toy_pack(&toy_pack(&inner));
    let registry = Registry::new()
        .with_detector(Box::new(ToyPic))
        .with_depacker(Box::new(ToyPack));

    assert!(registry
        .scan(img(double_packed.clone()), None, 1)
        .is_empty());
    let findings = registry.scan(img(double_packed), None, 2);
    assert_eq!(findings.len(), 1);
    assert_eq!(
        findings[0].via(),
        [PluginId::new("toy-pack"), PluginId::new("toy-pack")]
    );
}

#[test]
fn strip_is_never_called() {
    let mut buf = vec![0u8; 4096];
    plant(&mut buf, 0x100, b"PROT");
    let stripped = Arc::new(AtomicBool::new(false));
    let registry = Registry::new().with_deprotect(Box::new(SpyDeprotect {
        stripped: stripped.clone(),
    }));
    let findings = registry.scan(img(buf), None, 2);

    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.plugin.as_str(), "toy-prot");
    assert_eq!(f.kind, RipKind::Raw);
    assert_eq!(f.label, "protection detected: toy-prot-scheme");
    assert_eq!(f.offset.raw(), 0x100);
    assert_eq!(f.len, 4);
    assert!(!stripped.load(Ordering::SeqCst));
}

#[test]
fn deprotect_skipped_under_kind_filter() {
    let mut buf = vec![0u8; 4096];
    plant(&mut buf, 0x100, b"PROT");
    let stripped = Arc::new(AtomicBool::new(false));
    let registry = Registry::new().with_deprotect(Box::new(SpyDeprotect { stripped }));
    let findings = registry.scan(img(buf), Some(RipKind::Picture), 2);
    assert!(findings.is_empty());
}

#[test]
fn depack_bomb_is_capped() {
    let mut buf = b"BOMB".to_vec();
    buf.extend_from_slice(&[0u8; 16]);
    let registry = Registry::new()
        .with_detector(Box::new(ToyPic))
        .with_depacker(Box::new(BombPack));
    let findings = registry.scan(img(buf), None, 2);
    assert!(findings.is_empty());
}

#[test]
fn oob_finding_is_dropped() {
    let registry = Registry::new().with_detector(Box::new(Liar));
    let findings = registry.scan(img(vec![0u8; 1024]), None, 0);
    assert!(findings.is_empty());
}

#[test]
fn extract_raw_roundtrip() {
    let mut buf = vec![0u8; 64 * 1024];
    let toyf_bytes = toyf(16);
    plant(&mut buf, 0x1234, &toyf_bytes);
    let registry = Registry::new().with_detector(Box::new(ToyPic));
    let findings = registry.scan(img(buf), None, 0);
    let finding = &findings[0];

    let dir = temp_dir("raw");
    let extracted = extract(finding, ExtractFormat::Raw, &dir, 1).unwrap();
    assert_eq!(extracted.path, dir.join("rip-0001-toy-pic-001234.bin"));
    assert_eq!(std::fs::read(&extracted.path).unwrap(), toyf_bytes);
    assert_eq!(extracted.size, 22);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_prg_wrapper() {
    let mut buf = vec![0u8; 64 * 1024];
    let toyf_bytes = toyf(16);
    plant(&mut buf, 0x1234, &toyf_bytes);
    let registry = Registry::new().with_detector(Box::new(ToyPic));
    let findings = registry.scan(img(buf), None, 0);
    let finding = &findings[0];

    let dir = temp_dir("prg");
    let extracted = extract(finding, ExtractFormat::Prg, &dir, 2).unwrap();
    assert_eq!(extracted.path, dir.join("rip-0002-toy-pic-001234.prg"));
    let bytes = std::fs::read(&extracted.path).unwrap();
    assert_eq!(bytes.len(), 22 + 32);
    assert_eq!(&bytes[..2], &[0x60, 0x1A]);
    assert_eq!(&bytes[2..6], &22u32.to_be_bytes());
    assert_eq!(&bytes[28..50], toyf_bytes.as_slice());
    assert_eq!(&bytes[50..54], &[0, 0, 0, 0]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn add_plugin_is_one_line() {
    // Base count tracks default_registry()'s own plugin count (P6: degas +
    // neochrome + pack-ice-region + sndh + ym-player + pack-ice +
    // noop-deprotect = 7) — the invariant under test is "+1", not a literal
    // constant.
    let base = default_registry().summaries().len();
    let registry = default_registry().with_detector(Box::new(ToyPic));
    assert_eq!(registry.summaries().len(), base + 1);
}
