//! Wire schema for the `xrefs` tool (P14 §7.5).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::mem::scan::hex_encode;
use crate::analyze::{RefAccess, RefMode, Xref};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct XrefsArgs {
    /// Target address the references point at.
    pub addr: String,
    /// Widen the target to [addr, addr+target_len) (default 2, max 64).
    pub target_len: Option<u32>,
    /// Scan-region start (default $0 = whole snapshot).
    pub from: Option<String>,
    /// Scan-region length in bytes (default: to end of RAM).
    pub len: Option<u32>,
    /// Max hits returned (default 32, max 128).
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct XrefOut {
    /// insn address "$00DB2A"
    pub addr: String,
    pub mnemonic: String,
    pub ops: String,
    pub bytes_hex: String,
    /// "abs.l" | "abs.w" | "pc" | "pc-idx"
    pub mode: String,
    /// "read" | "write" | "ea" | "exec"
    pub access: String,
    /// the resolved address that matched
    pub ref_addr: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct XrefsOutput {
    pub target: String,
    pub target_len: u32,
    pub from: String,
    pub scan_len: u32,
    pub hits: Vec<XrefOut>,
    pub truncated: bool,
    pub candidates_capped: bool,
}

impl From<Xref> for XrefOut {
    fn from(x: Xref) -> Self {
        XrefOut {
            addr: x.insn.addr.to_string(),
            mnemonic: x.insn.mnemonic,
            ops: x.insn.ops,
            bytes_hex: hex_encode(&x.insn.bytes),
            mode: mode_str(x.mode).to_string(),
            access: access_str(x.access).to_string(),
            ref_addr: x.ref_addr.to_string(),
        }
    }
}

fn mode_str(mode: RefMode) -> &'static str {
    match mode {
        RefMode::AbsL => "abs.l",
        RefMode::AbsW => "abs.w",
        RefMode::PcDisp => "pc",
        RefMode::PcIdx => "pc-idx",
    }
}

fn access_str(access: RefAccess) -> &'static str {
    match access {
        RefAccess::Read => "read",
        RefAccess::Write => "write",
        RefAccess::Ea => "ea",
        RefAccess::Exec => "exec",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::disasm::Insn;
    use crate::types::Addr;

    #[test]
    fn xref_out_maps_mode_and_access() {
        let x = Xref {
            insn: Insn {
                addr: Addr::new(0x20000),
                mnemonic: "move.l".to_string(),
                ops: "$47250.l, d0".to_string(),
                bytes: vec![0x20, 0x39],
            },
            ref_addr: Addr::new(0x47250),
            mode: RefMode::AbsL,
            access: RefAccess::Read,
        };
        let out = XrefOut::from(x);
        assert_eq!(out.mode, "abs.l");
        assert_eq!(out.access, "read");
        assert_eq!(out.bytes_hex, "2039");
    }
}
