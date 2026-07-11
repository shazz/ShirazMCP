//! ZX0 v2 fixtures + the two depack-path proofs (P12 §6): the auto-engine's
//! whole-buffer-exact gate (atomik ⊃ zx0 ⊃ PI1 recursion) and the explicit
//! `depack` tool seam (`Registry::unpack_prefix`/`scan_via`). No committed
//! binaries: Z1–Z5 are the reference `zx0` v2.2 compressor's own output,
//! Z6 was assembled per §6.1 and decode-verified with the reference `dzx0`
//! binary — see ADR-24.
#![allow(clippy::unwrap_used, reason = "test code, not library code")]

use std::sync::Arc;

use shiraz_mcp::debug;
use shiraz_mcp::ripper::image::ScreenRes;
use shiraz_mcp::ripper::plugins::zx0::Zx0;
use shiraz_mcp::ripper::{default_registry, extract, Depacker, ExtractFormat, Finding, RipKind};
use shiraz_mcp::session::{memory, HatariConfig, HatariProcess, Session};
use shiraz_mcp::types::{Addr, MemImage, PluginId};

mod common;
use common::{make_pi, plant};

// ---- pinned byte vectors (P12 §6.2) ----

const Z1: &[u8] = &[0xD5, 0x41, 0x55, 0x60];
const Z2: &[u8] = &[0x87, 0x41, 0x55, 0x55, 0x80];
const Z3: &[u8] = &[0x7B, 0x41, 0x42, 0x43, 0xFA, 0x55, 0x55, 0x80];
const Z4: &[u8] = &[0x0D, 0x57, 0x58, 0x59, 0x5A, 0x55, 0x56];
const Z5: &[u8] = &[
    0x03, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0xD8, 0xF0, 0x0D, 0x49, 0x4A, 0x4B, 0x4C,
    0x4D, 0x4E, 0x4F, 0x50, 0x55, 0x56,
];
const Z6: &[u8] = &[0x3B, 0x41, 0x42, 0xFC, 0xAD, 0xF7, 0x5A, 0x55, 0x56];

// ---- the pinned encoder-mirroring assembler (P12 §6.1) ----

/// compress.c's write_bit/write_byte, verbatim semantics (§6.1 rules).
struct Zx0Asm {
    out: Vec<u8>,
    bit_mask: u8,     // 0 = no open bit byte
    bit_index: usize, // position of the open bit byte in `out`
    backtrack: bool,  // next bit ORs into out.last_mut() bit 0
}

impl Zx0Asm {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            bit_mask: 0,
            bit_index: 0,
            backtrack: true, // rule 1: the first write_bit is swallowed
        }
    }

    fn write_byte(&mut self, v: u8) {
        self.out.push(v);
    }

    fn write_bit(&mut self, b: bool) {
        if self.backtrack {
            if b {
                if let Some(last) = self.out.last_mut() {
                    *last |= 1;
                }
            }
            self.backtrack = false;
            return;
        }
        if self.bit_mask == 0 {
            self.bit_mask = 0x80;
            self.bit_index = self.out.len();
            self.out.push(0);
        }
        if b {
            self.out[self.bit_index] |= self.bit_mask;
        }
        self.bit_mask >>= 1;
    }

    fn gamma(&mut self, value: usize, inverted: bool) {
        let mut i = 2usize;
        while i <= value {
            i <<= 1;
        }
        i >>= 2;
        while i != 0 {
            self.write_bit(false);
            self.write_bit((value & i != 0) ^ inverted);
            i >>= 1;
        }
        self.write_bit(true);
    }
}

fn ev_literals(a: &mut Zx0Asm, p: &[u8]) {
    a.write_bit(false);
    a.gamma(p.len(), false);
    for &b in p {
        a.write_byte(b);
    }
}

fn ev_copy_last(a: &mut Zx0Asm, n: usize) {
    a.write_bit(false);
    a.gamma(n, false);
}

fn ev_copy_new(a: &mut Zx0Asm, off: usize, n: usize) {
    a.write_bit(true);
    a.gamma((off - 1) / 128 + 1, true);
    a.write_byte(((127 - (off - 1) % 128) << 1) as u8);
    a.backtrack = true; // bit 0 of the LSB byte = first bit of this gamma
    a.gamma(n - 1, false);
}

fn ev_end(a: &mut Zx0Asm) {
    a.write_bit(true);
    a.gamma(256, true);
}

/// One literal run + end. The payload appears VERBATIM in the blob — never
/// use for a payload a detector could match.
fn zx0_pack_literal(payload: &[u8]) -> Vec<u8> {
    assert!(!payload.is_empty());
    let mut a = Zx0Asm::new();
    ev_literals(&mut a, payload);
    ev_end(&mut a);
    a.out
}

