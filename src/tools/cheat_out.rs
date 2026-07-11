//! Output side of `cheat_scan` (P15): `CheatScanOutput`/`CandidateOut`,
//! pagination, and value/predicate formatting. Split out of `cheat_wire.rs`
//! (not new logic — a straight extraction, the `insn_out.rs` precedent) to
//! stay under the file-size ceiling: with the output builders inlined,
//! `cheat.rs`/`cheat_wire.rs` measured over 200 non-test lines (ADR-29).

use schemars::JsonSchema;
use serde::Serialize;

use super::cheat_wire::CheatStore;
use crate::analyze::{read_value, CandidateSet, Predicate, Width};
use crate::types::{Addr, MemImage};

pub(crate) const SAMPLE_DEFAULT: u32 = 16;
pub(crate) const SAMPLE_MAX: u32 = 64;

#[derive(Debug, Serialize, JsonSchema)]
pub struct CandidateOut {
    pub addr: String,
    pub value: String,
    pub annotations: Vec<super::mem::types::AnnotationOut>,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct CheatScanOutput {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    /// Count before this filter step (filter only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_count: Option<u64>,
    /// Canonical predicate applied (filter only), e.g. "delta(-1)".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    /// Capped sample of survivors, ascending by address.
    pub sample: Vec<CandidateOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    pub has_more: bool,
    /// reset only: whether a search existed to clear.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleared: Option<bool>,
}

/// Shared shape for new/filter/list: action + search_id/width/step/count
/// from the store, plus this page's sample. `filter` sets `prev_count`/
/// `predicate` afterward (the two fields no other action populates);
/// `reset` doesn't read a store, so it builds its own literal.
pub(crate) fn page_output(
    action: &str,
    store: &CheatStore,
    offset: u32,
    sample: Vec<CandidateOut>,
    has_more: bool,
) -> CheatScanOutput {
    CheatScanOutput {
        action: action.to_string(),
        search_id: Some(store.search_id),
        width: Some(store.width.bytes() as u32),
        step: Some(store.step),
        count: Some(store.candidates.count()),
        sample,
        offset: Some(offset),
        has_more,
        ..Default::default()
    }
}

/// Canonical predicate echo: "changed", "eq($1f40)", "delta(-1)" (wrap-free
/// arms fall back to lowercased `Debug` — the variant names already match).
pub(crate) fn predicate_str(p: Predicate) -> String {
    match p {
        Predicate::Eq(v) => format!("eq(${v:x})"),
        Predicate::Ne(v) => format!("ne(${v:x})"),
        Predicate::Delta(n) => format!("delta({n})"),
        simple => format!("{simple:?}").to_lowercase(),
    }
}

/// "$xxxx": lowercase hex zero-padded to 2×width.bytes() digits.
pub(crate) fn format_value(v: u32, width: Width) -> String {
    let w = width.bytes() * 2;
    format!("${v:0w$x}")
}

/// One output page, ascending by address: `set.iter().skip(offset).take(limit)`
/// read against `snap`. has_more = a further `.next()` still yields Some.
pub(crate) fn page_candidates(
    set: &CandidateSet,
    snap: &MemImage,
    width: Width,
    offset: u32,
    limit: u32,
) -> (Vec<CandidateOut>, bool) {
    let (bytes, base) = (snap.bytes(), snap.base());
    let mut it = set.iter().skip(offset as usize);
    let page = it
        .by_ref()
        .take(limit as usize)
        .map(|o| candidate_out(bytes, base, o, width))
        .collect();
    (page, it.next().is_some())
}

fn candidate_out(bytes: &[u8], base: Addr, off: usize, width: Width) -> CandidateOut {
    let addr = Addr::new(base.raw() + off as u32);
    CandidateOut {
        addr: addr.to_string(),
        value: format_value(read_value(bytes, off, width), width),
        annotations: super::mem::scan::annotations_for(addr, width.bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicate_str_canonical() {
        assert_eq!(predicate_str(Predicate::Changed), "changed");
        assert_eq!(predicate_str(Predicate::Eq(0x1F40)), "eq($1f40)");
        assert_eq!(predicate_str(Predicate::Delta(-1)), "delta(-1)");
    }

    #[test]
    fn format_value_pads() {
        assert_eq!(format_value(0x4C6, Width::Word), "$04c6");
        assert_eq!(format_value(5, Width::Byte), "$05");
        assert_eq!(format_value(0xDEAD_BEEF, Width::Long), "$deadbeef");
    }

    #[test]
    fn page_candidates_paginates() {
        // Base $4B4 + offset 6 == $4BA (the `_hz_200` sysvar) — exercises
        // the annotation enrichment alongside plain pagination.
        let mut bytes = vec![0u8; 8];
        bytes[6] = 0x01;
        let snap = MemImage::new(bytes, Addr::new(0x4B4));
        let mut set = CandidateSet::new(8);
        for o in [0, 2, 4, 6, 7] {
            set.insert(o);
        }
        let (page, has_more) = page_candidates(&set, &snap, Width::Byte, 0, 2);
        assert_eq!(page.len(), 2);
        assert!(has_more);

        let (page, has_more) = page_candidates(&set, &snap, Width::Byte, 2, 2);
        assert_eq!(page.len(), 2);
        assert!(has_more);
        assert_eq!(page[1].addr, "$0004BA");
        assert_eq!(page[1].value, format_value(1, Width::Byte));
        assert!(!page[1].annotations.is_empty());

        let (page, has_more) = page_candidates(&set, &snap, Width::Byte, 4, 2);
        assert_eq!(page.len(), 1);
        assert!(!has_more);
    }
}
