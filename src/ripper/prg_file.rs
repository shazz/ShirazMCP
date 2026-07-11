//! GEMDOS `.PRG` container parse (P16, §4.1/§4.2): 28-byte header,
//! TEXT/DATA/symbol segments, relocation-table decode. The header layout is
//! `extract::prg_wrap`'s table — `parse` round-trips `prg_wrap` output
//! (pinned test) so emit and parse can never drift. Path-validated file
//! read follows the `disk.rs` P7 precedent.

use std::path::Path;

use crate::error::{Error, Result};

pub const HEADER_LEN: usize = 28;
pub const PRG_MAGIC: u16 = 0x601A;
/// Nothing bigger fits ST RAM (§6.1 item 3).
const MAX_SEG_LEN: u64 = 4 * 1024 * 1024;
/// Refuses to slurp arbitrary huge files (§6.1.1 item 4).
const MAX_FILE_LEN: u64 = 8 * 1024 * 1024;
/// §4.2: advance the reloc cursor by 254 with no fixup emitted.
const RELOC_SKIP_BYTE: u8 = 1;
const RELOC_SKIP_DELTA: u32 = 254;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrgHeader {
    pub tlen: u32,
    pub dlen: u32,
    pub blen: u32,
    pub slen: u32,
    pub absflag: u16,
}

/// A parsed PRG. `bytes` is the whole original file (kept: the re-emit is
/// "copy + in-place byte patches" — header, symbols and reloc table are
/// preserved verbatim by construction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrgFile {
    pub header: PrgHeader,
    pub bytes: Vec<u8>,
    /// Sorted, deduplicated segment offsets of relocated longwords (§4.2).
    pub reloc: Vec<usize>,
}

impl PrgFile {
    /// The contiguous TEXT‖DATA image (`bytes[28 .. 28 + tlen + dlen]`).
    pub fn seg_image(&self) -> &[u8] {
        &self.bytes[HEADER_LEN..HEADER_LEN + self.seg_len()]
    }

    /// tlen + dlen as usize.
    pub fn seg_len(&self) -> usize {
        self.header.tlen as usize + self.header.dlen as usize
    }
}