/// Degas-safe (ADR-24 amended construction — the original single big
/// literal run over `payload[2..]` was falsified in implementation: the
/// detector scans every even offset and `PAL[0] = 0x0000` doubles as a
/// res word, so a verbatim palette tail aliases as a PI1 wherever it
/// starts). The split must land INSIDE the palette so no surviving
/// fragment can fill a 32-byte palette window: literals `payload[..1]`;
/// `copy_last(1)` (RLE from the INITIAL last_offset 1, reconstructs
/// `payload[1]`); literals `payload[2..12]` (PAL[0..5], 10 bytes);
/// `copy_new(3, 2)` (reconstructs `payload[12..14]` from `payload[9..11]`
/// — the PAL fixture's `07 07`); literals `payload[14..]`; end. Fragment 1
/// is poisoned by the adjacent copy_new offset byte (0xFB, invalid in
/// both word parities); fragment 2's aligned words are real palette
/// colors (never a res word) running into the screen's invalid 0xFFxx
/// words. Requires `payload[0] == payload[1]` and
/// `payload[9..11] == payload[12..14]` (true for the shared PAL fixture).
fn zx0_pack_pi1(payload: &[u8]) -> Vec<u8> {
    assert_eq!(payload[0], payload[1]);
    assert_eq!(&payload[9..11], &payload[12..14]);
    let mut a = Zx0Asm::new();
    ev_literals(&mut a, &payload[..1]);
    ev_copy_last(&mut a, 1);
    ev_literals(&mut a, &payload[2..12]);
    ev_copy_new(&mut a, 3, 2);
    ev_literals(&mut a, &payload[14..]);
    ev_end(&mut a);
    a.out
}

// ---- Atomik helpers, copied verbatim from tests/depack_atomik.rs §6.1.1
// (P9 precedent: integration crates copy non-common helpers) — the outer
// layer for the atomik ⊃ zx0 ⊃ PI1 recursion fixture (§6.3).

enum Ev {
    Bits(Vec<u8>),
    Byte(u8),
}

fn bytes_loaded(n: usize) -> usize {
    if n <= 7 {
        1
    } else {
        1 + (n - 7).div_ceil(8)
    }
}

fn pack_first(bits: &[u8]) -> u8 {
    let mut v: u16 = 0;
    for &b in bits {
        v = (v << 1) | u16::from(b);
    }
    let pad = 8 - bits.len() as u32;
    ((v << pad) | (1u16 << (pad - 1))) as u8
}

fn pack_plain(bits: &[u8]) -> u8 {
    let mut v: u16 = 0;
    for &b in bits {
        v = (v << 1) | u16::from(b);
    }
    (v << (8 - bits.len() as u32)) as u8
}

fn build_bit_bytes(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let first_k = bits.len().min(7);
    out.push(pack_first(&bits[..first_k]));
    let mut i = first_k;
    while i < bits.len() {
        let end = (i + 8).min(bits.len());
        out.push(pack_plain(&bits[i..end]));
        i = end;
    }
    out
}

fn build_consumption(script: &[Ev], bit_bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut bits_so_far = 0usize;
    let mut pushed = 0usize;
    for ev in script {
        match ev {
            Ev::Bits(b) => bits_so_far += b.len(),
            Ev::Byte(byte) => {
                let needed = bytes_loaded(bits_so_far);
                while pushed < needed {
                    out.push(vec![bit_bytes[pushed]]);
                    pushed += 1;
                }
                out.push(vec![*byte]);
            }
        }
    }
    while pushed < bit_bytes.len() {
        out.push(vec![bit_bytes[pushed]]);
        pushed += 1;
    }
    out
}

fn atomik_assemble(script: &[Ev], decrunched: u32) -> Vec<u8> {
    let all_bits: Vec<u8> = script
        .iter()
        .flat_map(|ev| match ev {
            Ev::Bits(b) => b.clone(),
            Ev::Byte(_) => Vec::new(),
        })
        .collect();
    let bit_bytes = build_bit_bytes(&all_bits);
    let mut units: Vec<Vec<u8>> = vec![vec![0u8]]; // pic_count, fixed 0
    units.extend(build_consumption(script, &bit_bytes));

    let stream_len: usize = units.iter().map(Vec::len).sum();
    let mut out = Vec::new();
    out.extend_from_slice(b"ATM5");
    out.extend_from_slice(&decrunched.to_be_bytes());
    out.extend_from_slice(&(stream_len as u32).to_be_bytes());
    for chunk in units.iter().rev() {
        out.extend_from_slice(chunk);
    }
    out
}

