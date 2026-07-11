//! DRI/GST symbol-table parse + relocation (P26 §6.3). Cited:
//! `symbols-common.c` v2.4.1:554-690 (constants identical in HEAD:694-855)
//! — read from the `v2.4.1` git tag, NOT the tattlemuss/hrdb fork checkout
//! at /home/matt/projects/hatari HEAD (docs/plan/P26.md's Hatari-source-
//! trap warning).
//!
//! Sibling of `prg_file.rs` (same layer, same path-validation caller):
//! operates on an already-parsed `PrgFile`, never re-reads the file. Pure,
//! synchronous, no emulator dependency — `ripper` must not import `tools`,
//! so the entry type here (`PrgSymbol`/`PrgSymKind`) is distinct from
//! `tools::symbols_wire::SymEntry`; the tool layer converts.

use crate::ripper::prg_file::{PrgFile, HEADER_LEN};

const SLOT_LEN: usize = 14;
const NAME_LEN: usize = 8;
const TABTYPE_OFFSET: usize = 18;
/// `symid` bit: the GST/HiSoft extension — the next slot is 14 more name
/// characters (8 + 14 = 22-char names, exactly what vasm `-Ftos` emits for
/// long labels).
const GST_EXT_MASK: u16 = 0x0048;
const SECTION_MASK: u16 = 0x0f00;
const TEXT_BIT: u16 = 0x0200;
const DATA_BIT: u16 = 0x0400;
const BSS_BIT: u16 = 0x0100;
const DEBUG_MASK: u16 = 0xe000;
const ABS_BIT: u16 = 0x4000;
const MAX_NAME_LEN: usize = NAME_LEN + SLOT_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrgSymKind {
    Text,
    Data,
    Bss,
    Abs,
}

/// One relocated symbol from a DRI/GST table (`tools::symbols_wire`
/// converts this into its own wire-facing `SymEntry`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrgSymbol {
    pub addr: u32,
    pub kind: PrgSymKind,
    pub name: String,
}

/// Why a PRG's table couldn't be mirrored (Hatari's own `symbols` load may
/// still work — the caller degrades to `listable: false`, never a hard
/// error, per P26 §7.2 step 5).
#[derive(Debug, PartialEq, Eq)]
pub enum DriError {
    /// `slen == 0` — no symbol table in this file at all.
    NoTable,
    /// Table type (file offset 18..22) isn't DRI's `0` — MiNT a.out / ELF.
    NotDri {
        tabletype: u32,
    },
    Malformed(String),
}

/// Parse + relocate a DRI/GST symbol table per §2.5/§6.3. `bases` =
/// `[text, data, bss]` section starts (from basepage discovery — the same
/// numbers Hatari's own `update_sections` uses). Returns the relocated
/// entries plus a count of slots/entries skipped by the format rules (weak,
/// debug, out-of-section, malformed extension, `.L*`/object-file names).
pub fn parse_dri(prg: &PrgFile, bases: [u32; 3]) -> Result<(Vec<PrgSymbol>, usize), DriError> {
    if prg.header.slen == 0 {
        return Err(DriError::NoTable);
    }
    let tabletype = be32(&prg.bytes, TABTYPE_OFFSET)
        .ok_or_else(|| DriError::Malformed("file shorter than the tabletype field".to_string()))?;
    if tabletype != 0 {
        return Err(DriError::NotDri { tabletype });
    }
    let table = table_slice(prg)?;
    let (raw, skipped) = parse_slots(table);
    let lens = [prg.header.tlen, prg.header.dlen, prg.header.blen];
    Ok(relocate(raw, skipped, lens, bases))
}

/// One un-relocated symbol, still tagged with its section and raw file value.
struct RawSym {
    kind: PrgSymKind,
    raw: u32,
    name: String,
}

fn table_slice(prg: &PrgFile) -> Result<&[u8], DriError> {
    let start = HEADER_LEN + prg.header.tlen as usize + prg.header.dlen as usize;
    let end = start
        .checked_add(prg.header.slen as usize)
        .ok_or_else(|| DriError::Malformed("symbol table size overflow".to_string()))?;
    let table = prg
        .bytes
        .get(start..end)
        .ok_or_else(|| DriError::Malformed("symbol table extends past end of file".to_string()))?;
    if table.is_empty() || !table.len().is_multiple_of(SLOT_LEN) {
        return Err(DriError::Malformed(format!(
            "table size {} is not a positive multiple of {SLOT_LEN}",
            table.len()
        )));
    }
    Ok(table)
}

/// One 14-byte slot's worth of raw fields, plus whether it starts a GST
/// extension whose continuation slot doesn't exist.
struct Slot<'a> {
    name: &'a [u8],
    symid: u16,
    value: u32,
}

