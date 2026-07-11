//! Wire structs, the last-scan store, and pure helpers for the rip tools
//! (§6.3) — internal to the tool layer, pure, unit-tested without a session.

use std::path::PathBuf;

use crate::ripper::{Finding, FindingId, PluginRole, PluginSummary, RipKind};

pub(crate) const RIP_DEFAULT_LIMIT: u32 = 32;
pub(crate) const RIP_MAX_LIMIT: u32 = 128;
pub(crate) const DEPTH_MAX: u8 = 4; // DEPTH_DEFAULT: crate::ripper::registry

/// Last scan's findings, for `FindingId` resolution — one retained (ADR-10).
#[derive(Debug)]
pub(crate) struct ScanStore {
    pub scan_id: u64,
    pub findings: Vec<Finding>,
    pub out_dir: PathBuf,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RipArgs {
    /// Kind filter: picture|music|sprite|packer|boot|raw. Omit for all.
    pub kind: Option<String>,
    /// Filter to one plugin id (see list_ripper_plugins).
    pub plugin: Option<String>,
    /// Depack→rescan recursion depth (default 2, max 4). 0 = no depacking.
    pub depth: Option<u8>,
    /// Findings per page (default 32, max 128).
    pub limit: Option<u32>,
    /// Skip the first N findings (pagination).
    pub offset: Option<u32>,
    /// Page a PREVIOUS scan's findings (ignores kind/plugin/depth). Stale ids rejected.
    pub scan_id: Option<u64>,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct FindingOut {
    /// Pass to `extract`. Format "<scan>:<index>", e.g. "3:0".
    pub id: String,
    pub plugin: String,
    pub kind: String,
    /// Offset within the found-in buffer (RAM if `via` empty, else depacked).
    pub offset: usize,
    pub len: usize,
    pub label: String,
    /// Structure confidence 0-100 (picture findings); null when unscored.
    pub confidence: Option<u8>,
    /// Depack provenance chain, outermost first (e.g. ["pack-ice"]).
    pub via: Vec<String>,
    /// Bus address ("$012345") — only for findings directly in RAM.
    pub addr: Option<String>,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct RipOutput {
    pub scan_id: u64,
    pub total: u32,
    pub offset: u32,
    pub findings: Vec<FindingOut>,
    pub has_more: bool,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct PluginOut {
    pub id: String,
    /// "detector" | "depacker" | "deprotect"
    pub role: String,
    /// Artifact kind (detectors only).
    pub kind: Option<String>,
    pub description: String,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct ListPluginsOutput {
    pub plugins: Vec<PluginOut>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ExtractArgs {
    /// A finding id from the latest rip scan ("<scan>:<index>").
    pub finding_id: String,
    /// Output format: "raw" (default), "prg", or "png".
    pub r#as: Option<String>,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct ExtractOutput {
    pub finding_id: String,
    pub format: String,
    /// Absolute path of the materialized file (under <work_dir>/ripped/).
    pub path: String,
    pub size: u64,
}

/// "picture" ↔ RipKind::Picture etc. — the ONLY kind↔string mapping.
pub(crate) fn kind_from_str(s: &str) -> Result<RipKind, String> {
    match s.to_ascii_lowercase().as_str() {
        "picture" => Ok(RipKind::Picture),
        "music" => Ok(RipKind::Music),
        "sprite" => Ok(RipKind::Sprite),
        "packer" => Ok(RipKind::Packer),
        "boot" => Ok(RipKind::Boot),
        "raw" => Ok(RipKind::Raw),
        _ => Err(format!(
            "unknown kind '{s}': use picture|music|sprite|packer|boot|raw"
        )),
    }
}

pub(crate) fn kind_to_str(k: RipKind) -> &'static str {
    match k {
        RipKind::Picture => "picture",
        RipKind::Music => "music",
        RipKind::Sprite => "sprite",
        RipKind::Packer => "packer",
        RipKind::Boot => "boot",
        RipKind::Raw => "raw",
    }
}

/// Resolve a `FindingId` against the store — `Err` is what the tool returns
/// as `invalid_params`.
pub(crate) fn resolve_finding(store: &Option<ScanStore>, id: FindingId) -> Result<Finding, String> {
    let s = store
        .as_ref()
        .ok_or("no findings in memory: run rip first")?;
    if s.scan_id != id.scan() {
        return Err(format!(
            "stale finding id {id}: latest scan is {}; re-run rip",
            s.scan_id
        ));
    }
    s.findings.get(id.index() as usize).cloned().ok_or_else(|| {
        format!(
            "unknown finding {id}: scan {} has {} findings",
            s.scan_id,
            s.findings.len()
        )
    })
}

/// Build one wire page (id = scan_id:offset+position-in-page).
pub(crate) fn page_findings(
    scan_id: u64,
    findings: &[Finding],
    offset: u32,
    limit: u32,
) -> RipOutput {
    let total = findings.len() as u32;
    let start = (offset as usize).min(findings.len());
    let end = (start + limit as usize).min(findings.len());
    let page: Vec<FindingOut> = findings[start..end]
        .iter()
        .enumerate()
        .map(|(i, f)| finding_out(scan_id, offset + i as u32, f))
        .collect();
    let has_more = offset as usize + page.len() < total as usize;
    RipOutput {
        scan_id,
        total,
        offset,
        findings: page,
        has_more,
    }
}

pub(crate) fn finding_out(scan_id: u64, index: u32, f: &Finding) -> FindingOut {
    FindingOut {
        id: FindingId::new(scan_id, index).to_string(),
        plugin: f.plugin.as_str().to_string(),
        kind: kind_to_str(f.kind).to_string(),
        offset: f.offset.raw(),
        len: f.len,
        label: f.label.clone(),
        confidence: f.confidence,
        via: f.via().iter().map(|p| p.as_str().to_string()).collect(),
        addr: f.addr().map(|a| a.to_string()),
    }
}

pub(crate) fn plugin_out(s: &PluginSummary) -> PluginOut {
    PluginOut {
        id: s.id.as_str().to_string(),
        role: match s.role {
            PluginRole::Detector => "detector",
            PluginRole::Depacker => "depacker",
            PluginRole::Deprotect => "deprotect",
        }
        .to_string(),
        kind: s.kind.map(kind_to_str).map(str::to_string),
        description: s.describe.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ripper::{Registry, RipPlugin};
    use crate::types::{Addr, MemImage, Offset, PluginId};
    use std::sync::Arc;

    /// One-plugin fixture registry over a small buffer, used to build
    /// attached `Finding`s for the wire-helper tests below.
    struct Tagger;
    impl RipPlugin for Tagger {
        fn id(&self) -> PluginId {
            PluginId::new("tagger")
        }
        fn kind(&self) -> RipKind {
            RipKind::Raw
        }
        fn describe(&self) -> &'static str {
            "test-only tagger"
        }
        fn scan(&self, mem: &MemImage) -> Vec<Finding> {
            (0..mem.len())
                .step_by(4)
                .map(|off| Finding::new(self.id(), RipKind::Raw, Offset::new(off), 4, "tag".into()))
                .collect()
        }
    }

    fn attached_findings(n: usize) -> Vec<Finding> {
        let mem = Arc::new(MemImage::new(vec![0u8; n * 4], Addr::new(0)));
        Registry::new()
            .with_detector(Box::new(Tagger))
            .scan(mem, None, 0)
    }

    #[test]
    fn kind_roundtrip_all_six() {
        let expected = ["picture", "music", "sprite", "packer", "boot", "raw"];
        let kinds = [
            RipKind::Picture,
            RipKind::Music,
            RipKind::Sprite,
            RipKind::Packer,
            RipKind::Boot,
            RipKind::Raw,
        ];
        for (k, s) in kinds.into_iter().zip(expected) {
            assert_eq!(kind_to_str(k), s);
            assert_eq!(kind_from_str(kind_to_str(k)), Ok(k));
        }
    }

    #[test]
    fn kind_unknown_is_err() {
        let err = kind_from_str("gif").unwrap_err();
        assert!(err.contains("unknown kind 'gif'"));
    }

    #[test]
    fn resolve_none_store() {
        let err = resolve_finding(&None, FindingId::new(1, 0)).unwrap_err();
        assert_eq!(err, "no findings in memory: run rip first");
    }

    #[test]
    fn resolve_stale_scan() {
        let store = Some(ScanStore {
            scan_id: 2,
            findings: attached_findings(1),
            out_dir: PathBuf::new(),
        });
        let err = resolve_finding(&store, FindingId::new(1, 0)).unwrap_err();
        assert_eq!(err, "stale finding id 1:0: latest scan is 2; re-run rip");
    }

    #[test]
    fn resolve_bad_index() {
        let store = Some(ScanStore {
            scan_id: 2,
            findings: attached_findings(1),
            out_dir: PathBuf::new(),
        });
        let err = resolve_finding(&store, FindingId::new(2, 5)).unwrap_err();
        assert_eq!(err, "unknown finding 2:5: scan 2 has 1 findings");
    }

    #[test]
    fn page_findings_has_more() {
        let findings = attached_findings(5);
        let page = page_findings(1, &findings, 0, 2);
        assert_eq!(page.findings.len(), 2);
        assert_eq!(page.findings[0].id, "1:0");
        assert_eq!(page.findings[1].id, "1:1");
        assert_eq!(page.total, 5);
        assert!(page.has_more);

        let page = page_findings(1, &findings, 4, 2);
        assert_eq!(page.findings.len(), 1);
        assert_eq!(page.findings[0].id, "1:4");
        assert!(!page.has_more);
    }
}
