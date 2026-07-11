//! Capstone M68K op_str token resolution for xrefs/writer_at. The exact
//! output grammar is pinned in P14.md §3.4 (compiled-spike capture).

use crate::analyze::disasm::Insn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefMode {
    AbsL,
    AbsW,
    PcDisp,
    PcIdx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefAccess {
    Read,
    Write,
    Ea,
    Exec,
}

/// Split an op_str on ", " at parenthesis depth 0 — `$20012(pc, d0.w), a0`
/// is TWO operands.
pub(crate) fn split_operands(ops: &str) -> Vec<&str> {
    if ops.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in ops.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(ops[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(ops[start..].trim());
    out
}

/// All statically-resolvable address references in one instruction:
/// (24-bit bus address, mode, operand index).
pub(crate) fn resolve_operands(insn: &Insn) -> Vec<(u32, RefMode, usize)> {
    split_operands(&insn.ops)
        .into_iter()
        .enumerate()
        .filter_map(|(i, tok)| resolve_one(tok).map(|(addr, mode)| (addr, mode, i)))
        .collect()
}

/// One operand token: `$…l` → AbsL, `$…w` → AbsW, `$…(pc)` → PcDisp,
/// `$…(pc, …)` → PcIdx. Anything else (registers, `#$…` immediates,
/// `(a0)`-family) yields `None` — resolution is a filter, never an error.
fn resolve_one(tok: &str) -> Option<(u32, RefMode)> {
    let rest = tok.strip_prefix('$')?;
    if let Some(hex) = rest.strip_suffix(".l") {
        return Some((parse_hex(hex)? & 0xFF_FFFF, RefMode::AbsL));
    }
    if let Some(hex) = rest.strip_suffix(".w") {
        let v = parse_hex(hex)?;
        let addr = ((v as u16 as i16) as i32 as u32) & 0xFF_FFFF;
        return Some((addr, RefMode::AbsW));
    }
    if let Some(hex) = rest.strip_suffix("(pc)") {
        return Some((parse_hex(hex)? & 0xFF_FFFF, RefMode::PcDisp));
    }
    let idx = rest.find("(pc,")?;
    Some((parse_hex(&rest[..idx])? & 0xFF_FFFF, RefMode::PcIdx))
}

/// 1..=8 hex digits; a malformed or over-wide token is not a value (never
/// an error — the caller treats it as "does not resolve").
fn parse_hex(hex: &str) -> Option<u32> {
    if hex.is_empty() || hex.len() > 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

const READ_LAST: &[&str] = &["cmp", "cmpa", "cmpi", "cmpm", "btst", "tst", "chk"];

/// Access classification (pinned, P14.md §6.3): lea/pea → Ea; jsr/jmp →
/// Exec; a non-last operand → Read; the last operand → Read for
/// compare/test mnemonics, Write otherwise (includes read-modify-write —
/// "who writes this" is the question `xrefs` answers).
pub(crate) fn classify(insn: &Insn, op_index: usize, n_ops: usize) -> RefAccess {
    let base = insn.mnemonic.split('.').next().unwrap_or(&insn.mnemonic);
    if base == "lea" || base == "pea" {
        return RefAccess::Ea;
    }
    if base == "jsr" || base == "jmp" {
        return RefAccess::Exec;
    }
    if op_index < n_ops.saturating_sub(1) {
        return RefAccess::Read;
    }
    if READ_LAST.contains(&base) {
        return RefAccess::Read;
    }
    RefAccess::Write
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Addr;

    fn insn(mnemonic: &str, ops: &str) -> Insn {
        Insn {
            addr: Addr::new(0),
            mnemonic: mnemonic.to_string(),
            ops: ops.to_string(),
            bytes: Vec::new(),
        }
    }

    #[test]
    fn split_respects_parens() {
        assert_eq!(
            split_operands("$20012(pc, d0.w), a0"),
            vec!["$20012(pc, d0.w)", "a0"]
        );
    }

    #[test]
    fn split_single_operand() {
        assert_eq!(split_operands("$47250.l"), vec!["$47250.l"]);
    }

    #[test]
    fn resolve_abs_l() {
        let i = insn("move.l", "$47250.l, d0");
        assert_eq!(resolve_operands(&i), vec![(0x47250, RefMode::AbsL, 0)]);
    }

    #[test]
    fn resolve_abs_l_sign_extended() {
        let i = insn("move.l", "#$777, $ffff8240.l");
        assert_eq!(resolve_operands(&i), vec![(0xFF8240, RefMode::AbsL, 1)]);
    }

    #[test]
    fn resolve_abs_w_sign_extends() {
        let i = insn("move.w", "d0, $8240.w");
        assert_eq!(resolve_operands(&i), vec![(0xFF8240, RefMode::AbsW, 1)]);
    }

    #[test]
    fn resolve_pc_disp() {
        let i = insn("lea.l", "$20010(pc), a0");
        assert_eq!(resolve_operands(&i), vec![(0x20010, RefMode::PcDisp, 0)]);
    }

    #[test]
    fn resolve_pc_idx() {
        let i = insn("lea.l", "$20012(pc, d0.w), a0");
        assert_eq!(resolve_operands(&i), vec![(0x20012, RefMode::PcIdx, 0)]);
    }

    #[test]
    fn resolve_ignores_regs_and_imm() {
        let i = insn("move.l", "#$deadbeef, d0");
        assert_eq!(resolve_operands(&i), vec![]);
    }

    #[test]
    fn classify_table() {
        assert_eq!(
            classify(&insn("move.w", "d0, $1234.w"), 1, 2),
            RefAccess::Write
        );
        assert_eq!(
            classify(&insn("cmp.w", "$1234.w, d0"), 0, 2),
            RefAccess::Read
        );
        assert_eq!(
            classify(&insn("move.l", "$47250.l, d0"), 0, 2),
            RefAccess::Read
        );
        assert_eq!(
            classify(&insn("lea.l", "$20010(pc), a0"), 0, 2),
            RefAccess::Ea
        );
        assert_eq!(classify(&insn("pea.l", "$20010(pc)"), 0, 1), RefAccess::Ea);
        assert_eq!(classify(&insn("jsr", "$47250.l"), 0, 1), RefAccess::Exec);
        assert_eq!(classify(&insn("tst.w", "$47250.l"), 0, 1), RefAccess::Read);
        assert_eq!(classify(&insn("clr.w", "$47250.l"), 0, 1), RefAccess::Write);
        assert_eq!(
            classify(&insn("addq.w", "#$1, $1234.w"), 1, 2),
            RefAccess::Write
        );
    }
}
