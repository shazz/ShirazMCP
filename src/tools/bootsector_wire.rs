//! Wire structs + validation + the pure `boot_summary` seam for the
//! `bootsector` tool (§8.2/§8.3) — internal to the tool layer, pure,
//! unit-tested without a session.

use std::path::PathBuf;

use rmcp::ErrorData as McpError;

use super::rip_wire::ScanStore;
use crate::ripper::plugins::boot_heur::{Tell, TellKind};
use crate::ripper::plugins::bootsector::{self, BootClass, Bpb};
use crate::ripper::{Finding, FindingId};
use crate::types::{Addr, MemImage};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BootsectorArgs {
    /// "ram" (read the TOS disk buffer from a running/paused session) or
    /// "disk" (read sector 0 of a raw .ST image at `path`).
    pub source: String,
    /// Path to a raw .ST image — required for source="disk", forbidden for "ram".
    pub path: Option<String>,
    /// Depack→rescan depth for the ram scan (default 2, max 4). Ignored for disk.
    pub depth: Option<u8>,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct BpbOut {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub root_dir_entries: u16,
    pub total_sectors: u16,
    pub media_descriptor: u8,
    pub sectors_per_fat: u16,
    pub sectors_per_track: u16,
    pub sides: u16,
    pub hidden_sectors: u16,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct TellOut {
    /// "vector-write" | "self-copy" | "disk-trap"
    pub kind: String,
    /// Where the flagged instruction sits ("$00001E").
    pub addr: String,
    /// Decoded instruction text (the annotated evidence).
    pub insn: String,
    pub note: String,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct BootsectorOutput {
    pub scan_id: u64,
    /// false only for source="ram" when no boot sector was found — NOT an error.
    pub found: bool,
    /// Finding id of the sector — pass to extract (as="raw" / "prg").
    pub finding_id: Option<String>,
    /// $1234 checksum verdict: does TOS consider this sector executable?
    pub executable: bool,
    /// The big-endian word-sum, "$XXXX". ($1234 iff executable.)
    pub checksum: String,
    /// "plain-data" | "loader" | "suspicious" (heuristic; see confidence).
    pub classification: String,
    pub bpb_plausible: bool,
    pub bpb: Option<BpbOut>,
    /// Triggered virus tells — evidence, not a verdict. Empty for a clean loader.
    pub tells: Vec<TellOut>,
    /// 0.0..=0.9 — deliberately never 1.0. 0.0 = no tells / undecodable.
    pub confidence: f32,
    pub label: String,
}

impl BootsectorOutput {
    fn not_found(scan_id: u64) -> Self {
        Self {
            scan_id,
            found: false,
            finding_id: None,
            executable: false,
            checksum: String::new(),
            classification: String::new(),
            bpb_plausible: false,
            bpb: None,
            tells: Vec::new(),
            confidence: 0.0,
            label: String::new(),
        }
    }
}

pub(crate) enum Source {
    Ram,
    Disk(PathBuf),
}

/// §8.3 validation: `source`/`path` combination gate. The real filesystem
/// validation of `path` happens in `disk::read_boot_sector` — not duplicated
/// here.
pub(crate) fn validate_source(a: &BootsectorArgs) -> Result<Source, McpError> {
    match a.source.to_ascii_lowercase().as_str() {
        "ram" if a.path.is_some() => Err(McpError::invalid_params(
            "path is only valid for source=\"disk\"",
            None,
        )),
        "ram" => Ok(Source::Ram),
        "disk" => a
            .path
            .clone()
            .map(|p| Source::Disk(PathBuf::from(p)))
            .ok_or_else(|| McpError::invalid_params("source=\"disk\" requires path", None)),
        _ => Err(McpError::invalid_params(
            "source must be \"ram\" or \"disk\"",
            None,
        )),
    }
}

/// Fold the stored boot finding into the `bootsector` output shape.
/// Err = store missing/raced (tool maps to internal_error).
pub(crate) fn boot_summary(
    store: &Option<ScanStore>,
    scan_id: u64,
) -> Result<BootsectorOutput, String> {
    let s = store.as_ref().ok_or("scan store empty after rip; retry")?;
    if s.scan_id != scan_id {
        return Err(format!(
            "scan store moved to {} during bootsector; retry",
            s.scan_id
        ));
    }
    let Some((i, f)) = s
        .findings
        .iter()
        .enumerate()
        .find(|(_, f)| f.plugin.as_str() == "bootsector")
    else {
        return Ok(BootsectorOutput::not_found(scan_id));
    };
    found_out(scan_id, i as u32, f)
}

/// Recompute the wire shape from one attached `bootsector` finding's bytes.
fn found_out(scan_id: u64, index: u32, f: &Finding) -> Result<BootsectorOutput, String> {
    let bytes = f.bytes().ok_or("boot finding unattached")?;
    let mem = MemImage::new(bytes.to_vec(), Addr::new(0));
    let r = bootsector::analyze_sector(&mem, 0);
    Ok(BootsectorOutput {
        scan_id,
        found: true,
        finding_id: Some(FindingId::new(scan_id, index).to_string()),
        executable: r.executable,
        checksum: format!("${:04X}", r.checksum),
        classification: class_str(r.class).to_string(),
        bpb_plausible: r.bpb_plausible,
        bpb: Some(bpb_out(&r.bpb)),
        tells: r.tells.iter().map(tell_out).collect(),
        confidence: r.confidence,
        label: f.label.clone(),
    })
}

fn class_str(c: BootClass) -> &'static str {
    match c {
        BootClass::PlainData => "plain-data",
        BootClass::Loader => "loader",
        BootClass::Suspicious => "suspicious",
    }
}

fn bpb_out(b: &Bpb) -> BpbOut {
    BpbOut {
        bytes_per_sector: b.bps,
        sectors_per_cluster: b.spc,
        reserved_sectors: b.res,
        num_fats: b.nfats,
        root_dir_entries: b.ndirs,
        total_sectors: b.nsects,
        media_descriptor: b.media,
        sectors_per_fat: b.spf,
        sectors_per_track: b.spt,
        sides: b.nsides,
        hidden_sectors: b.nhid,
    }
}

fn tell_out(t: &Tell) -> TellOut {
    TellOut {
        kind: match t.kind {
            TellKind::VectorWrite => "vector-write",
            TellKind::SelfCopy => "self-copy",
            TellKind::DiskTrap => "disk-trap",
        }
        .to_string(),
        addr: t.addr.to_string(),
        insn: t.insn.clone(),
        note: t.note.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ripper::plugins::bootsector::BootSector;
    use crate::ripper::{Registry, RipKind};
    use std::sync::Arc;

    fn put(s: &mut [u8], off: usize, bytes: &[u8]) {
        s[off..off + bytes.len()].copy_from_slice(bytes);
    }

    fn set_checksum(s: &mut [u8]) {
        let mut sum = 0u16;
        for i in 0..255 {
            sum = sum.wrapping_add(u16::from_be_bytes([s[2 * i], s[2 * i + 1]]));
        }
        let adj = 0x1234u16.wrapping_sub(sum);
        put(s, 0x1FE, &adj.to_be_bytes());
    }

    fn plain_fat_sector() -> Vec<u8> {
        let mut s = vec![0u8; 512];
        put(&mut s, 0x00B, &512u16.to_le_bytes());
        s[0x00D] = 2;
        put(&mut s, 0x00E, &1u16.to_le_bytes());
        s[0x010] = 2;
        put(&mut s, 0x011, &112u16.to_le_bytes());
        put(&mut s, 0x013, &720u16.to_le_bytes());
        s[0x015] = 0xF9;
        put(&mut s, 0x016, &5u16.to_le_bytes());
        put(&mut s, 0x018, &9u16.to_le_bytes());
        put(&mut s, 0x01A, &2u16.to_le_bytes());
        s
    }

    fn bootable_loader_sector() -> Vec<u8> {
        let mut s = plain_fat_sector();
        put(&mut s, 0x000, &[0x60, 0x1C]);
        put(&mut s, 0x01E, &[0x4E, 0x71]);
        put(&mut s, 0x020, &[0x4E, 0x75]);
        set_checksum(&mut s);
        s
    }

    fn virus_like_sector() -> Vec<u8> {
        let mut s = plain_fat_sector();
        put(&mut s, 0x000, &[0x60, 0x1C]);
        put(&mut s, 0x01E, &[0x21, 0xC8, 0x04, 0x72]);
        put(&mut s, 0x022, &[0x21, 0xC9, 0x04, 0x2A]);
        put(&mut s, 0x026, &[0x22, 0xD8]);
        put(&mut s, 0x028, &[0x51, 0xC8, 0xFF, 0xFC]);
        put(&mut s, 0x02C, &[0x4E, 0x75]);
        set_checksum(&mut s);
        s
    }

    fn store_from(sector: Vec<u8>) -> Option<ScanStore> {
        let mem = Arc::new(MemImage::new(sector, Addr::new(0)));
        let findings =
            Registry::new()
                .with_detector(Box::new(BootSector))
                .scan(mem, Some(RipKind::Boot), 0);
        Some(ScanStore {
            scan_id: 7,
            findings,
            out_dir: PathBuf::new(),
        })
    }

    #[test]
    fn summary_loader() {
        let store = store_from(bootable_loader_sector());
        let out = boot_summary(&store, 7).unwrap();
        assert!(out.found);
        assert_eq!(out.finding_id, Some("7:0".to_string()));
        assert!(out.executable);
        assert_eq!(out.checksum, "$1234");
        assert_eq!(out.classification, "loader");
        assert!(out.bpb.is_some());
        assert!(out.tells.is_empty());
        assert_eq!(out.confidence, 0.0);
    }

    #[test]
    fn summary_suspicious() {
        let store = store_from(virus_like_sector());
        let out = boot_summary(&store, 7).unwrap();
        assert_eq!(out.classification, "suspicious");
        assert!(out.tells.iter().any(|t| t.kind == "vector-write"));
        assert!(out.confidence > 0.0);
    }

    #[test]
    fn summary_plain_data() {
        let store = store_from(plain_fat_sector());
        let out = boot_summary(&store, 7).unwrap();
        assert!(out.found);
        assert!(!out.executable);
        assert_eq!(out.classification, "plain-data");
        assert_eq!(out.bpb.unwrap().bytes_per_sector, 512);
    }

    #[test]
    fn summary_not_found() {
        let store = Some(ScanStore {
            scan_id: 7,
            findings: vec![],
            out_dir: PathBuf::new(),
        });
        let out = boot_summary(&store, 7).unwrap();
        assert!(!out.found);
        assert!(out.finding_id.is_none());
        assert!(out.bpb.is_none());
    }

    #[test]
    fn summary_store_raced() {
        let store = Some(ScanStore {
            scan_id: 8,
            findings: vec![],
            out_dir: PathBuf::new(),
        });
        let err = boot_summary(&store, 7).unwrap_err();
        assert!(err.contains("retry"));
    }

    #[test]
    fn summary_no_store() {
        let err = boot_summary(&None, 7).unwrap_err();
        assert!(err.contains("retry"));
    }

    #[test]
    fn validate_source_rules() {
        let ram = BootsectorArgs {
            source: "ram".to_string(),
            path: None,
            depth: None,
        };
        assert!(matches!(validate_source(&ram), Ok(Source::Ram)));

        let ram_with_path = BootsectorArgs {
            source: "ram".to_string(),
            path: Some("x.st".to_string()),
            depth: None,
        };
        assert!(validate_source(&ram_with_path).is_err());

        let disk_no_path = BootsectorArgs {
            source: "disk".to_string(),
            path: None,
            depth: None,
        };
        assert!(validate_source(&disk_no_path).is_err());

        let disk = BootsectorArgs {
            source: "disk".to_string(),
            path: Some("x.st".to_string()),
            depth: None,
        };
        assert!(matches!(validate_source(&disk), Ok(Source::Disk(_))));

        let bad = BootsectorArgs {
            source: "tape".to_string(),
            path: None,
            depth: None,
        };
        assert!(validate_source(&bad).is_err());
    }
}
