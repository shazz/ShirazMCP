//! RNC ProPack (`RNC\x01`/`RNC\x02`) fixtures + the depack→rescan
//! recursion proof, both methods (P9 §6.3). No committed binaries:
//! fixtures are byte-pinned builders (hand-traced vectors + pinned stream
//! writers), consistent with ADR-12's fixture policy — see ADR-19.
#![allow(clippy::unwrap_used, reason = "test code, not library code")]

use std::sync::Arc;

use shiraz_mcp::ripper::image::ScreenRes;
use shiraz_mcp::ripper::plugins::rnc::{Rnc, RncRegion};
use shiraz_mcp::ripper::{default_registry, extract, Depacker, ExtractFormat, RipKind, RipPlugin};
use shiraz_mcp::types::{Addr, MemImage, PluginId};

mod common;
use common::{make_pi, plant};

// ---- helpers (P9 §6.3.1) ----

/// Duplicate of the library's CRC-16/ARC (test crates can't reach
/// `pub(crate)` items).
fn crc16(data: &[u8]) -> u16 {
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

fn rnc_wrap(method: u8, stream: &[u8], out: &[u8]) -> Vec<u8> {
    let mut blob = b"RNC".to_vec();
    blob.push(method);
    blob.extend((out.len() as u32).to_be_bytes());
    blob.extend((stream.len() as u32).to_be_bytes());
    blob.extend(crc16(out).to_be_bytes());
    blob.extend(crc16(stream).to_be_bytes());
    blob.extend([0x00, 0x01]);
    blob.extend_from_slice(stream);
    blob
}

/// Mirrors the method-1 reader: 16-bit LE cells; a raw run reclaims the
/// (always fully-unconsumed) lookahead cell's 2-byte slot and re-allocates
/// one after the run. Values are written LSB-first (peek semantics).
struct R1Writer {
    stream: Vec<u8>,
    cur: usize,
    f: u32,
    la: usize,
}

impl R1Writer {
    fn new() -> Self {
        Self {
            stream: vec![0, 0, 0, 0],
            cur: 0,
            f: 0,
            la: 2,
        }
    }

    fn bits(&mut self, v: u32, n: u32) {
        for j in 0..n {
            if self.f == 16 {
                self.cur = self.la;
                self.f = 0;
                self.la = self.stream.len();
                self.stream.extend([0, 0]);
            }
            if (v >> j) & 1 != 0 {
                let byte_idx = self.cur + (self.f / 8) as usize;
                self.stream[byte_idx] |= 1 << (self.f % 8);
            }
            self.f += 1;
        }
    }

    fn raw(&mut self, data: &[u8]) {
        assert_eq!(self.la, self.stream.len() - 2);
        self.stream.truncate(self.stream.len() - 2);
        self.stream.extend_from_slice(data);
        self.la = self.stream.len();
        self.stream.extend([0, 0]);
    }
}

fn r1_table(w: &mut R1Writer, lens: &[u32]) {
    w.bits(lens.len() as u32, 5);
    for &l in lens {
        w.bits(l, 4);
    }
}

/// Literal runs of <=16 raw bytes, one run per chunk set (`ch_count = 1`
/// — no matches, empty dist/len tables). Interrupts the payload every
/// <=16 bytes in the blob (< the 34-byte Degas header+palette).
fn rnc1_pack_pi1(payload: &[u8]) -> Vec<u8> {
    assert_eq!(payload.len() % 16, 2, "run split is 16/2-specific");
    let mut w = R1Writer::new();
    w.bits(0, 2); // pad
    for run in payload.chunks(16) {
        match run.len() {
            16 => {
                r1_table(&mut w, &[0, 0, 0, 0, 0, 1]); // symbol 5: (1<<4)|extra4
                r1_table(&mut w, &[]);
                r1_table(&mut w, &[]);
                w.bits(1, 16);
                w.bits(0, 1);
                w.bits(0, 4);
            }
            2 => {
                r1_table(&mut w, &[0, 0, 1]); // symbol 2: (1<<1)|extra1
                r1_table(&mut w, &[]);
                r1_table(&mut w, &[]);
                w.bits(1, 16);
                w.bits(0, 1);
                w.bits(0, 1);
            }
            n => unreachable!("32034 = 2002*16 + 2, n = {n}"),
        }
        w.raw(run);
    }
    w.stream
}

/// One decode event, in DECODER-consumption order (method 2 is forward —
/// no reversal, contrast Atomik's backward assembler).
enum Ev {
    Bits(Vec<u8>),
    Byte(u8),
}

/// §6.3.1: the init bit byte carries 2 pad + 6 data bits; each following
/// byte carries 8, zero-padded low if incomplete.
fn bytes_loaded_m2(n: usize) -> usize {
    if n <= 6 {
        1
    } else {
        1 + (n - 6).div_ceil(8)
    }
}

fn pack_first_m2(bits: &[u8]) -> u8 {
    let mut v: u8 = 0;
    for &b in bits {
        v = (v << 1) | b;
    }
    v << (6 - bits.len() as u32)
}

fn pack_plain_m2(bits: &[u8]) -> u8 {
    let mut v: u8 = 0;
    for &b in bits {
        v = (v << 1) | b;
    }
    v << (8 - bits.len() as u32)
}

fn build_bit_bytes_m2(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let first_k = bits.len().min(6);
    out.push(pack_first_m2(&bits[..first_k]));
    let mut i = first_k;
    while i < bits.len() {
        let end = (i + 8).min(bits.len());
        out.push(pack_plain_m2(&bits[i..end]));
        i = end;
    }
    out
}

/// Assemble a method-2 stream from a script: forward units, no reversal.
fn rnc2_assemble(script: &[Ev]) -> Vec<u8> {
    let all_bits: Vec<u8> = script
        .iter()
        .flat_map(|ev| match ev {
            Ev::Bits(b) => b.clone(),
            Ev::Byte(_) => Vec::new(),
        })
        .collect();
    let bit_bytes = build_bit_bytes_m2(&all_bits);
    let mut out: Vec<u8> = Vec::new();
    let mut bits_so_far = 0usize;
    let mut pushed = 0usize;
    for ev in script {
        match ev {
            Ev::Bits(b) => bits_so_far += b.len(),
            Ev::Byte(byte) => {
                let needed = bytes_loaded_m2(bits_so_far);
                while pushed < needed {
                    out.push(bit_bytes[pushed]);
                    pushed += 1;
                }
                out.push(*byte);
            }
        }
    }
    while pushed < bit_bytes.len() {
        out.push(bit_bytes[pushed]);
        pushed += 1;
    }
    out
}

/// Literal-only, forward: every payload byte is its own flag+literal
/// event, terminated by the EOF escape. Accepts any payload.
fn rnc2_pack_pi1(payload: &[u8]) -> Vec<u8> {
    let mut script = Vec::new();
    for &b in payload {
        script.push(Ev::Bits(vec![0]));
        script.push(Ev::Byte(b));
    }
    script.push(Ev::Bits(vec![1, 1, 1, 1])); // flag + "111"
    script.push(Ev::Byte(0x00)); // escape byte
    script.push(Ev::Bits(vec![0])); // end-of-stream bit
    rnc2_assemble(&script)
}

// ---- pinned byte vectors (P9 §6.3.2) ----

const M1_STREAM: &[u8] = &[
    0x88, 0x88, 0x10, 0x03, 0x20, 0x04, 0x00, 0x12, 0x41, 0x00, 0x00,
];
const M2_STREAM: &[u8] = &[0x07, 0x41, 0x42, 0x43, 0x3C, 0x02, 0x00];

// ---- misc helpers ----

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shiraz-mcp-p9-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

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
fn crc16_check_value() {
    assert_eq!(crc16(b"123456789"), 0xBB3D);
    assert_eq!(crc16(b"AAAAAA"), 0x5687);
    assert_eq!(crc16(b"ABCABC"), 0xDF43);
}

#[test]
fn pinned_blobs_depack_both_methods() {
    assert_eq!(
        Rnc.try_unpack(&rnc_wrap(1, M1_STREAM, b"AAAAAA")),
        Some(b"AAAAAA".to_vec())
    );
    assert_eq!(
        Rnc.try_unpack(&rnc_wrap(2, M2_STREAM, b"ABCABC")),
        Some(b"ABCABC".to_vec())
    );
}

#[test]
fn writers_match_pinned_vectors() {
    let mut w = R1Writer::new();
    w.bits(0, 2);
    r1_table(&mut w, &[1, 1]);
    r1_table(&mut w, &[1]);
    r1_table(&mut w, &[0, 0, 1]);
    w.bits(2, 16);
    w.bits(1, 1);
    w.raw(b"A");
    w.bits(0, 1);
    w.bits(0, 1);
    w.bits(1, 1);
    w.bits(0, 1);
    assert_eq!(w.stream, M1_STREAM);

    let script = [
        Ev::Bits(vec![0]),
        Ev::Byte(0x41),
        Ev::Bits(vec![0]),
        Ev::Byte(0x42),
        Ev::Bits(vec![0]),
        Ev::Byte(0x43),
        Ev::Bits(vec![1, 1, 1, 0]),
        Ev::Bits(vec![0]),
        Ev::Byte(0x02),
        Ev::Bits(vec![1, 1, 1, 1]),
        Ev::Byte(0x00),
        Ev::Bits(vec![0]),
    ];
    assert_eq!(rnc2_assemble(&script), M2_STREAM);
}

#[test]
fn crc_gate_rejects() {
    let good = rnc_wrap(1, M1_STREAM, b"AAAAAA");

    let mut flipped_stream = good.clone();
    let last = flipped_stream.len() - 1;
    flipped_stream[last] ^= 0xFF;
    assert_eq!(Rnc.try_unpack(&flipped_stream), None);
    let mem = MemImage::new(flipped_stream, Addr::new(0));
    assert!(RncRegion.scan(&mem).is_empty());

    let mut flipped_crc_unp = good.clone();
    flipped_crc_unp[12] ^= 0xFF;
    assert_eq!(Rnc.try_unpack(&flipped_crc_unp), None);
    let mem = MemImage::new(flipped_crc_unp, Addr::new(0));
    assert_eq!(RncRegion.scan(&mem).len(), 1); // packed CRC still ok
}

fn m1_planted_buf() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let pi1 = make_pi(ScreenRes::Low);
    let stream = rnc1_pack_pi1(&pi1);
    let blob = rnc_wrap(1, &stream, &pi1);
    assert_eq!(blob.len(), 47_076);
    let mut buf = vec![0u8; 256 * 1024];
    plant(&mut buf, 0x0400, &blob);
    (buf, blob, pi1)
}

