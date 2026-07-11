//! Wire schema + pure helpers for the `symbols` tool (P26 §6.1/§6.2/§7.1):
//! the server-side listing store, the nm-ASCII map parser (mirrors Hatari's
//! `symbols_load_ascii`, v2.4.1 symbols.c:63-170 — cited from the v2.4.1
//! git tag, not the tattlemuss/hrdb fork checkout), and args/output types.
//! `symbols.rs` (the tool itself) is choreography; everything pure lives
//! here, the same "wire.rs" split as `rip_wire.rs`/`analyze_wire.rs`.

use std::path::PathBuf;

use rmcp::ErrorData as McpError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ripper::prg_symbols::{PrgSymKind, PrgSymbol};
use crate::types::Addr;

pub(crate) const LIST_DEFAULT: u32 = 32;
pub(crate) const LIST_MAX: u32 = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SymKind {
    Text,
    Data,
    Bss,
    Abs,
}

#[derive(Debug, Clone)]
pub struct SymEntry {
    pub addr: u32,
    pub kind: SymKind,
    pub name: String,
}

/// Server-side listing mirror (P26 §3 — the listing authority; Hatari
/// itself stays the sole NAME-RESOLUTION authority via `resolve_addr`).
/// Rebuilt on every successful `load`, cleared on `free`/`stop`.
///
/// `source`/`text_base`/`data_base`/`bss_base`/`skipped` are the §6.1-
/// pinned provenance fields — already surfaced once in `LoadedOut` at load
/// time, so `action=list` (the only other reader) doesn't need them again;
/// kept on the struct as the deliberate schema shape for future diagnostic
/// surfaces (e.g. echoing bases alongside a listing) rather than trimmed
/// and re-added later.
#[allow(
    dead_code,
    reason = "P26 §6.1 store provenance fields, not yet read back by any tool output"
)]
#[derive(Debug, Clone)]
pub struct SymbolStore {
    /// What was loaded, for display: `"prg:/host/path.prg"` | `"file:/path.sym"`.
    pub source: String,
    pub text_base: u32,
    pub data_base: u32,
    pub bss_base: u32,
    pub entries: Vec<SymEntry>,
    /// Lines/slots skipped by the format rules (weak, debug, malformed…).
    pub skipped: usize,
}

fn prg_kind(k: PrgSymKind) -> SymKind {
    match k {
        PrgSymKind::Text => SymKind::Text,
        PrgSymKind::Data => SymKind::Data,
        PrgSymKind::Bss => SymKind::Bss,
        PrgSymKind::Abs => SymKind::Abs,
    }
}

/// `ripper` must not import `tools` (layering), so the DRI parser returns
/// its own `PrgSymbol` — this is the one conversion point into the wire
/// entry type.
pub(crate) fn from_prg_symbols(syms: Vec<PrgSymbol>) -> Vec<SymEntry> {
    syms.into_iter()
        .map(|s| SymEntry {
            addr: s.addr,
            kind: prg_kind(s.kind),
            name: s.name,
        })
        .collect()
}

/// Mirror of Hatari v2.4.1 `symbols_load_ascii` (§2.4) so the store matches
/// what Hatari loaded from the SAME file with the SAME offsets. `Err` means
/// the whole file is rejected, exactly like Hatari's own loader.
pub(crate) fn parse_ascii_map(
    text: &str,
    offsets: [u32; 3],
) -> std::result::Result<(Vec<SymEntry>, usize), String> {
    let mut entries = Vec::new();
    let mut skipped = 0usize;
    for (lineno, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let Some(first) = trimmed.chars().next() else {
            continue; // blank line
        };
        if first == '#' || first == '*' {
            continue;
        }
        if !first.is_ascii_hexdigit() {
            return Err(format!(
                "line {} doesn't start with an address: {line:?}",
                lineno + 1
            ));
        }
        match parse_ascii_line(trimmed, offsets) {
            Some(entry) => entries.push(entry),
            None => skipped += 1,
        }
    }
    Ok((entries, skipped))
}

/// One already-non-blank, non-comment, hex-starting line. `None` covers
/// every §2.4 skip case: malformed token shape, `W`/`V` weak symbols,
/// unrecognized type chars, and the post-offset `> 0xFFFFFF` bound.
fn parse_ascii_line(line: &str, offsets: [u32; 3]) -> Option<SymEntry> {
    let mut tokens = line.split_whitespace();
    let addr_raw = parse_hex_token(tokens.next()?)?;
    let type_ch = tokens.next()?.chars().next()?;
    let name_tok = tokens.next()?;
    let (kind, off) = classify_ascii_type(type_ch, offsets)?;
    let name: String = name_tok
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        .take(32)
        .collect();
    // Hatari does unsigned C addition (u32::wrapping_add).
    let addr = addr_raw.wrapping_add(off);
    if addr > Addr::BUS_LEN - 1 {
        return None;
    }
    Some(SymEntry { addr, kind, name })
}