fn read_slot(table: &[u8], base: usize) -> Slot<'_> {
    Slot {
        name: &table[base..base + NAME_LEN],
        symid: be16_at(table, base + NAME_LEN),
        value: be32_at(table, base + NAME_LEN + 2),
    }
}

/// §6.3: walk the table, resolving GST-extended names and classifying
/// sections. Returns the accepted symbols plus a running skip count (weak,
/// debug, unrecognized-section, malformed-extension, `.L*`/object names).
fn parse_slots(table: &[u8]) -> (Vec<RawSym>, usize) {
    let n = table.len() / SLOT_LEN;
    let mut out = Vec::new();
    let mut skipped = 0usize;
    let mut slot = 0usize;
    while slot < n {
        let base = slot * SLOT_LEN;
        let head = read_slot(table, base);
        let (name_bytes, advance) = match extended_name(table, base, head.symid, slot, n) {
            Some(bytes) => (bytes, 2),
            None if head.symid & GST_EXT_MASK != 0 => {
                // Extension flagged on the LAST slot: no continuation slot
                // exists — malformed, not a whole-table error (§6.3).
                skipped += 1;
                slot += 1;
                continue;
            }
            None => (head.name.to_vec(), 1),
        };
        slot += advance;
        push_or_skip(&mut out, &mut skipped, head.symid, head.value, name_bytes);
    }
    (out, skipped)
}

fn extended_name(table: &[u8], base: usize, symid: u16, slot: usize, n: usize) -> Option<Vec<u8>> {
    if symid & GST_EXT_MASK == 0 || slot + 1 >= n {
        return None;
    }
    let mut name = table[base..base + NAME_LEN].to_vec();
    name.extend_from_slice(&table[base + SLOT_LEN..base + SLOT_LEN + SLOT_LEN]);
    Some(name)
}

fn push_or_skip(
    out: &mut Vec<RawSym>,
    skipped: &mut usize,
    symid: u16,
    value: u32,
    name_bytes: Vec<u8>,
) {
    let name = decode_name(&name_bytes);
    if name.is_empty() || is_local(&name) || is_obj_name(&name) {
        *skipped += 1;
        return;
    }
    match classify(symid) {
        Some(kind) => out.push(RawSym {
            kind,
            raw: value,
            name,
        }),
        None => *skipped += 1,
    }
}

fn classify(symid: u16) -> Option<PrgSymKind> {
    match symid & SECTION_MASK {
        TEXT_BIT => Some(PrgSymKind::Text),
        DATA_BIT => Some(PrgSymKind::Data),
        BSS_BIT => Some(PrgSymKind::Bss),
        _ if symid & DEBUG_MASK == DEBUG_MASK => None,
        _ if symid & ABS_BIT != 0 => Some(PrgSymKind::Abs),
        _ => None,
    }
}

/// NUL-terminate at the first zero byte; trim (defends against stray
/// whitespace padding some toolchains emit).
fn decode_name(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end.min(MAX_NAME_LEN)])
        .trim()
        .to_string()
}

/// v2.4.1 `no_local` option: `.L*` local labels.
fn is_local(name: &str) -> bool {
    name.starts_with(".L")
}

/// v2.4.1 `no_obj` option: object/archive file names.
fn is_obj_name(name: &str) -> bool {
    name.contains('/') || name.ends_with(".a") || name.ends_with(".o")
}

/// §2.5 `use_bssdata_offset` relocation heuristic + the post-relocation
/// section-end skip. `skipped` carries forward the parse-time skip count.
/// `lens` = `[tlen, dlen, blen]`.
fn relocate(
    raw: Vec<RawSym>,
    mut skipped: usize,
    lens: [u32; 3],
    bases: [u32; 3],
) -> (Vec<PrgSymbol>, usize) {
    let use_bssdata_offset = raw
        .iter()
        .any(|s| matches!(s.kind, PrgSymKind::Data | PrgSymKind::Bss) && s.raw < lens[0]);
    let ends = section_ends(lens, bases);
    let mut out = Vec::with_capacity(raw.len());
    for s in raw {
        let addr = relocate_one(&s, bases, use_bssdata_offset);
        if in_section(s.kind, addr, ends) {
            out.push(PrgSymbol {
                addr,
                kind: s.kind,
                name: s.name,
            });
        } else {
            skipped += 1;
        }
    }
    (out, skipped)
}

fn section_ends(lens: [u32; 3], bases: [u32; 3]) -> [u32; 3] {
    [
        bases[0].wrapping_add(lens[0]),
        bases[1].wrapping_add(lens[1]),
        bases[2].wrapping_add(lens[2]),
    ]
}

