//! Wire args + mode/path validation + the output enum for `save_prg`
//! (§7.2 repatch / §8.1 snapshot, P22) — internal to the tool layer, pure,
//! unit-tested without a session. Repatch's own output shape and region
//! plumbing live in `repatch_wire.rs` (200-line budget split).

use rmcp::ErrorData as McpError;
use std::path::{Component, Path, PathBuf};

pub use super::repatch_wire::RepatchOutput;

fn inv(msg: impl Into<String>) -> McpError {
    McpError::invalid_params(msg.into(), None)
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SavePrgArgs {
    /// Where to write the patched copy / snapshot .PRG (parent directory must exist).
    pub out_path: String,
    /// Path of the ORIGINAL .PRG the running program was loaded from — the byte baseline the diff maps back onto.
    /// Required for mode="repatch"; forbidden for mode="snapshot" (a snapshot needs no original file).
    pub source_prg: Option<String>,
    /// "repatch" (default): re-emit the running program as a modified copy of its original .PRG.
    /// "snapshot": freeze the whole machine (RAM + CPU + hardware registers) into a self-restoring .PRG.
    pub mode: Option<String>,
    /// Optional allow-list: only diffs inside these RAM ranges are applied (others reported as filtered). mode="repatch" only.
    pub regions: Option<Vec<RegionArg>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RegionArg {
    /// Bus address, "$hex" or decimal (same syntax as read_memory).
    pub addr: String,
    /// Length in bytes (>= 1).
    pub len: usize,
}

/// The two flavours legitimately report different facts (ADR-30/ADR-38);
/// forcing them into one struct of `Option`s would be worse for every
/// consumer, so the wire type is an untagged enum discriminated by shape
/// (`changed_bytes` for repatch vs `entry_pc` for snapshot).
///
/// `JsonSchema` is hand-implemented rather than derived: schemars renders
/// an untagged enum as a bare `oneOf` with no top-level `"type"` (the
/// branches' shapes aren't unified into one type key), but rmcp's
/// `#[tool]` macro rejects any output schema whose root lacks `"type":
/// "object"` (MCP spec requirement, checked at router-build time —
/// verified live: the panic fires before the server ever answers a
/// request). Both variants really are objects, so generating from the
/// schema-only `SavePrgOutputSchema` mirror and adding `"type":"object"`
/// alongside the derived `oneOf` is exact, not a lie — JSON Schema allows
/// `type` and `oneOf` together (`oneOf` still selects which shape).
#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum SavePrgOutput {
    Repatch(RepatchOutput),
    Snapshot(crate::tools::snapshot_wire::SnapshotOutput),
}

#[derive(serde::Serialize, schemars::JsonSchema)]
#[serde(untagged)]
#[allow(dead_code, reason = "schema-shape mirror only, never constructed")]
enum SavePrgOutputSchema {
    Repatch(RepatchOutput),
    Snapshot(crate::tools::snapshot_wire::SnapshotOutput),
}

impl schemars::JsonSchema for SavePrgOutput {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SavePrgOutput".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut schema = SavePrgOutputSchema::json_schema(generator);
        if let Some(obj) = schema.as_object_mut() {
            obj.insert("type".to_string(), serde_json::json!("object"));
        }
        schema
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Repatch,
    Snapshot,
}

/// None/"repatch" -> Repatch; "snapshot" -> Snapshot (ADR-30's reserved
/// value, shipped by ADR-38); anything else -> generic error.
pub(crate) fn validate_mode(m: Option<&str>) -> Result<Mode, McpError> {
    match m {
        None | Some("repatch") => Ok(Mode::Repatch),
        Some("snapshot") => Ok(Mode::Snapshot),
        Some(_) => Err(inv("mode must be \"repatch\" or \"snapshot\"")),
    }
}

/// Cross-field validation (§8.1): each mode has arguments the other
/// forbids. Checked once, up front, so the tool body never has to guess
/// which fields are meaningful.
pub(crate) fn validate_args(a: &SavePrgArgs, mode: &Mode) -> Result<(), McpError> {
    match mode {
        Mode::Repatch if a.source_prg.is_none() => {
            Err(inv("source_prg is required for mode=repatch"))
        }
        Mode::Snapshot if a.source_prg.is_some() => {
            Err(inv("source_prg is not used by mode=snapshot — omit it"))
        }
        Mode::Snapshot if a.regions.is_some() => Err(inv("regions apply to mode=repatch only")),
        _ => Ok(()),
    }
}

/// SECURITY: path traversal / clobber guard for mode=repatch (`source`'s
/// deep check is `prg_file::read_prg_file`'s job). Callers pass an
/// already-`validate_args`-checked `SavePrgArgs` (`source_prg` is `Some`).
pub(crate) fn validate_paths(a: &SavePrgArgs) -> Result<(PathBuf, PathBuf), McpError> {
    let source_prg = a
        .source_prg
        .as_deref()
        .ok_or_else(|| inv("source_prg is required for mode=repatch"))?;
    if source_prg.is_empty() {
        return Err(inv("source_prg must not be empty"));
    }
    let source = PathBuf::from(source_prg);
    let source_canon = source
        .canonicalize()
        .map_err(|_| inv(format!("source_prg does not exist: {source_prg}")))?;
    let out = validate_out_path(&a.out_path, Some(&source_canon))?;
    Ok((source, out))
}

/// mode=snapshot has no source file to guard against — just the `out_path`
/// shape/existence checks.
pub(crate) fn validate_out_only(out_path: &str) -> Result<PathBuf, McpError> {
    validate_out_path(out_path, None)
}

fn validate_out_path(out_path: &str, source_canon: Option<&Path>) -> Result<PathBuf, McpError> {
    if out_path.is_empty() {
        return Err(inv("out_path must not be empty"));
    }
    let out = PathBuf::from(out_path);
    if out.components().any(|c| c == Component::ParentDir) {
        return Err(inv("out_path must not contain '..'"));
    }
    let file_name = out
        .file_name()
        .ok_or_else(|| inv("out_path must name a file"))?;
    let parent_canon = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .map_err(|_| inv(format!("out_path parent directory missing: {out_path}")))?;
    let effective = parent_canon.join(file_name);
    if effective.exists() && !effective.is_file() {
        return Err(inv("out_path exists and is not a regular file"));
    }
    if source_canon.is_some_and(|s| effective == *s) {
        return Err(inv(
            "refusing to overwrite the source PRG — pick a different out_path",
        ));
    }
    Ok(effective)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repatch_args(out_path: &str, source_prg: &str) -> SavePrgArgs {
        SavePrgArgs {
            out_path: out_path.to_string(),
            source_prg: Some(source_prg.to_string()),
            mode: None,
            regions: None,
        }
    }

    #[test]
    fn mode_default_and_repatch() {
        assert!(matches!(validate_mode(None), Ok(Mode::Repatch)));
        assert!(matches!(validate_mode(Some("repatch")), Ok(Mode::Repatch)));
    }

    #[test]
    fn mode_snapshot_accepted() {
        assert!(matches!(
            validate_mode(Some("snapshot")),
            Ok(Mode::Snapshot)
        ));
    }

    #[test]
    fn mode_garbage_rejected() {
        assert!(validate_mode(Some("prg")).is_err());
    }

    #[test]
    fn validate_args_repatch_requires_source() {
        let a = SavePrgArgs {
            out_path: "/tmp/out.prg".to_string(),
            source_prg: None,
            mode: None,
            regions: None,
        };
        let err = validate_args(&a, &Mode::Repatch).unwrap_err();
        assert!(err.message.contains("required for mode=repatch"));
    }

    #[test]
    fn validate_args_snapshot_rejects_source() {
        let a = repatch_args("/tmp/out.prg", "/tmp/src.prg");
        let err = validate_args(&a, &Mode::Snapshot).unwrap_err();
        assert!(err.message.contains("not used by mode=snapshot"));
    }

    #[test]
    fn validate_args_snapshot_rejects_regions() {
        let a = SavePrgArgs {
            out_path: "/tmp/out.prg".to_string(),
            source_prg: None,
            mode: Some("snapshot".to_string()),
            regions: Some(vec![RegionArg {
                addr: "$1000".to_string(),
                len: 4,
            }]),
        };
        let err = validate_args(&a, &Mode::Snapshot).unwrap_err();
        assert!(err.message.contains("regions apply to mode=repatch only"));
    }

    #[test]
    fn validate_args_accepts_matching_pairs() {
        let repatch = repatch_args("/tmp/out.prg", "/tmp/src.prg");
        assert!(validate_args(&repatch, &Mode::Repatch).is_ok());
        let snapshot = SavePrgArgs {
            out_path: "/tmp/out.prg".to_string(),
            source_prg: None,
            mode: Some("snapshot".to_string()),
            regions: None,
        };
        assert!(validate_args(&snapshot, &Mode::Snapshot).is_ok());
    }

    #[test]
    fn out_path_parent_missing() {
        let src = std::env::temp_dir().join("shirazmcp-savewire-src-missing.prg");
        std::fs::write(&src, b"x").unwrap();
        let a = repatch_args("/nonexistent-dir-xyz/out.prg", &src.display().to_string());
        assert!(validate_paths(&a).is_err());
        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn out_path_dotdot_rejected() {
        let src = std::env::temp_dir().join("shirazmcp-savewire-src-dotdot.prg");
        std::fs::write(&src, b"x").unwrap();
        let a = repatch_args("/tmp/../etc/x.prg", &src.display().to_string());
        let err = validate_paths(&a).unwrap_err();
        assert!(err.message.contains(".."));
        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn out_equals_source_rejected() {
        let src = std::env::temp_dir().join("shirazmcp-savewire-same.prg");
        std::fs::write(&src, b"x").unwrap();
        let a = repatch_args(&src.display().to_string(), &src.display().to_string());
        let err = validate_paths(&a).unwrap_err();
        assert!(err.message.contains("refusing to overwrite"));
        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn out_existing_directory_rejected() {
        let src = std::env::temp_dir().join("shirazmcp-savewire-src-dir.prg");
        std::fs::write(&src, b"x").unwrap();
        let dir = std::env::temp_dir().join("shirazmcp-savewire-outdir");
        std::fs::create_dir_all(&dir).unwrap();
        let a = repatch_args(&dir.display().to_string(), &src.display().to_string());
        assert!(validate_paths(&a).is_err());
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_out_only_accepts_snapshot_shape() {
        let dir = std::env::temp_dir();
        let out = dir.join("shirazmcp-savewire-snap-out.prg");
        let path = validate_out_only(&out.display().to_string()).unwrap();
        assert_eq!(path, out);
    }
}