/// §2.4 type-char table (case-insensitive `toupper`): each accepted type
/// maps to a section and which of `offsets` (`[text, data, bss]`) applies
/// — `A` (ABS) gets no offset. `W`/`V` (weak) and anything else return
/// `None`, folded into the caller's generic skip path.
fn classify_ascii_type(c: char, offsets: [u32; 3]) -> Option<(SymKind, u32)> {
    match c.to_ascii_uppercase() {
        'T' => Some((SymKind::Text, offsets[0])),
        'O' | 'R' | 'D' => Some((SymKind::Data, offsets[1])),
        'B' => Some((SymKind::Bss, offsets[2])),
        'A' => Some((SymKind::Abs, 0)),
        _ => None,
    }
}

fn parse_hex_token(tok: &str) -> Option<u32> {
    let digits = tok
        .strip_prefix("0x")
        .or_else(|| tok.strip_prefix("0X"))
        .unwrap_or(tok);
    u32::from_str_radix(digits, 16).ok()
}

/// Filter (case-insensitive substring on name) → sort (code/data: by addr
/// over that kind subset; all: by name) → skip `offset` → take `limit`.
/// `total_matches` is the count BEFORE skip/take (§7.1).
pub(crate) fn list_symbols(
    store: &SymbolStore,
    kind: SymListKind,
    find: Option<&str>,
    limit: u32,
    offset: u32,
) -> (Vec<SymbolOut>, usize) {
    let matches = matching_entries(store, kind, find);
    let total = matches.len();
    let rows = matches
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|e| SymbolOut {
            addr: Addr::new(e.addr).to_string(),
            kind: e.kind,
            name: e.name.clone(),
        })
        .collect();
    (rows, total)
}

fn matching_entries<'a>(
    store: &'a SymbolStore,
    kind: SymListKind,
    find: Option<&str>,
) -> Vec<&'a SymEntry> {
    let needle = find.map(str::to_ascii_lowercase);
    let mut matches: Vec<&SymEntry> = store
        .entries
        .iter()
        .filter(|e| kind_matches(kind, e.kind))
        .filter(|e| {
            needle
                .as_deref()
                .is_none_or(|n| e.name.to_ascii_lowercase().contains(n))
        })
        .collect();
    match kind {
        SymListKind::All => matches.sort_by(|a, b| a.name.cmp(&b.name)),
        SymListKind::Code | SymListKind::Data => matches.sort_by_key(|e| e.addr),
    }
    matches
}

fn kind_matches(list_kind: SymListKind, sym_kind: SymKind) -> bool {
    match list_kind {
        SymListKind::All => true,
        SymListKind::Code => sym_kind == SymKind::Text,
        SymListKind::Data => matches!(sym_kind, SymKind::Data | SymKind::Bss | SymKind::Abs),
    }
}

/// SECURITY: path traversal / unexpected-file guard, the `prg_file::
/// read_prg_file`/`save_prg_wire::validate_paths` read-side precedent.
/// Requires an absolute path (§7.2 step 2).
pub(crate) fn validate_symbols_path(raw: &str) -> Result<PathBuf, McpError> {
    let inv = |m: String| McpError::invalid_params(m, None);
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(inv(format!("path must be absolute: {raw}")));
    }
    let canon = path
        .canonicalize()
        .map_err(|_| inv(format!("path does not exist: {raw}")))?;
    let meta = std::fs::symlink_metadata(&canon)
        .map_err(|_| inv(format!("path does not exist: {raw}")))?;
    if !meta.is_file() {
        return Err(inv(format!("{raw} is not a regular file")));
    }
    Ok(canon)
}

