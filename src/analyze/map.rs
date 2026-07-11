//! Parser + index over the ST hardware/system-variable memory map. Dataset
//! provenance: verbatim copy of the `atari-st-hardware-map` skill's
//! `reference/memory_map.txt` (1465 lines), embedded via `include_str!` and
//! parsed once at runtime behind a `LazyLock` — no build script (nothing to
//! generate), no runtime file I/O (no path fragility), no `const` parser
//! (the format is too loose to be worth one). This is the recorded P1
//! choice; do not revisit without an ADR.

use std::sync::LazyLock;

const DATASET: &str = include_str!("memory_map.txt");

/// One dataset row. `group` is the nearest preceding section header
/// (trimmed of '#' fencing and whitespace), e.g. "CPU Exception Vectors".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapEntry {
    pub addr: u32,
    pub group: &'static str,
    pub desc: &'static str,
    pub size: Option<&'static str>,
}

static MAP: LazyLock<Vec<MapEntry>> = LazyLock::new(parse_dataset);

fn parse_dataset() -> Vec<MapEntry> {
    let mut entries = Vec::new();
    let mut group: &'static str = "";
    for line in DATASET.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let header = line.trim_matches(|c: char| c == '#' || c.is_whitespace());
            if !header.is_empty() {
                group = header;
            }
        } else if trimmed.starts_with('$') {
            if let Some(entry) = parse_row(trimmed, group) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by_key(|e| e.addr);
    entries.dedup_by_key(|e| e.addr); // keeps the FIRST of each run (stable sort above)
    entries
}

fn parse_row(trimmed: &'static str, group: &'static str) -> Option<MapEntry> {
    let fields: Vec<&'static str> = trimmed.split('|').collect();
    let addr = u32::from_str_radix(fields.first()?.strip_prefix('$')?, 16).ok()?;
    let (size, desc) = match fields.len() {
        n if n >= 4 => (non_empty(fields[1].trim()), fields[2].trim_end()),
        3 => (None, fields[1].trim_end()),
        _ => return None,
    };
    Some(MapEntry {
        addr,
        group,
        desc,
        size,
    })
}

fn non_empty(s: &'static str) -> Option<&'static str> {
    (!s.is_empty()).then_some(s)
}

/// Exact-address lookup (binary search). `addr` must already be 24-bit-
/// masked by the caller.
pub fn lookup(addr: u32) -> Option<MapEntry> {
    MAP.binary_search_by_key(&addr, |e| e.addr)
        .ok()
        .map(|i| MAP[i])
}

/// Word-register fallback: if `addr` is odd and `addr - 1` hits an entry
/// with `size == Some("word")`, return that entry (callers label it as the
/// low byte).
pub fn lookup_word_base(addr: u32) -> Option<MapEntry> {
    if addr.is_multiple_of(2) {
        return None;
    }
    let base = lookup(addr.checked_sub(1)?)?;
    (base.size == Some("word")).then_some(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_loads_and_is_sorted() {
        let entry = lookup(0x000004).unwrap();
        assert!(entry.desc.contains("Initial PC"));
        assert!(MAP.len() > 300);
    }

    #[test]
    fn map_exact_hit_ff8800() {
        let entry = lookup(0xFF8800).unwrap();
        assert_eq!(entry.desc, "Read data/Register select");
    }

    #[test]
    fn map_word_base_fallback() {
        let entry = lookup_word_base(0xFF8241).unwrap();
        assert_eq!(entry.addr, 0xFF8240);
        assert_eq!(entry.size, Some("word"));
    }
}