/// One chunk, all literals. Valid for 1..=65_535 bytes.
fn atomik_pack_literal(payload: &[u8]) -> Vec<u8> {
    assert!(!payload.is_empty() && payload.len() <= 65_535);
    let mut script = vec![
        Ev::Bits(vec![0]),
        Ev::Bits(vec![0]), // no spec table, no freq table
        Ev::Byte(0x00),    // d3
        Ev::Byte((payload.len() >> 8) as u8),
        Ev::Byte(payload.len() as u8),
    ];
    for &b in payload.iter().rev() {
        // output is backward: last payload byte first
        script.push(Ev::Bits(vec![0]));
        script.push(Ev::Byte(b));
    }
    atomik_assemble(&script, payload.len() as u32)
}

// ---- misc helpers ----

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shiraz-mcp-p12-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Pinned LCG (seed 1): x = x*1103515245 + 12345 (wrapping); each byte is
/// the high byte of x, `(x >> 16) as u8` (P5's pinned generator).
fn lcg_bytes(n: usize) -> Vec<u8> {
    let mut x: u32 = 1;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (x >> 16) as u8
        })
        .collect()
}

// ---- tests ----

#[test]
fn pinned_blobs_depack() {
    assert_eq!(Zx0.try_unpack(Z1), Some(b"A".to_vec()));
    assert_eq!(Zx0.try_unpack(Z2), Some(b"AAAAAA".to_vec()));
    assert_eq!(Zx0.try_unpack(Z3), Some(b"ABCABC".to_vec()));
    assert_eq!(Zx0.try_unpack(Z4), Some(b"WXYZ".to_vec()));
    assert_eq!(
        Zx0.try_unpack(Z5),
        Some(b"ABCDEFGHABCDEFGHIJKLMNOP".to_vec())
    );
    assert_eq!(Zx0.try_unpack(Z6), Some(b"ABABAABZB".to_vec()));
}

#[test]
fn assembler_matches_pinned_vectors() {
    let mut a = Zx0Asm::new();
    ev_literals(&mut a, b"A");
    ev_end(&mut a);
    assert_eq!(a.out, Z1);

    let mut a = Zx0Asm::new();
    ev_literals(&mut a, b"A");
    ev_copy_last(&mut a, 5);
    ev_end(&mut a);
    assert_eq!(a.out, Z2);

    let mut a = Zx0Asm::new();
    ev_literals(&mut a, b"ABC");
    ev_copy_new(&mut a, 3, 3);
    ev_end(&mut a);
    assert_eq!(a.out, Z3);

    let mut a = Zx0Asm::new();
    ev_literals(&mut a, b"WXYZ");
    ev_end(&mut a);
    assert_eq!(a.out, Z4);

    let mut a = Zx0Asm::new();
    ev_literals(&mut a, b"ABCDEFGH");
    ev_copy_new(&mut a, 8, 8);
    ev_literals(&mut a, b"IJKLMNOP");
    ev_end(&mut a);
    assert_eq!(a.out, Z5);

    // Z6's script (§6.2): AB, copy_new(2,3)->ABABA, copy_new(5,2)->ABABAAB,
    // Z, copy_last(1) (offset-5 reuse -> B), end.
    let mut a = Zx0Asm::new();
    ev_literals(&mut a, b"AB");
    ev_copy_new(&mut a, 2, 3);
    ev_copy_new(&mut a, 5, 2);
    ev_literals(&mut a, b"Z");
    ev_copy_last(&mut a, 1);
    ev_end(&mut a);
    assert_eq!(a.out, Z6);
}

#[test]
fn encoder_roundtrips_via_depacker() {
    let payloads: Vec<Vec<u8>> = vec![
        b"Q".to_vec(),
        b"AB".to_vec(),
        b"WXYZ".to_vec(),
        vec![0xAAu8; 6],
        vec![0xBBu8; 10],
        vec![0xCCu8; 100],
        vec![0xDDu8; 300],
        lcg_bytes(5000),
    ];
    for p in payloads {
        assert_eq!(Zx0.try_unpack(&zx0_pack_literal(&p)), Some(p));
    }
    let pi1 = make_pi(ScreenRes::Low);
    let zblob = zx0_pack_pi1(&pi1);
    assert_eq!(zblob.len(), 32_040);
    // ADR-24 bit-exact verification vector (simulator cross-checked
    // against Z1-Z6).
    assert_eq!(
        &zblob[..18],
        &[
            0xA1, 0x00, 0x39, 0x00, 0x00, 0x07, 0x00, 0x00, 0x70, 0x00, 0x07, 0x07, 0x70, 0xFB,
            0x54, 0x40, 0x44, 0x35,
        ]
    );
    assert_eq!(Zx0.try_unpack(&zblob), Some(pi1));

    // ADR-24 darkness guard: the raw blob must alias as no picture at ANY
    // offset with no depacking — this is the property tests 5/6 depend
    // on, pinned mechanically instead of by prose claim.
    let zblob_again = zx0_pack_pi1(&make_pi(ScreenRes::Low));
    let dark = MemImage::new(zblob_again, Addr::new(0));
    assert!(default_registry()
        .scan(Arc::new(dark), Some(RipKind::Picture), 0)
        .is_empty());
}

