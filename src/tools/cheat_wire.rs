//! Input side of `cheat_scan` (P15): wire args, the `CheatStore`, and pure
//! parse/validate helpers — internal to the tool layer, pure, unit-tested
//! without a session. Output side (`CheatScanOutput`/pagination) split into
//! `cheat_out.rs` (ADR-29).

use rmcp::ErrorData as McpError;
use schemars::JsonSchema;
use serde::Deserialize;

use super::mem::scan::parse_addr;
use crate::analyze::{CandidateSet, Predicate, SeedFilter, Width};
use crate::types::MemImage;

/// This file's `Err` channel — mapped by the tool to `invalid_params`.
type PResult<T> = Result<T, String>;

/// The one in-flight cheat search (ADR-10 pattern: single store, replaced
/// by `new`, cleared by `reset`). `snapshot` is the last acquired image.
#[derive(Debug)]
pub(crate) struct CheatStore {
    pub search_id: u64,
    pub width: Width,
    /// Narrowing steps applied so far: 0 after `new`, +1 per `filter`.
    pub step: u32,
    pub candidates: CandidateSet,
    pub snapshot: MemImage,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheatScanArgs {
    /// new|filter|list|reset (see the tool description).
    pub action: String,
    /// filter: changed|unchanged|increased|decreased|eq|ne|delta; new (optional): eq|ne only.
    pub predicate: Option<String>,
    /// eq/ne (unsigned) or delta (signed, e.g. "-1"): "$1F40"/"0x1f40"/"8000". Must fit the width.
    pub value: Option<String>,
    /// Value width in bytes: 1, 2 (default) or 4 — action=new only, fixed for the search's life.
    pub width: Option<u32>,
    /// Region bound for action=new (default: all RAM); address forms as read_memory.
    pub start: Option<String>,
    pub end: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// "new"|"filter"|"list"|"reset" (case-sensitive) → enum; else Err(String).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    New,
    Filter,
    List,
    Reset,
}

pub(crate) fn parse_action(s: &str) -> PResult<Action> {
    match s {
        "new" => Ok(Action::New),
        "filter" => Ok(Action::Filter),
        "list" => Ok(Action::List),
        "reset" => Ok(Action::Reset),
        _ => Err(format!("unknown action '{s}': use new|filter|list|reset")),
    }
}

/// §7.1 parameter-validity matrix: which `CheatScanArgs` fields `action`
/// allows. Anything present outside that set → Err naming the field; a
/// missing `predicate` on `filter` is the one required-field case.
pub(crate) fn validate_params(a: &CheatScanArgs, action: Action) -> PResult<()> {
    let name = format!("{action:?}").to_lowercase();
    let hit = forbidden_fields(a, action)
        .into_iter()
        .find(|(_, present)| *present);
    if let Some((field, _)) = hit {
        return Err(format!("{field} is forbidden for action=\"{name}\""));
    }
    (action != Action::Filter || a.predicate.is_some())
        .then_some(())
        .ok_or_else(|| "predicate is required for action=\"filter\"".to_string())
}

/// The (field name, present?) pairs `action` forbids — §7.1's matrix.
/// Owned `Vec`: the tuples embed per-call `a.field.is_some()` values.
fn forbidden_fields(a: &CheatScanArgs, action: Action) -> Vec<(&'static str, bool)> {
    match action {
        Action::New => vec![("offset", a.offset.is_some())],
        Action::Filter => vec![
            ("width", a.width.is_some()),
            ("start", a.start.is_some()),
            ("end", a.end.is_some()),
            ("offset", a.offset.is_some()),
        ],
        Action::List => vec![
            ("predicate", a.predicate.is_some()),
            ("value", a.value.is_some()),
            ("width", a.width.is_some()),
            ("start", a.start.is_some()),
            ("end", a.end.is_some()),
        ],
        Action::Reset => vec![
            ("predicate", a.predicate.is_some()),
            ("value", a.value.is_some()),
            ("width", a.width.is_some()),
            ("start", a.start.is_some()),
            ("limit", a.limit.is_some()),
            ("offset", a.offset.is_some()),
        ],
    }
}

fn split_hex(s: &str) -> Option<&str> {
    s.strip_prefix('$')
        .or(s.strip_prefix("0x"))
        .or(s.strip_prefix("0X"))
}

/// §4's value grammar: unsigned ($/0x/decimal, ≤ width.max_value()).
pub(crate) fn parse_value(s: &str, width: Width) -> PResult<u32> {
    let (digits, radix) = split_hex(s).map_or((s, 10), |h| (h, 16));
    let raw = u32::from_str_radix(digits, radix).map_err(|_| format!("invalid value '{s}'"))?;
    let max = width.max_value();
    (raw <= max)
        .then_some(raw)
        .ok_or_else(|| format!("'{s}' exceeds max ${max:X} for this width"))
}

/// Signed variant for delta (optional leading '-', |n| ≤ width.max_value()).
pub(crate) fn parse_delta(s: &str, width: Width) -> PResult<i64> {
    let (sign, rest) = s.strip_prefix('-').map_or((1i64, s), |r| (-1, r));
    Ok(sign * i64::from(parse_value(rest, width)?))
}

pub(crate) fn parse_width(w: Option<u32>) -> PResult<Width> {
    Width::from_bytes(w.unwrap_or(2)).ok_or_else(|| "width must be 1, 2 or 4".to_string())
}

/// (predicate, value) args → Predicate, enforcing §4's value rules.
pub(crate) fn predicate(pred: &str, value: Option<&str>, width: Width) -> PResult<Predicate> {
    match (pred, value) {
        ("changed", None) => Ok(Predicate::Changed),
        ("unchanged", None) => Ok(Predicate::Unchanged),
        ("increased", None) => Ok(Predicate::Increased),
        ("decreased", None) => Ok(Predicate::Decreased),
        ("changed" | "unchanged" | "increased" | "decreased", Some(_)) => {
            Err(format!("predicate '{pred}' takes no value"))
        }
        ("eq", Some(v)) => parse_value(v, width).map(Predicate::Eq),
        ("ne", Some(v)) => parse_value(v, width).map(Predicate::Ne),
        ("delta", Some(v)) => parse_delta(v, width).map(Predicate::Delta),
        ("eq" | "ne" | "delta", None) => Err(format!("predicate '{pred}' requires a value")),
        _ => Err(format!(
            "unknown predicate '{pred}': use changed|unchanged|increased|decreased|eq|ne|delta"
        )),
    }
}

/// new-action variant: eq/ne only, else Err (only eq/ne are seedable).
pub(crate) fn seed_filter(
    pred: Option<&str>,
    value: Option<&str>,
    width: Width,
) -> PResult<Option<SeedFilter>> {
    match (pred, value) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err("value given without a predicate on action=\"new\"".to_string()),
        (Some("eq"), Some(v)) => parse_value(v, width).map(|v| Some(SeedFilter::Eq(v))),
        (Some("ne"), Some(v)) => parse_value(v, width).map(|v| Some(SeedFilter::Ne(v))),
        (Some(p @ ("eq" | "ne")), None) => Err(format!("predicate '{p}' requires a value")),
        (Some(other), _) => Err(format!(
            "predicate '{other}' needs a previous snapshot — only eq/ne are valid with action=\"new\""
        )),
    }
}