pub(crate) fn validate_offset_args(a: &SymbolsArgs) -> Result<(), McpError> {
    let any_offset = a.text_offset.is_some() || a.data_offset.is_some() || a.bss_offset.is_some();
    if any_offset && a.path.is_none() {
        return Err(McpError::invalid_params(
            "offsets apply to ASCII symbol files — pass path",
            None,
        ));
    }
    if (a.data_offset.is_some() || a.bss_offset.is_some()) && a.text_offset.is_none() {
        return Err(McpError::invalid_params(
            "data/bss offsets require text_offset",
            None,
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SymbolsArgs {
    pub action: SymbolsAction,
    /// load: host path to an nm-style ASCII map or a .PRG with a DRI/GST
    /// symbol table. Omit to load from the running GEMDOS-HD program.
    pub path: Option<String>,
    /// load(ASCII only): explicit TEXT offset ("$70000"/decimal). Omitting
    /// it selects auto-detection from the GEMDOS basepage. Rejected for
    /// .PRG sources (Hatari relocates those itself).
    pub text_offset: Option<String>,
    /// load(ASCII only): DATA offset; defaults to text_offset when that is
    /// given, else auto-detected.
    pub data_offset: Option<String>,
    /// load(ASCII only): BSS offset; same defaulting as data_offset.
    pub bss_offset: Option<String>,
    /// list: "code" (TEXT, by address) | "data" (DATA/BSS/ABS, by address)
    /// | "all" (default, by name).
    pub kind: Option<SymListKind>,
    /// list: case-insensitive substring filter — the primary interface for
    /// big maps; prefer it over paging.
    pub find: Option<String>,
    /// list: max rows (default 32, cap 128).
    pub limit: Option<u32>,
    /// list: skip this many matches first (pagination cursor).
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SymbolsAction {
    Load,
    List,
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SymListKind {
    Code,
    Data,
    All,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SymbolsOutput {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded: Option<LoadedOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbols: Option<Vec<SymbolOut>>,
    /// list: total matches before limit/offset (so the agent knows to
    /// narrow `find` instead of paging a 4000-symbol table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_matches: Option<usize>,
    pub state: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LoadedOut {
    pub source: String,
    pub total: u32,
    pub text_symbols: u32,
    pub text_base: String,
    pub data_base: String,
    pub bss_base: String,
    /// "basepage" | "explicit" | "hatari".
    pub base_source: String,
    /// Server-side mirror for `list` — false + `note` when the mirror
    /// could not be built (a.out table, unreadable file…). Name resolution
    /// via `resolve_addr` still works whenever Hatari loaded the table.
    pub listable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SymbolOut {
    pub addr: String,
    pub kind: SymKind,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(addr: u32, kind: SymKind, name: &str) -> SymEntry {
        SymEntry {
            addr,
            kind,
            name: name.to_string(),
        }
    }

    fn store(entries: Vec<SymEntry>) -> SymbolStore {
        SymbolStore {
            source: "file:/x.sym".to_string(),
            text_base: 0,
            data_base: 0,
            bss_base: 0,
            entries,
            skipped: 0,
        }
    }

    // ---- parse_ascii_map: §2.4 rule coverage ----

    #[test]
    fn text_offset_applies_to_t() {
        let (entries, skipped) = parse_ascii_map("0 T main\n", [0x70000, 0, 0]).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(entries[0].addr, 0x70000);
        assert_eq!(entries[0].kind, SymKind::Text);
        assert_eq!(entries[0].name, "main");
    }

    #[test]
    fn data_types_o_r_d_use_data_offset() {
        for ch in ['O', 'R', 'D', 'o', 'r', 'd'] {
            let line = format!("10 {ch} var\n");
            let (entries, _) = parse_ascii_map(&line, [0, 0x80000, 0]).unwrap();
            assert_eq!(entries[0].addr, 0x80010, "type char {ch}");
            assert_eq!(entries[0].kind, SymKind::Data);
        }
    }

    #[test]
    fn bss_type_uses_bss_offset() {
        let (entries, _) = parse_ascii_map("20 B buf\n", [0, 0, 0x90000]).unwrap();
        assert_eq!(entries[0].addr, 0x90020);
        assert_eq!(entries[0].kind, SymKind::Bss);
    }

    #[test]
    fn abs_type_ignores_offsets() {
        let (entries, _) =
            parse_ascii_map("ff0000 A stacktop\n", [0x70000, 0x70000, 0x70000]).unwrap();
        assert_eq!(entries[0].addr, 0xff0000);
        assert_eq!(entries[0].kind, SymKind::Abs);
    }

    #[test]
    fn weak_w_and_v_are_skip_counted() {
        let (entries, skipped) = parse_ascii_map("0 W weak1\n0 V weak2\n", [0, 0, 0]).unwrap();
        assert!(entries.is_empty());
        assert_eq!(skipped, 2);
    }

    #[test]
    fn unrecognized_type_is_skip_counted() {
        let (entries, skipped) = parse_ascii_map("0 Z mystery\n", [0, 0, 0]).unwrap();
        assert!(entries.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn name_truncates_at_32_chars() {
        let long_name = "n".repeat(40);
        let line = format!("0 T {long_name}\n");
        let (entries, _) = parse_ascii_map(&line, [0, 0, 0]).unwrap();
        assert_eq!(entries[0].name.len(), 32);
    }

    #[test]
    fn dot_and_dash_are_legal_name_chars() {
        let (entries, _) = parse_ascii_map("0 T foo.bar-baz\n", [0, 0, 0]).unwrap();
        assert_eq!(entries[0].name, "foo.bar-baz");
    }

    #[test]
    fn address_beyond_24bit_bus_is_skip_counted() {
        let (entries, skipped) = parse_ascii_map("0 T over\n", [0x0100_0000, 0, 0]).unwrap();
        assert!(entries.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let (entries, skipped) =
            parse_ascii_map("# comment\n* also comment\n\n0 T x\n", [0, 0, 0]).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(skipped, 0);
    }

    #[test]
    fn non_hex_start_aborts_whole_load() {
        let err = parse_ascii_map("0 T x\nnope not hex\n", [0, 0, 0]).unwrap_err();
        assert!(err.contains("line 2"));
    }

    #[test]
    fn bare_hex_without_0x_prefix_parses() {
        let (entries, _) = parse_ascii_map("70000 T x\n", [0, 0, 0]).unwrap();
        assert_eq!(entries[0].addr, 0x70000);
    }

    // ---- list_symbols ----

    #[test]
    fn list_filters_case_insensitively_and_reports_total() {
        let s = store(vec![
            entry(1, SymKind::Text, "BlitSphere"),
            entry(2, SymKind::Text, "DrawSpheres"),
            entry(3, SymKind::Data, "Unrelated"),
        ]);
        let (rows, total) = list_symbols(&s, SymListKind::All, Some("sphere"), 32, 0);
        assert_eq!(total, 2);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn list_code_kind_is_text_only_sorted_by_addr() {
        let s = store(vec![
            entry(0x200, SymKind::Text, "b"),
            entry(0x100, SymKind::Text, "a"),
            entry(0x50, SymKind::Data, "d"),
        ]);
        let (rows, total) = list_symbols(&s, SymListKind::Code, None, 32, 0);
        assert_eq!(total, 2);
        assert_eq!(rows[0].name, "a");
        assert_eq!(rows[1].name, "b");
    }

    #[test]
    fn list_data_kind_covers_data_bss_abs() {
        let s = store(vec![
            entry(1, SymKind::Data, "d"),
            entry(2, SymKind::Bss, "b"),
            entry(3, SymKind::Abs, "a"),
            entry(4, SymKind::Text, "t"),
        ]);
        let (_, total) = list_symbols(&s, SymListKind::Data, None, 32, 0);
        assert_eq!(total, 3);
    }

    #[test]
    fn list_respects_limit_and_offset() {
        let entries = (0..5)
            .map(|i| entry(i, SymKind::Text, &format!("s{i}")))
            .collect();
        let s = store(entries);
        let (rows, total) = list_symbols(&s, SymListKind::Code, None, 2, 2);
        assert_eq!(total, 5);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "s2");
    }

    // ---- validate_offset_args ----

    fn args(path: Option<&str>, t: Option<&str>, d: Option<&str>, b: Option<&str>) -> SymbolsArgs {
        SymbolsArgs {
            action: SymbolsAction::Load,
            path: path.map(str::to_string),
            text_offset: t.map(str::to_string),
            data_offset: d.map(str::to_string),
            bss_offset: b.map(str::to_string),
            kind: None,
            find: None,
            limit: None,
            offset: None,
        }
    }

    #[test]
    fn offsets_without_path_rejected() {
        let err = validate_offset_args(&args(None, Some("$1000"), None, None)).unwrap_err();
        assert!(err.message.contains("pass path"));
    }

    #[test]
    fn data_offset_without_text_offset_rejected() {
        let err =
            validate_offset_args(&args(Some("/a.sym"), None, Some("$1000"), None)).unwrap_err();
        assert!(err.message.contains("require text_offset"));
    }

    #[test]
    fn matching_offsets_accepted() {
        assert!(validate_offset_args(&args(
            Some("/a.sym"),
            Some("$1000"),
            Some("$2000"),
            Some("$3000")
        ))
        .is_ok());
    }

    #[test]
    fn no_offsets_accepted() {
        assert!(validate_offset_args(&args(None, None, None, None)).is_ok());
    }

    // ---- from_prg_symbols ----

    #[test]
    fn from_prg_symbols_maps_kind_and_fields() {
        let out = from_prg_symbols(vec![PrgSymbol {
            addr: 0x70000,
            kind: PrgSymKind::Text,
            name: "main".to_string(),
        }]);
        assert_eq!(out[0].addr, 0x70000);
        assert_eq!(out[0].kind, SymKind::Text);
        assert_eq!(out[0].name, "main");
    }
}