fn m2_planted_buf() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let pi1 = make_pi(ScreenRes::Low);
    let stream = rnc2_pack_pi1(&pi1);
    let blob = rnc_wrap(2, &stream, &pi1);
    assert_eq!(blob.len(), 36_059);
    let mut buf = vec![0u8; 256 * 1024];
    plant(&mut buf, 0x0400, &blob);
    (buf, blob, pi1)
}

fn assert_recursion_finds_picture(buf: Vec<u8>, pi1: &[u8], tag: &str) {
    let mem = Arc::new(MemImage::new(buf, Addr::new(0x10000)));
    let registry = default_registry();
    let findings = registry.scan(mem, Some(RipKind::Picture), 2);

    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.plugin.as_str(), "degas");
    assert_eq!(f.via(), [PluginId::new("rnc")]);
    assert_eq!(f.bytes().unwrap(), pi1);

    let png = registry.render_png(f.plugin, f.bytes().unwrap()).unwrap();
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (320, 200));
    let rgba8 = decoded.to_rgba8();
    assert_eq!(rgba8.get_pixel(0, 0).0, [0, 0, 0, 255]);
    assert_eq!(rgba8.get_pixel(25, 0).0, [255, 0, 0, 255]);
    assert_eq!(rgba8.get_pixel(319, 199).0, [182, 182, 182, 255]);

    let dir = temp_dir(tag);
    let extracted = extract(f, ExtractFormat::Raw, &dir, 1).unwrap();
    assert_eq!(std::fs::read(&extracted.path).unwrap(), pi1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn m1_packed_pi1_recursion() {
    let (buf, _blob, pi1) = m1_planted_buf();
    assert_recursion_finds_picture(buf, &pi1, "recursion-m1");
}

#[test]
fn m2_packed_pi1_recursion() {
    let (buf, _blob, pi1) = m2_planted_buf();
    assert_recursion_finds_picture(buf, &pi1, "recursion-m2");
}

#[test]
fn depth_zero_sees_region_not_picture() {
    let (buf, blob, _pi1) = m1_planted_buf();
    let mem = Arc::new(MemImage::new(buf, Addr::new(0x10000)));
    let registry = default_registry();

    assert!(registry
        .scan(mem.clone(), Some(RipKind::Picture), 0)
        .is_empty());

    let findings = registry.scan(mem, Some(RipKind::Packer), 0);
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.plugin.as_str(), "rnc-region");
    assert_eq!(f.len, blob.len());
    assert!(f.label.contains("method 1"));
    assert!(f.label.contains("47076"));
    assert!(f.label.contains("32034"));
}

#[test]
fn roundtrips() {
    let payloads: Vec<Vec<u8>> = vec![
        b"Q".to_vec(),
        b"AB".to_vec(),
        b"WXYZ".to_vec(),
        vec![0xCCu8; 100],
        lcg_bytes(5000),
        make_pi(ScreenRes::Low),
    ];
    for p in payloads {
        let stream = rnc2_pack_pi1(&p);
        assert_eq!(Rnc.try_unpack(&rnc_wrap(2, &stream, &p)), Some(p));
    }

    let pi1 = make_pi(ScreenRes::Low);
    assert_eq!(pi1.len() % 16, 2, "rnc1_pack_pi1's precondition");
    let stream = rnc1_pack_pi1(&pi1);
    assert_eq!(Rnc.try_unpack(&rnc_wrap(1, &stream, &pi1)), Some(pi1));
}

#[test]
fn corruption_never_panics() {
    let blob = rnc_wrap(2, M2_STREAM, b"ABCABC");
    for pos in 0..blob.len() {
        for byte in 0..=u8::MAX {
            let mut mutated = blob.clone();
            mutated[pos] = byte;
            let _ = Rnc.try_unpack(&mutated);
        }
    }
}