/// A search is only valid against same-shape snapshots (session restarts
/// with a different memsize invalidate it).
pub(crate) fn geometry_matches(a: &MemImage, b: &MemImage) -> bool {
    a.len() == b.len() && a.base() == b.base()
}

/// Region bound for `new`, clamped to RAM and converted to offsets.
/// `start > end` yields an empty range, not an error (`window_slice`).
pub(crate) fn cheat_region(
    mem: &MemImage,
    start: Option<&str>,
    end: Option<&str>,
) -> Result<(usize, usize), McpError> {
    let base = mem.base().raw();
    let top = base + mem.len() as u32;
    let s = resolve_bound(start, base)?.clamp(base, top);
    let e = resolve_bound(end, top)?.clamp(base, top);
    Ok(((s - base) as usize, (e - base) as usize))
}

fn resolve_bound(addr: Option<&str>, default: u32) -> Result<u32, McpError> {
    Ok(addr
        .map(parse_addr)
        .transpose()?
        .map_or(default, |a| a.raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(action: &str) -> CheatScanArgs {
        CheatScanArgs {
            action: action.to_string(),
            predicate: None,
            value: None,
            width: None,
            start: None,
            end: None,
            limit: None,
            offset: None,
        }
    }

    #[test]
    fn parse_action_all() {
        assert_eq!(parse_action("new"), Ok(Action::New));
        assert_eq!(parse_action("filter"), Ok(Action::Filter));
        assert_eq!(parse_action("list"), Ok(Action::List));
        assert_eq!(parse_action("reset"), Ok(Action::Reset));
        let err = parse_action("NEW").unwrap_err();
        assert!(err.contains("new|filter|list|reset"));
        assert!(parse_action("drop").is_err());
    }

    #[test]
    fn validate_params_new_accepts_region_rejects_offset() {
        assert!(validate_params(&args("new"), Action::New).is_ok());
        let bad = CheatScanArgs {
            offset: Some(1),
            ..args("new")
        };
        assert!(validate_params(&bad, Action::New)
            .unwrap_err()
            .contains("offset is forbidden for action=\"new\""));
    }

    #[test]
    fn validate_params_filter_requires_predicate_rejects_width() {
        let missing = validate_params(&args("filter"), Action::Filter).unwrap_err();
        assert!(missing.contains("predicate is required"));
        let bad = CheatScanArgs {
            predicate: Some("changed".into()),
            width: Some(2),
            ..args("filter")
        };
        assert!(validate_params(&bad, Action::Filter)
            .unwrap_err()
            .contains("width is forbidden for action=\"filter\""));
    }

    #[test]
    fn validate_params_list_accepts_paging_rejects_predicate() {
        let ok = CheatScanArgs {
            limit: Some(8),
            offset: Some(2),
            ..args("list")
        };
        assert!(validate_params(&ok, Action::List).is_ok());
        let bad = CheatScanArgs {
            predicate: Some("eq".into()),
            ..args("list")
        };
        assert!(validate_params(&bad, Action::List)
            .unwrap_err()
            .contains("predicate is forbidden for action=\"list\""));
    }

    #[test]
    fn validate_params_reset_accepts_bare_rejects_limit() {
        assert!(validate_params(&args("reset"), Action::Reset).is_ok());
        let bad = CheatScanArgs {
            limit: Some(1),
            ..args("reset")
        };
        assert!(validate_params(&bad, Action::Reset)
            .unwrap_err()
            .contains("limit is forbidden for action=\"reset\""));
    }

    #[test]
    fn parse_value_grammars() {
        assert_eq!(parse_value("$1F40", Width::Word), Ok(0x1F40));
        assert_eq!(parse_value("0x1f40", Width::Word), Ok(0x1F40));
        assert_eq!(parse_value("8000", Width::Word), Ok(8000));
        assert!(parse_value("$100", Width::Byte)
            .unwrap_err()
            .contains("max $FF"));
        assert_eq!(parse_value("$FFFFFFFF", Width::Long), Ok(0xFFFF_FFFF));
        assert!(parse_value("banana", Width::Byte).is_err());
        assert!(parse_value("", Width::Byte).is_err());
    }

    #[test]
    fn parse_delta_signed() {
        assert_eq!(parse_delta("-1", Width::Word), Ok(-1));
        assert_eq!(parse_delta("-$FF", Width::Byte), Ok(-255));
        assert!(parse_delta("$100", Width::Byte).is_err());
        assert!(parse_delta("256", Width::Byte).is_err());
        assert_eq!(parse_delta("0", Width::Byte), Ok(0));
    }

    #[test]
    fn parse_width_roundtrip_and_rejects() {
        assert_eq!(parse_width(Some(1)), Ok(Width::Byte));
        assert_eq!(parse_width(None), Ok(Width::Word));
        assert_eq!(parse_width(Some(4)), Ok(Width::Long));
        assert!(parse_width(Some(3)).is_err());
    }

    #[test]
    fn predicate_builds_and_requires_value() {
        assert_eq!(
            predicate("changed", None, Width::Word),
            Ok(Predicate::Changed)
        );
        assert!(predicate("changed", Some("1"), Width::Word)
            .unwrap_err()
            .contains("takes no value"));
        assert!(predicate("eq", None, Width::Word)
            .unwrap_err()
            .contains("requires a value"));
        assert_eq!(
            predicate("delta", Some("-3"), Width::Word),
            Ok(Predicate::Delta(-3))
        );
        let err = predicate("gt", None, Width::Word).unwrap_err();
        for name in [
            "changed",
            "unchanged",
            "increased",
            "decreased",
            "eq",
            "ne",
            "delta",
        ] {
            assert!(err.contains(name), "{err} missing {name}");
        }
    }

    #[test]
    fn seed_filter_rejects_non_seedable() {
        assert!(seed_filter(Some("changed"), None, Width::Word)
            .unwrap_err()
            .contains("only eq/ne are valid with action=\"new\""));
        assert_eq!(
            seed_filter(Some("eq"), Some("$3"), Width::Word),
            Ok(Some(SeedFilter::Eq(3)))
        );
        assert_eq!(seed_filter(None, None, Width::Word), Ok(None));
        assert!(seed_filter(None, Some("$3"), Width::Word).is_err());
    }

    #[test]
    fn geometry_matches_checks_len_and_base() {
        use crate::types::Addr;
        let a = MemImage::new(vec![0; 4], Addr::new(0));
        let same = MemImage::new(vec![1; 4], Addr::new(0));
        let diff_len = MemImage::new(vec![0; 8], Addr::new(0));
        let diff_base = MemImage::new(vec![0; 4], Addr::new(4));
        assert!(geometry_matches(&a, &same));
        assert!(!geometry_matches(&a, &diff_len));
        assert!(!geometry_matches(&a, &diff_base));
    }

    #[test]
    fn cheat_region_clamps_and_defaults() {
        use crate::types::Addr;
        let mem = MemImage::new(vec![0; 16], Addr::new(0x400));
        assert_eq!(cheat_region(&mem, None, None).unwrap(), (0, 16));
        assert_eq!(
            cheat_region(&mem, Some("$404"), Some("$408")).unwrap(),
            (4, 8)
        );
        // Out-of-range end clamps to the top of RAM, not an error.
        assert_eq!(cheat_region(&mem, None, Some("$FFFFFF")).unwrap(), (0, 16));
    }

    #[test]
    fn cheat_region_start_after_end_is_empty() {
        use crate::types::Addr;
        let mem = MemImage::new(vec![0; 16], Addr::new(0));
        let (s, e) = cheat_region(&mem, Some("$8"), Some("$4")).unwrap();
        assert!(s > e);
    }
}