fn relocate_one(s: &RawSym, bases: [u32; 3], use_bssdata_offset: bool) -> u32 {
    match s.kind {
        PrgSymKind::Text => bases[0].wrapping_add(s.raw),
        PrgSymKind::Data if use_bssdata_offset => bases[0].wrapping_add(s.raw),
        PrgSymKind::Data => bases[1].wrapping_add(s.raw),
        PrgSymKind::Bss if use_bssdata_offset => bases[0].wrapping_add(s.raw),
        PrgSymKind::Bss => bases[2].wrapping_add(s.raw),
        PrgSymKind::Abs => s.raw,
    }
}

fn in_section(kind: PrgSymKind, addr: u32, ends: [u32; 3]) -> bool {
    match kind {
        PrgSymKind::Text => addr < ends[0],
        PrgSymKind::Data => addr < ends[1],
        PrgSymKind::Bss => addr < ends[2],
        PrgSymKind::Abs => true,
    }
}

fn be32(bytes: &[u8], o: usize) -> Option<u32> {
    let b = bytes.get(o..o + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be16_at(bytes: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([bytes[o], bytes[o + 1]])
}

fn be32_at(bytes: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ripper::prg_file::{PrgFile, PrgHeader, PRG_MAGIC};

    /// Builds a minimal `PrgFile` around a hand-assembled symbol table:
    /// `tlen`/`dlen`/`blen` describe the (unused-content) TEXT/DATA/BSS
    /// sections, `tabletype` sits at file offset 18, and `slots` is the raw
    /// symbol-table bytes appended after the (zero-filled) TEXT+DATA image.
    fn fixture(tlen: u32, dlen: u32, blen: u32, tabletype: u32, slots: &[u8]) -> PrgFile {
        let mut bytes = vec![0u8; HEADER_LEN];
        bytes[0..2].copy_from_slice(&PRG_MAGIC.to_be_bytes());
        bytes[2..6].copy_from_slice(&tlen.to_be_bytes());
        bytes[6..10].copy_from_slice(&dlen.to_be_bytes());
        bytes[10..14].copy_from_slice(&blen.to_be_bytes());
        bytes[14..18].copy_from_slice(&(slots.len() as u32).to_be_bytes());
        bytes[18..22].copy_from_slice(&tabletype.to_be_bytes());
        bytes.extend(std::iter::repeat_n(0u8, tlen as usize + dlen as usize));
        bytes.extend_from_slice(slots);
        PrgFile {
            header: PrgHeader {
                tlen,
                dlen,
                blen,
                slen: slots.len() as u32,
                absflag: 0,
            },
            bytes,
            reloc: Vec::new(),
        }
    }

    /// One plain 14-byte slot: 8-char name (NUL-padded), symid (BE),
    /// value (BE).
    fn slot(name8: &str, symid: u16, value: u32) -> Vec<u8> {
        let mut b = vec![0u8; NAME_LEN];
        let n = name8.as_bytes();
        b[..n.len().min(NAME_LEN)].copy_from_slice(&n[..n.len().min(NAME_LEN)]);
        b.extend_from_slice(&symid.to_be_bytes());
        b.extend_from_slice(&value.to_be_bytes());
        b
    }

    const TEXT_SYM: u16 = 0x0200;
    const DATA_SYM: u16 = 0x0400;
    const BSS_SYM: u16 = 0x0100;
    const ABS_SYM: u16 = 0x4000;
    const DEBUG_SYM: u16 = 0xe000;

    #[test]
    fn eight_char_text_symbol_relocates_to_text_base() {
        let table = slot("MAIN", TEXT_SYM, 0x10);
        let prg = fixture(0x1000, 0x100, 0x10, 0, &table);
        let (syms, skipped) = parse_dri(&prg, [0x70000, 0x71000, 0x71100]).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "MAIN");
        assert_eq!(syms[0].kind, PrgSymKind::Text);
        assert_eq!(syms[0].addr, 0x70010);
    }

    #[test]
    fn gst_extension_gives_22_char_name() {
        let mut table = slot("BlitSphe", GST_EXT_MASK | TEXT_SYM, 0x20);
        // Continuation slot: its whole 14 bytes are name characters.
        let mut cont = b"reFunction\0\0\0\0".to_vec();
        cont.truncate(SLOT_LEN);
        table.extend_from_slice(&cont);
        let prg = fixture(0x1000, 0, 0, 0, &table);
        let (syms, skipped) = parse_dri(&prg, [0x70000, 0x70000, 0x70000]).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(syms[0].name, "BlitSphereFunction");
    }

    #[test]
    fn one_symbol_per_section_nibble() {
        let mut table = slot("TXT", TEXT_SYM, 0x10);
        table.extend(slot("DAT", DATA_SYM, 0x0));
        table.extend(slot("BSSV", BSS_SYM, 0x0));
        let prg = fixture(0x100, 0x100, 0x100, 0, &table);
        let (syms, skipped) = parse_dri(&prg, [0x70000, 0x70100, 0x70200]).unwrap();
        assert_eq!(skipped, 0);
        let kinds: Vec<_> = syms.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![PrgSymKind::Text, PrgSymKind::Data, PrgSymKind::Bss]
        );
    }

    #[test]
    fn debug_symbol_is_skipped() {
        let table = slot("DBG", DEBUG_SYM, 0x0);
        let prg = fixture(0x100, 0, 0, 0, &table);
        let (syms, skipped) = parse_dri(&prg, [0x70000, 0x70000, 0x70000]).unwrap();
        assert!(syms.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn abs_symbol_is_not_relocated() {
        let table = slot("STACKTOP", ABS_SYM, 0xFF_0000);
        let prg = fixture(0x100, 0, 0, 0, &table);
        let (syms, skipped) = parse_dri(&prg, [0x70000, 0x70000, 0x70000]).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(syms[0].addr, 0xFF_0000);
        assert_eq!(syms[0].kind, PrgSymKind::Abs);
    }

    #[test]
    fn use_bssdata_offset_text_relative_branch() {
        // A DATA symbol raw value below tlen -> whole table is TEXT-relative.
        let mut table = slot("DVAR", DATA_SYM, 0x50); // < tlen (0x100)
        table.extend(slot("BVAR", BSS_SYM, 0x60));
        let prg = fixture(0x100, 0x100, 0x100, 0, &table);
        let (syms, _) = parse_dri(&prg, [0x70000, 0x80000, 0x90000]).unwrap();
        // Both DATA and BSS get the TEXT base, not their own section base.
        assert_eq!(syms[0].addr, 0x70050);
        assert_eq!(syms[1].addr, 0x70060);
    }

    #[test]
    fn use_bssdata_offset_section_relative_branch() {
        // Every DATA/BSS raw value >= tlen (0x100) -> section-relative (the
        // normal case). dlen/blen are widened to 0x200 so a raw value of
        // 0x150 is both >= tlen and still inside its own section.
        let mut table = slot("DVAR", DATA_SYM, 0x150);
        table.extend(slot("BVAR", BSS_SYM, 0x150));
        let prg = fixture(0x100, 0x200, 0x200, 0, &table);
        let (syms, _) = parse_dri(&prg, [0x70000, 0x80000, 0x90000]).unwrap();
        assert_eq!(syms[0].addr, 0x80150);
        assert_eq!(syms[1].addr, 0x90150);
    }

    #[test]
    fn out_of_section_symbol_is_skipped() {
        let table = slot("BAD", TEXT_SYM, 0x5000); // tlen is only 0x100
        let prg = fixture(0x100, 0, 0, 0, &table);
        let (syms, skipped) = parse_dri(&prg, [0x70000, 0x70000, 0x70000]).unwrap();
        assert!(syms.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn not_dri_tabletype_is_rejected() {
        let table = slot("X", TEXT_SYM, 0);
        // A nonzero tabletype (e.g. a MiNT/ELF magic) is not DRI.
        let prg = fixture(0x100, 0, 0, 0x1234_5678, &table);
        let err = parse_dri(&prg, [0, 0, 0]).unwrap_err();
        assert_eq!(
            err,
            DriError::NotDri {
                tabletype: 0x1234_5678
            }
        );
    }

    #[test]
    fn malformed_table_size_is_rejected() {
        let mut table = slot("X", TEXT_SYM, 0);
        table.push(0xFF); // one extra byte breaks the %14 invariant
        let prg = fixture(0x100, 0, 0, 0, &table);
        assert!(matches!(
            parse_dri(&prg, [0, 0, 0]),
            Err(DriError::Malformed(_))
        ));
    }

    #[test]
    fn no_table_when_slen_zero() {
        let prg = fixture(0x100, 0, 0, 0, &[]);
        assert_eq!(parse_dri(&prg, [0, 0, 0]), Err(DriError::NoTable));
    }

    #[test]
    fn extension_flag_on_last_slot_is_skipped_not_fatal() {
        // GST extension bit set, but this is the only (last) slot — no
        // continuation slot exists.
        let table = slot("TRUNCATE", GST_EXT_MASK | TEXT_SYM, 0);
        let prg = fixture(0x100, 0, 0, 0, &table);
        let (syms, skipped) = parse_dri(&prg, [0, 0, 0]).unwrap();
        assert!(syms.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn local_and_object_names_are_filtered() {
        let mut table = slot(".L0001", TEXT_SYM, 0x10);
        table.extend(slot("crt0.o\0\0", TEXT_SYM, 0x20));
        let prg = fixture(0x100, 0, 0, 0, &table);
        let (syms, skipped) = parse_dri(&prg, [0x70000, 0x70000, 0x70000]).unwrap();
        assert!(syms.is_empty());
        assert_eq!(skipped, 2);
    }
}