#[test]
fn full_consumption_gate() {
    // (a) exact stream.
    assert_eq!(Zx0.try_unpack(Z2), Some(b"AAAAAA".to_vec()));

    // (b) trailing bytes: try_unpack (exact gate) fails; try_unpack_prefix
    // (explicit path) succeeds and reports the exact consumed length.
    let mut with_trailer = Z2.to_vec();
    with_trailer.extend([0x99; 7]);
    assert_eq!(Zx0.try_unpack(&with_trailer), None);
    assert_eq!(
        Zx0.try_unpack_prefix(&with_trailer),
        Some((b"AAAAAA".to_vec(), 5))
    );

    // (c) leading zeros before the stream: offset-0-only gate rejects.
    let mut buf = vec![0u8; 4096];
    plant(&mut buf, 0x100, Z2);
    assert_eq!(Zx0.try_unpack(&buf), None);
}

#[test]
fn packed_pi1_recursion_finds_picture() {
    let pi1 = make_pi(ScreenRes::Low);
    let blob = atomik_pack_literal(&zx0_pack_pi1(&pi1));
    assert_eq!(blob.len(), 36_062);
    let mut buf = vec![0u8; 256 * 1024];
    plant(&mut buf, 0x0400, &blob);
    let mem = Arc::new(MemImage::new(buf, Addr::new(0x10000)));
    let registry = default_registry();
    let findings = registry.scan(mem, Some(RipKind::Picture), 2);

    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.plugin.as_str(), "degas");
    assert_eq!(f.via(), [PluginId::new("atomik"), PluginId::new("zx0")]);
    assert_eq!(f.offset.raw(), 0);
    assert_eq!(f.len, 32_034);
    assert!(f.addr().is_none());
    assert_eq!(f.bytes().unwrap(), pi1.as_slice());

    let png = registry.render_png(f.plugin, f.bytes().unwrap()).unwrap();
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (320, 200));
    let rgba8 = decoded.to_rgba8();
    assert_eq!(rgba8.get_pixel(0, 0).0, [0, 0, 0, 255]);
    assert_eq!(rgba8.get_pixel(25, 0).0, [255, 0, 0, 255]);
    assert_eq!(rgba8.get_pixel(319, 199).0, [182, 182, 182, 255]);

    let dir = temp_dir("recursion");
    let extracted = extract(f, ExtractFormat::Raw, &dir, 1).unwrap();
    assert_eq!(std::fs::read(&extracted.path).unwrap(), pi1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn depth_boundaries() {
    let pi1 = make_pi(ScreenRes::Low);
    let blob = atomik_pack_literal(&zx0_pack_pi1(&pi1));
    let mut buf = vec![0u8; 256 * 1024];
    plant(&mut buf, 0x0400, &blob);
    let mem = Arc::new(MemImage::new(buf, Addr::new(0x10000)));
    let registry = default_registry();

    assert!(registry
        .scan(mem.clone(), Some(RipKind::Picture), 0)
        .is_empty());
    // The zx0 blob surfaces at depth 1, but zx0_pack_pi1 broke its
    // res+palette header contiguity — the split hides it, not luck.
    assert!(registry
        .scan(mem.clone(), Some(RipKind::Picture), 1)
        .is_empty());

    let findings = registry.scan(mem, Some(RipKind::Packer), 0);
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.plugin.as_str(), "atomik-region");
    assert_eq!(f.offset.raw(), 0x0400);
    assert_eq!(f.len, blob.len());
    assert_eq!(f.addr(), Some(Addr::new(0x10400)));
}

