use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use super::finding::Finding;

/// Byte-container formats only. PNG is a *render*, not a container — see
/// `extract_png` and the tool layer's `WireFormat` (ADR-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractFormat {
    Raw,
    Prg,
}

impl ExtractFormat {
    pub const fn ext(self) -> &'static str {
        match self {
            Self::Raw => "bin",
            Self::Prg => "prg",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Prg => "prg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub path: PathBuf,
    pub size: u64,
}

/// Standard rip filename: "rip-{seq:04}-{plugin}-{offset:06x}.{ext}"
/// (e.g. "rip-0001-toy-pic-001234.bin"). Shared by `extract` and `extract_png`.
fn rip_path(out_dir: &Path, seq: u64, finding: &Finding, ext: &str) -> PathBuf {
    out_dir.join(format!(
        "rip-{seq:04}-{}-{:06x}.{ext}",
        finding.plugin,
        finding.offset.raw()
    ))
}

/// Write one finding's bytes to `out_dir` (created on demand), return the
/// path + size — never the bytes (ADR-5: filesystem as intermediate state).
/// Synchronous `std::fs` — the tool layer wraps the call in `spawn_blocking`;
/// the engine stays sync.
pub fn extract(
    finding: &Finding,
    format: ExtractFormat,
    out_dir: &Path,
    seq: u64,
) -> Result<Extracted> {
    let bytes = finding.bytes().ok_or_else(|| Error::Ripper {
        plugin: finding.plugin,
        message: "finding has no attached source buffer".to_string(),
    })?;
    if bytes.len() > u32::MAX as usize {
        return Err(Error::Ripper {
            plugin: finding.plugin,
            message: "payload too large for PRG header".to_string(),
        });
    }
    let payload: Cow<[u8]> = match format {
        ExtractFormat::Raw => Cow::Borrowed(bytes),
        ExtractFormat::Prg => Cow::Owned(prg_wrap(bytes)),
    };
    std::fs::create_dir_all(out_dir)?;
    let path = rip_path(out_dir, seq, finding, format.ext());
    std::fs::write(&path, &payload)?;
    Ok(Extracted {
        path,
        size: payload.len() as u64,
    })
}

/// Write pre-rendered PNG bytes for `finding` under the standard rip
/// filename scheme ("rip-{seq:04}-{plugin}-{offset:06x}.png"). The caller
/// (tool layer) produced `png` via `Registry::render_png`; this function
/// only materializes it (filesystem-as-intermediate-state, ADR-5).
pub fn extract_png(finding: &Finding, png: &[u8], out_dir: &Path, seq: u64) -> Result<Extracted> {
    std::fs::create_dir_all(out_dir)?;
    let path = rip_path(out_dir, seq, finding, "png");
    std::fs::write(&path, png)?;
    Ok(Extracted {
        path,
        size: png.len() as u64,
    })
}

/// Wrap raw bytes in a minimal GEMDOS .PRG so TOS can load/run the rip —
/// the Ultimate Ripper save-as-PRG trick. Layout reference:
/// refs/orionfuzion/r1pp3r/src/R1PP3R.S (the $1c-byte header with text
/// length at +2, data length at +6, and fixup-table handling around its
/// `.exe` label) — cited for the layout ONLY, do not port any code.
///
/// Exact output = 28-byte header + payload (as TEXT) + 4-byte empty fixup
/// table, all big-endian:
///   +0  u16  0x601A          (PRG magic / bra.s)
///   +2  u32  payload.len()   (text segment length)
///   +6  u32  0               (data length)
///   +10 u32  0               (bss length)
///   +14 u32  0               (symbol table length)
///   +18 u32  0               (reserved)
///   +22 u32  0               (program flags)
///   +26 u16  0               (ABSFLAG)
///   +28      payload
///   last u32 0               (empty relocation/fixup table — the TOS
///                             loader reads the first fixup longword even
///                             with ABSFLAG conventions in play; 0 = none)
pub fn prg_wrap(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(28 + payload.len() + 4);
    out.extend_from_slice(&0x601A_u16.to_be_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&0_u32.to_be_bytes()); // data length
    out.extend_from_slice(&0_u32.to_be_bytes()); // bss length
    out.extend_from_slice(&0_u32.to_be_bytes()); // symbol table length
    out.extend_from_slice(&0_u32.to_be_bytes()); // reserved
    out.extend_from_slice(&0_u32.to_be_bytes()); // program flags
    out.extend_from_slice(&0_u16.to_be_bytes()); // ABSFLAG
    out.extend_from_slice(payload);
    out.extend_from_slice(&0_u32.to_be_bytes()); // empty fixup table
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prg_wrap_exact_layout() {
        let out = prg_wrap(b"AB");
        let expected: [u8; 34] = [
            0x60, 0x1A, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0x41, 0x42, 0, 0, 0, 0,
        ];
        assert_eq!(out, expected);
    }

    #[test]
    fn prg_wrap_empty_payload() {
        let out = prg_wrap(&[]);
        assert_eq!(out.len(), 32);
        assert_eq!(&out[2..6], &[0, 0, 0, 0]);
    }
}
