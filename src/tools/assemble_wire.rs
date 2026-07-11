//! Wire schema + pure helpers for `assemble`/`patch` (P8, ADR-18) — internal
//! to the tool layer, unit-tested without vasm or a session.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::insn_out::{insns_out, InsnOut};
use super::mem::scan::hex_encode;
use crate::analyze;
use crate::assemble::AsmResult;
use crate::types::{Addr, MemImage};

pub const PATCH_MAX_BYTES: usize = 4096;
pub const READBACK_MAX_INSNS: usize = 64;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AssembleArgs {
    /// 68000 source, vasm Motorola syntax (labels, $hex, move.w/.l, dc.b…).
    pub source: String,
    /// Load address for absolute label math ("$70000"). When given, the
    /// source must not contain its own org/rorg. Omit for position-free code.
    pub org: Option<String>,
    /// true → also return the assembler listing (addresses + bytes + source).
    pub listing: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AssembleOutput {
    /// false = the SOURCE failed to assemble; fix it using `errors` and retry.
    pub ok: bool,
    /// Machine code, contiguous lowercase hex ("" when !ok).
    pub bytes_hex: String,
    /// Byte length of the assembled code.
    pub length: u32,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub listing: Option<String>,
}

impl AssembleOutput {
    pub(crate) fn from_result(r: AsmResult) -> Self {
        Self {
            ok: r.ok,
            length: r.bytes.len() as u32,
            bytes_hex: hex_encode(&r.bytes),
            errors: r.errors,
            warnings: r.warnings,
            listing: r.listing,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PatchArgs {
    /// Even target address in ST RAM ("$70000"). org is forced to this.
    pub addr: String,
    /// 68000 source (no org/rorg — the server injects `org addr`).
    pub source: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PatchOutput {
    /// assembled AND written AND readback == assembled bytes.
    pub ok: bool,
    /// false = assembly failed (see errors); nothing was written.
    pub assembled: bool,
    /// Readback bytes equal assembled bytes. false after a write means the
    /// destination did not take the bytes (ROM/unmapped) — nothing to retry.
    pub verified: bool,
    pub addr: String,
    /// Bytes written (0 when !assembled).
    pub length: u32,
    /// addr + length — handy for the next patch or a breakpoint.
    pub end_addr: String,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Disassembly of what is ACTUALLY in RAM after the write (Capstone,
    /// with cycle estimates + hardware annotations). Empty when !assembled.
    pub readback: Vec<InsnOut>,
    /// true when the region decodes to more than 64 instructions (bytes are
    /// still fully verified; use the disasm tool to page through the rest).
    pub readback_truncated: bool,
}

impl PatchOutput {
    /// The !assembled shape: everything zero/empty, errors carried over.
    pub(crate) fn failed_assembly(addr: Addr, errors: Vec<String>, warnings: Vec<String>) -> Self {
        Self {
            ok: false,
            assembled: false,
            verified: false,
            addr: addr.to_string(),
            length: 0,
            end_addr: addr.to_string(),
            errors,
            warnings,
            readback: Vec::new(),
            readback_truncated: false,
        }
    }
}

/// Pure testability seam: disassemble the readback region. Decodes with
/// `analyze::disasm` over `mem` (base == addr), keeps insns while
/// `insn.addr < addr+len`, caps at `READBACK_MAX_INSNS` (second bool =
/// truncated). Decode failure (cannot happen for an even addr inside `mem`,
/// but degrade anyway) → `(vec![], false)`.
pub(crate) fn readback_listing(mem: &MemImage, addr: Addr, len: usize) -> (Vec<InsnOut>, bool) {
    let want = len / 2 + 1;
    let Ok(insns) = analyze::disasm(mem, addr, want) else {
        return (Vec::new(), false);
    };
    let end = addr.raw() + len as u32;
    let mut in_range: Vec<_> = insns
        .into_iter()
        .take_while(|i| i.addr.raw() < end)
        .collect();
    let truncated = in_range.len() > READBACK_MAX_INSNS;
    in_range.truncate(READBACK_MAX_INSNS);
    let (out, _) = insns_out(&in_range, addr);
    (out, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_result_maps() {
        let r = AsmResult {
            ok: true,
            bytes: vec![0x4e, 0x75],
            errors: Vec::new(),
            warnings: Vec::new(),
            listing: None,
        };
        let out = AssembleOutput::from_result(r);
        assert!(out.ok);
        assert_eq!(out.bytes_hex, "4e75");
        assert_eq!(out.length, 2);
    }

    #[test]
    fn from_result_not_ok() {
        let r = AsmResult {
            ok: false,
            bytes: Vec::new(),
            errors: vec!["undefined symbol".to_string()],
            warnings: Vec::new(),
            listing: None,
        };
        let out = AssembleOutput::from_result(r);
        assert!(!out.ok);
        assert_eq!(out.bytes_hex, "");
        assert_eq!(out.length, 0);
        assert_eq!(out.errors, vec!["undefined symbol".to_string()]);
    }

    fn region() -> MemImage {
        MemImage::new(
            vec![0x31, 0xFC, 0x07, 0x77, 0x82, 0x40, 0x4E, 0x75],
            Addr::new(0x70000),
        )
    }

    #[test]
    fn readback_lists_region() {
        let mem = region();
        let (out, truncated) = readback_listing(&mem, Addr::new(0x70000), 8);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].mnemonic, "move.w");
        assert_eq!(out[1].mnemonic, "rts");
        assert!(!truncated);
        assert!(out[0].cycles_est.is_some());
        assert!(out[0].annotations.iter().any(|a| a.addr == "$FF8240"));
    }

    #[test]
    fn readback_stops_at_len() {
        let mem = region();
        let (out, truncated) = readback_listing(&mem, Addr::new(0x70000), 6);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].mnemonic, "move.w");
        assert!(!truncated);
    }

    #[test]
    fn readback_caps_at_64() {
        let bytes = std::iter::repeat_n([0x4E, 0x71], 200).flatten().collect();
        let mem = MemImage::new(bytes, Addr::new(0x70000));
        let (out, truncated) = readback_listing(&mem, Addr::new(0x70000), 400);
        assert_eq!(out.len(), 64);
        assert!(truncated);
    }

    #[test]
    fn failed_assembly_shape() {
        let out = PatchOutput::failed_assembly(
            Addr::new(0x70000),
            vec!["assembled to 0 bytes — nothing to write".to_string()],
            Vec::new(),
        );
        assert!(!out.ok);
        assert!(!out.assembled);
        assert!(!out.verified);
        assert_eq!(out.length, 0);
        assert!(out.readback.is_empty());
        assert_eq!(out.addr, "$070000");
    }
}