#[test]
fn explicit_depack_path() {
    let pi1 = make_pi(ScreenRes::Low);
    let zblob = zx0_pack_pi1(&pi1);
    let mut buf = vec![0u8; 64 * 1024];
    plant(&mut buf, 0x400, &zblob);
    buf[0xF000] = 0xFF; // junk after the stream — the prefix case
    let mem = Arc::new(MemImage::new(buf, Addr::new(0)));
    let registry = default_registry();
    let id = PluginId::new("zx0");

    let (unpacked_bytes, consumed) = registry.unpack_prefix(id, &mem.bytes()[0x400..]).unwrap();
    assert_eq!(consumed, 32_040);
    assert_eq!(unpacked_bytes, pi1);

    let unpacked = Arc::new(MemImage::new(unpacked_bytes, Addr::new(0)));
    let via = [id];
    let whole = Finding::whole_buffer(&unpacked, id, &via, "zx0 depacked blob".to_string());
    assert_eq!(whole.bytes(), Some(pi1.as_slice()));
    assert!(whole.addr().is_none());
    assert_eq!(whole.len, 32_034);

    let findings = registry.scan_via(unpacked, Some(RipKind::Picture), 2, &via);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].plugin.as_str(), "degas");
    assert_eq!(findings[0].via(), via);
    assert!(findings[0].addr().is_none());

    // The miss: starting inside the leading zeros does not decode.
    assert!(registry.unpack_prefix(id, &mem.bytes()[0x300..]).is_none());
}

#[test]
fn corruption_never_panics() {
    for pos in 0..Z6.len() {
        for byte in 0..=u8::MAX {
            let mut mutated = Z6.to_vec();
            mutated[pos] = byte;
            let _ = Zx0.try_unpack(&mutated);
        }
    }
    for k in 0..Z5.len() {
        assert_eq!(Zx0.try_unpack(&Z5[..k]), None, "k={k}");
    }
}

// Duplicated from tests/integration.rs (separate test crates cannot share
// items without more tests/common/ additions than this one shared helper
// warrants): MJJ config + the "MJJHACK.PRG's own code is running" poll.
fn mjj_config(name: &str) -> HatariConfig {
    HatariConfig {
        harddrive: Some("/path/to/projects/MJJ/build".into()),
        work_dir: format!("/tmp/shiraz-mcp-p12-itest-{name}").into(),
        ..HatariConfig::default()
    }
}

#[allow(clippy::panic, reason = "integration test, not library code")]
async fn wait_until_ready(proc: &mut HatariProcess) {
    for _ in 0..50 {
        let symbols = debug::load_symbols(proc).await;
        let regs = debug::registers(proc).await.unwrap();
        if symbols.is_ok() && regs.pc.raw() < (1 << 20) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("MJJHACK.PRG never became ready (loaded + PC in RAM) within the poll budget");
}

/// Load a zx0-packed PI1 into live ST RAM and prove the `depack` tool's
/// server-side seam (unpack_prefix + scan_via) finds it, mirroring what
/// `tools/depack.rs::depack` does. Requires Hatari + MJJ — not run by this
/// phase's implementer (another agent owns the emulator).
#[tokio::test]
#[ignore = "requires a running Hatari + MJJ session (see tests/integration.rs)"]
async fn depacked_pi1_live_depack() {
    let config = mjj_config("p12");
    let work_dir = config.work_dir.clone();
    let mut session = Session::new(config).start().await.unwrap();
    wait_until_ready(session.process_mut()).await;
    let mut paused = session.pause().await.unwrap();

    let pi1 = make_pi(ScreenRes::Low);
    let blob = zx0_pack_pi1(&pi1);
    let path = work_dir.join("packed.zx0");
    std::fs::write(&path, &blob).unwrap();

    let loaded = paused
        .process_mut()
        .debug(&format!("loadbin {} $60000", path.display()))
        .await;
    if loaded.is_err() {
        for (i, chunk) in blob.chunks(1024).enumerate() {
            let addr = Addr::new(0x60000 + (i * 1024) as u32);
            debug::write_bytes(paused.process_mut(), addr, chunk)
                .await
                .unwrap();
        }
    }

    let mem = memory::acquire(paused.process_mut()).await.unwrap();
    let base = mem.base().raw();
    let registry = default_registry();
    let id = PluginId::new("zx0");
    let off = (0x60000 - base) as usize;
    let (unpacked_bytes, consumed) = registry.unpack_prefix(id, &mem.bytes()[off..]).unwrap();
    assert_eq!(consumed, 32_040);
    let unpacked = Arc::new(MemImage::new(unpacked_bytes, Addr::new(0)));
    let findings = registry.scan_via(unpacked, Some(RipKind::Picture), 2, &[id]);
    let hit = findings
        .iter()
        .find(|f| f.plugin.as_str() == "degas")
        .unwrap();
    assert_eq!(hit.bytes().unwrap(), pi1.as_slice());

    paused.stop().await;
}