fn be16(bytes: &[u8], o: usize) -> Option<u16> {
    let b = bytes.get(o..o + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn be32(bytes: &[u8], o: usize) -> Option<u32> {
    let b = bytes.get(o..o + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn header_err(input: impl Into<String>) -> Error {
    Error::Parse {
        context: "prg header".to_string(),
        input: input.into(),
    }
}

fn reloc_err(input: impl Into<String>) -> Error {
    Error::Parse {
        context: "prg reloc table".to_string(),
        input: input.into(),
    }
}

fn parse_header(bytes: &[u8]) -> Result<PrgHeader> {
    let magic = be16(bytes, 0).ok_or_else(|| header_err("file shorter than the 28-byte header"))?;
    if bytes.len() < HEADER_LEN || magic != PRG_MAGIC {
        return Err(header_err(format!("bad magic ${magic:04x} or short file")));
    }
    Ok(PrgHeader {
        tlen: be32(bytes, 2).ok_or_else(|| header_err("truncated header"))?,
        dlen: be32(bytes, 6).ok_or_else(|| header_err("truncated header"))?,
        blen: be32(bytes, 10).ok_or_else(|| header_err("truncated header"))?,
        slen: be32(bytes, 14).ok_or_else(|| header_err("truncated header"))?,
        absflag: be16(bytes, 26).ok_or_else(|| header_err("truncated header"))?,
    })
}

/// Parse an in-memory PRG (§4.1 header + §4.2 reloc decode). Errors are
/// `Error::Parse` with contexts "prg header" / "prg reloc table".
pub fn parse(bytes: Vec<u8>) -> Result<PrgFile> {
    let header = parse_header(&bytes)?;
    if u64::from(header.tlen) + u64::from(header.dlen) > MAX_SEG_LEN {
        return Err(header_err("segment image exceeds 4 MB"));
    }
    let seg_end = HEADER_LEN as u64
        + u64::from(header.tlen)
        + u64::from(header.dlen)
        + u64::from(header.slen);
    if seg_end > bytes.len() as u64 {
        return Err(header_err("segments exceed file"));
    }
    let reloc = decode_reloc(&header, &bytes[seg_end as usize..])?;
    Ok(PrgFile {
        header,
        bytes,
        reloc,
    })
}

/// §4.2 relocation-table decode: `tail` starts right after the symbol
/// table. Returns sorted, deduplicated (by construction — a strictly
/// advancing cursor) segment offsets.
fn decode_reloc(header: &PrgHeader, tail: &[u8]) -> Result<Vec<usize>> {
    if header.absflag != 0 || tail.len() < 4 {
        return Ok(Vec::new());
    }
    let seg_len = u64::from(header.tlen) + u64::from(header.dlen);
    let first = be32(tail, 0).ok_or_else(|| reloc_err("truncated reloc table"))?;
    if first == 0 {
        return Ok(Vec::new());
    }
    let mut off = check_reloc_offset(first, seg_len)?;
    let mut offsets = vec![off as usize];
    let mut i = 4usize;
    loop {
        let delta = *tail
            .get(i)
            .ok_or_else(|| reloc_err("unterminated relocation table"))?;
        i += 1;
        match delta {
            0 => break,
            RELOC_SKIP_BYTE => off = off.wrapping_add(RELOC_SKIP_DELTA),
            d if d % 2 == 0 => {
                off = off.wrapping_add(u32::from(d));
                offsets.push(check_reloc_offset(off, seg_len)? as usize);
            }
            d => return Err(reloc_err(format!("odd relocation delta {d}"))),
        }
    }
    Ok(offsets)
}

fn check_reloc_offset(off: u32, seg_len: u64) -> Result<u32> {
    if !off.is_multiple_of(2) || u64::from(off) + 4 > seg_len {
        return Err(reloc_err(format!(
            "relocation offset ${off:x} out of range"
        )));
    }
    Ok(off)
}

/// Path-validated read + parse of a `.PRG` on disk (§6.1.1).
pub fn read_prg_file(path: &Path) -> Result<PrgFile> {
    // SECURITY: path traversal / unexpected-file guard.
    let canon = path.canonicalize()?;
    let meta = std::fs::symlink_metadata(&canon)?;
    if !meta.is_file() {
        return Err(Error::Parse {
            context: "prg file".to_string(),
            input: format!("{} is not a regular file", canon.display()),
        });
    }
    validate_extension(&canon)?;
    if meta.len() > MAX_FILE_LEN {
        return Err(Error::Parse {
            context: "prg file".to_string(),
            input: "file too large for an ST executable".to_string(),
        });
    }
    parse(std::fs::read(&canon)?)
}

/// Extension gate (case-insensitive): the GEMDOS-executable family.
fn validate_extension(path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("prg" | "tos" | "ttp" | "app") => Ok(()),
        _ => Err(Error::Parse {
            context: "prg file".to_string(),
            input: "not a GEMDOS executable (.prg/.tos/.ttp/.app)".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ripper::extract::prg_wrap;

    /// Header(28) + TEXT(32) + DATA(16) + no symbols + reloc table. TEXT
    /// holds a relocated longword at seg_off 8 (file value $00000010 —
    /// "points at DATA start") and recognizable filler elsewhere; the
    /// reloc table is `[00 00 00 08, 00]` (first fixup at 8, terminator).
    fn fixture_prg() -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&0x601Au16.to_be_bytes());
        f.extend_from_slice(&32u32.to_be_bytes()); // tlen
        f.extend_from_slice(&16u32.to_be_bytes()); // dlen
        f.extend_from_slice(&8u32.to_be_bytes()); // blen
        f.extend_from_slice(&0u32.to_be_bytes()); // slen
        f.extend_from_slice(&[0u8; 10]); // reserved+flags+absflag
        f.extend((0u8..32).map(|i| i ^ 0xA5)); // TEXT
        f[28 + 8..28 + 12].copy_from_slice(&0x0000_0010u32.to_be_bytes());
        f.extend((0u8..16).map(|i| 0x40 + i)); // DATA
        f.extend_from_slice(&8u32.to_be_bytes()); // reloc: first at 8
        f.push(0); // terminator
        f
    }

    fn unique_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "shirazmcp-prgfile-test-{}-{name}",
            std::process::id()
        ))
    }

    #[test]
    fn parse_round_trips_prg_wrap() {
        let prg = parse(prg_wrap(b"ABCD")).unwrap();
        assert_eq!(prg.header.tlen, 4);
        assert_eq!(prg.header.dlen, 0);
        assert_eq!(prg.header.blen, 0);
        assert_eq!(prg.header.slen, 0);
        assert_eq!(prg.header.absflag, 0);
        assert!(prg.reloc.is_empty());
        assert_eq!(prg.seg_image(), b"ABCD");
    }

    #[test]
    fn parse_fixture() {
        let prg = parse(fixture_prg()).unwrap();
        assert_eq!(prg.header.tlen, 32);
        assert_eq!(prg.header.dlen, 16);
        assert_eq!(prg.header.blen, 8);
        assert_eq!(prg.reloc, vec![8]);
        assert_eq!(prg.seg_len(), 48);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut f = fixture_prg();
        f[0] = 0x60;
        f[1] = 0x1B;
        let err = parse(f).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "prg header"));
    }

    #[test]
    fn rejects_short_file() {
        let err = parse(vec![0u8; 20]).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "prg header"));
    }

    #[test]
    fn rejects_segment_overflow() {
        let mut f = fixture_prg();
        f[2..6].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        let err = parse(f).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "prg header"));
    }

    #[test]
    fn reloc_delta_1_advances_254() {
        let mut f = Vec::new();
        f.extend_from_slice(&0x601Au16.to_be_bytes());
        f.extend_from_slice(&300u32.to_be_bytes()); // tlen
        f.extend_from_slice(&0u32.to_be_bytes()); // dlen
        f.extend_from_slice(&[0u8; 18]); // blen/slen/reserved/flags/absflag
        f.extend(std::iter::repeat_n(0xFFu8, 300)); // TEXT filler
        f.extend_from_slice(&8u32.to_be_bytes()); // first offset
        f.extend_from_slice(&[1, 2, 0]); // skip 254, fixup +2, terminator
        let prg = parse(f).unwrap();
        assert_eq!(prg.reloc, vec![8, 8 + 254 + 2]);
    }

    #[test]
    fn reloc_odd_delta_rejected() {
        let mut f = fixture_prg();
        f.truncate(28 + 48); // drop the pinned reloc table
        f.extend_from_slice(&8u32.to_be_bytes()); // first offset
        f.extend_from_slice(&[3, 0]); // odd delta, then terminator
        let err = parse(f).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "prg reloc table"));
    }

    #[test]
    fn reloc_unterminated_rejected() {
        let mut f = fixture_prg();
        f.pop(); // drop the trailing terminator byte
        let err = parse(f).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "prg reloc table"));
    }

    #[test]
    fn reloc_offset_out_of_range() {
        let mut f = fixture_prg();
        let seg_len = 48u32;
        f[28 + 48..28 + 52].copy_from_slice(&(seg_len - 2).to_be_bytes());
        let err = parse(f).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "prg reloc table"));
    }

    #[test]
    fn absflag_skips_table() {
        let mut f = fixture_prg();
        f[26..28].copy_from_slice(&1u16.to_be_bytes()); // absflag = 1
        f.extend_from_slice(&[0xDE, 0xAD]); // garbage after the segments
        let prg = parse(f).unwrap();
        assert!(prg.reloc.is_empty());
    }

    #[test]
    fn read_prg_rejects_msa_extension() {
        let path = unique_path("rejects_msa.msa");
        std::fs::write(&path, fixture_prg()).unwrap();
        let err = read_prg_file(&path).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "prg file"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_prg_rejects_directory() {
        let path = unique_path("rejects_dir");
        std::fs::create_dir_all(&path).unwrap();
        let err = read_prg_file(&path).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "prg file"));
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn read_prg_rejects_missing() {
        let path = unique_path("rejects_missing.prg");
        assert!(matches!(read_prg_file(&path), Err(Error::Io(_))));
    }

    #[test]
    fn read_prg_accepts_prg_extension() {
        let path = unique_path("accepts.prg");
        std::fs::write(&path, fixture_prg()).unwrap();
        let prg = read_prg_file(&path).unwrap();
        assert_eq!(prg.header.tlen, 32);
        let _ = std::fs::remove_file(&path);
    }
}
