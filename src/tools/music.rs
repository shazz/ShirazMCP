//! `identify_music` — façade over rip(kind=music) (PLAN §P6): one call that
//! scans, then summarizes the stored findings into the skill's output shape
//! { sndh_header?, replay_addr, player, confidence }. Findings stay in the
//! last-scan store, so `extract(finding_id, as="prg")` works immediately
//! after. Structured facts are recomputed from finding bytes (SNDH) and the
//! pinned ym-player label round-trip — never scraped from prose.

use std::sync::PoisonError;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::rip_wire::{RipArgs, ScanStore, RIP_MAX_LIMIT};
use super::HatariServer;
use crate::ripper::plugins::{sndh_header, ym_player};
use crate::ripper::{Finding, FindingId};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IdentifyMusicArgs {
    /// Depack→rescan recursion depth for the underlying scan (default 2, max 4).
    pub depth: Option<u8>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SndhHeaderOut {
    /// Finding id of the SNDH blob — pass to `extract` (as="prg" for a runnable rip).
    pub finding_id: String,
    pub title: Option<String>,
    pub composer: Option<String>,
    pub ripper: Option<String>,
    pub converter: Option<String>,
    pub subtunes: u16,
    /// "timer-a".."timer-d" | "vbl"
    pub replay: String,
    pub freq_hz: u16,
    /// init/exit/play entry offsets relative to the blob start.
    pub init_offset: u32,
    pub exit_offset: Option<u32>,
    pub play_offset: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IdentifyMusicOutput {
    /// The underlying rip scan — its finding ids stay pageable via `rip(scan_id=…)`.
    pub scan_id: u64,
    /// Present iff an SNDH blob was found (first one, scan order).
    pub sndh_header: Option<SndhHeaderOut>,
    /// Bus address ("$012345") of the best replay cluster; absent when no
    /// anchor was found or the cluster lives in a depacked buffer.
    pub replay_addr: Option<String>,
    /// Finding id of that cluster — pass to `extract`.
    pub replay_finding_id: Option<String>,
    /// Driver name, or "unknown" (graceful degradation — never an error).
    pub player: String,
    /// 0.0 = nothing found; 0.25 = anchor only; higher = signature match.
    pub confidence: f32,
    pub total_music_findings: u32,
}

#[tool_router(router = music_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "identify_music",
        description = "Identify music in emulator RAM: \
scans for SNDH blobs (title/composer/subtunes/timer) and YM2149 replay \
routines (via the $FFFF8800/$FFFF8802 write anchor), fingerprints the \
player (Mad Max, TCB Tracker, …). Unknown players return player=\"unknown\" \
with low confidence — never an error. A façade over rip(kind=music): the \
findings are stored, so pass replay_finding_id or the SNDH finding_id \
straight to extract (as=\"prg\" saves a runnable rip)."
    )]
    pub async fn identify_music(
        &self,
        Parameters(a): Parameters<IdentifyMusicArgs>,
    ) -> Result<Json<IdentifyMusicOutput>, McpError> {
        let Json(page) = self
            .rip(Parameters(RipArgs {
                kind: Some("music".to_string()),
                plugin: None,
                depth: a.depth,
                limit: Some(RIP_MAX_LIMIT),
                offset: Some(0),
                scan_id: None,
            }))
            .await?;
        let guard = self
            .last_scan
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        music_summary(&guard, page.scan_id)
            .map(Json)
            .map_err(|m| McpError::internal_error(m, None))
    }
}

/// Fold the stored music findings into the identify_music shape.
/// Err = store missing/raced (tool maps to internal_error).
pub(crate) fn music_summary(
    store: &Option<ScanStore>,
    scan_id: u64,
) -> Result<IdentifyMusicOutput, String> {
    let s = store.as_ref().ok_or("scan store empty after rip; retry")?;
    if s.scan_id != scan_id {
        return Err(format!(
            "scan store moved to {} during identify_music; retry",
            s.scan_id
        ));
    }
    let sndh_out_val = s
        .findings
        .iter()
        .enumerate()
        .find(|(_, f)| f.plugin.as_str() == "sndh")
        .and_then(|(i, f)| sndh_out(scan_id, i as u32, f));
    let best = best_ym(&s.findings);
    Ok(IdentifyMusicOutput {
        scan_id,
        sndh_header: sndh_out_val,
        replay_addr: best
            .as_ref()
            .and_then(|(_, f, ..)| f.addr())
            .map(|a| a.to_string()),
        replay_finding_id: best
            .as_ref()
            .map(|(i, ..)| FindingId::new(scan_id, *i).to_string()),
        player: best
            .as_ref()
            .map_or_else(|| "unknown".to_string(), |(.., p, _)| p.clone()),
        confidence: best.as_ref().map_or(0.0, |(.., c)| *c),
        total_music_findings: s.findings.len() as u32,
    })
}

/// Re-parse an SNDH finding's attached bytes (header base = finding offset
/// 0) into the wire shape. `None` = blob changed under us / unattached —
/// degrade, don't error.
fn sndh_out(scan_id: u64, index: u32, f: &Finding) -> Option<SndhHeaderOut> {
    let info = sndh_header::parse_header(f.bytes()?, 0)?;
    Some(SndhHeaderOut {
        finding_id: FindingId::new(scan_id, index).to_string(),
        title: info.title,
        composer: info.composer,
        ripper: info.ripper,
        converter: info.converter,
        subtunes: info.subtunes,
        replay: info.replay.as_str().to_string(),
        freq_hz: info.freq_hz,
        init_offset: info.init,
        exit_offset: info.exit,
        play_offset: info.play,
    })
}

/// Highest-confidence `ym-player` finding, tie -> first (lowest index).
/// Unparseable labels are skipped (defensive; cannot happen with the
/// `format_label`/`parse_label` round-trip).
fn best_ym(findings: &[Finding]) -> Option<(u32, &Finding, String, f32)> {
    findings
        .iter()
        .enumerate()
        .filter(|(_, f)| f.plugin.as_str() == "ym-player")
        .filter_map(|(i, f)| {
            let (player, conf, _hits) = ym_player::parse_label(&f.label)?;
            Some((i as u32, f, player, conf))
        })
        .max_by(|a, b| a.3.total_cmp(&b.3))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::ripper::plugins::{sndh::Sndh, ym_player::YmPlayer};
    use crate::ripper::{Registry, RipKind};
    use crate::types::{Addr, MemImage};

    fn build_sndh() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x60, 0x00, 0x00, 0x42]);
        out.extend_from_slice(&[0x60, 0x00, 0x00, 0x40]);
        out.extend_from_slice(&[0x60, 0x00, 0x00, 0x3E]);
        out.extend_from_slice(b"SNDH");
        out.extend_from_slice(b"TITLTest Tune\0");
        out.extend_from_slice(b"COMMClaude\0");
        out.extend_from_slice(b"RIPPShirazMCP\0");
        out.extend_from_slice(b"##02");
        out.extend_from_slice(b"TC50\0");
        out.extend_from_slice(b"HDNS");
        assert_eq!(out.len(), 68);
        out.extend_from_slice(&[0x4E, 0x75, 0x4E, 0x75]);
        out.extend_from_slice(&[
            0x41, 0xF8, 0x88, 0x00, // lea.l   $8800.w, a0
            0x11, 0xFC, 0x00, 0x0B, 0x88, 0x00, // move.b  #$b, $8800.w  (select R11)
            0x48, 0xF8, 0x00, 0x0F, 0x88, 0x00, // movem.l d0-d3, $8800.w (hippel-movem-w)
            0x4E, 0x75, // rts
        ]);
        out
    }

    fn raw_writes_block() -> Vec<u8> {
        vec![0x11, 0xC0, 0x88, 0x00, 0x11, 0xC0, 0x88, 0x02, 0x4E, 0x75]
    }

    fn store_from(bytes: Vec<u8>, base: u32) -> Option<ScanStore> {
        let mem = Arc::new(MemImage::new(bytes, Addr::new(base)));
        let findings = Registry::new()
            .with_detector(Box::new(Sndh))
            .with_detector(Box::new(YmPlayer))
            .scan(mem, Some(RipKind::Music), 0);
        Some(ScanStore {
            scan_id: 7,
            findings,
            out_dir: PathBuf::new(),
        })
    }

    #[test]
    fn summary_full() {
        let mut buf = vec![0u8; 4096];
        let sndh = build_sndh();
        buf[0..sndh.len()].copy_from_slice(&sndh);
        let store = store_from(buf, 0x30000);
        let out = music_summary(&store, 7).unwrap();
        let header = out.sndh_header.unwrap();
        assert_eq!(header.title.as_deref(), Some("Test Tune"));
        assert_eq!(header.subtunes, 2);
        assert_eq!(header.replay, "timer-c");
        assert_eq!(header.freq_hz, 50);
        assert_eq!(header.finding_id, "7:0");
        assert_eq!(out.replay_finding_id, Some("7:1".to_string()));
        assert_eq!(out.replay_addr, Some(Addr::new(0x30000 + 72).to_string()));
        assert_eq!(out.player, "Jochen Hippel (Mad Max)");
        assert!((out.confidence - 0.82).abs() < 1e-6);
        assert_eq!(out.total_music_findings, 2);
    }

    #[test]
    fn summary_unknown_degrades() {
        let mut buf = vec![0u8; 64];
        buf[0..10].copy_from_slice(&raw_writes_block());
        let store = store_from(buf, 0);
        let out = music_summary(&store, 7).unwrap();
        assert!(out.sndh_header.is_none());
        assert_eq!(out.player, "unknown");
        assert!((out.confidence - 0.25).abs() < 1e-6);
        assert!(out.replay_addr.is_some());
    }

    #[test]
    fn summary_empty_scan() {
        let store = Some(ScanStore {
            scan_id: 7,
            findings: vec![],
            out_dir: PathBuf::new(),
        });
        let out = music_summary(&store, 7).unwrap();
        assert_eq!(out.player, "unknown");
        assert_eq!(out.confidence, 0.0);
        assert!(out.sndh_header.is_none());
        assert!(out.replay_addr.is_none());
        assert!(out.replay_finding_id.is_none());
        assert_eq!(out.total_music_findings, 0);
    }

    #[test]
    fn summary_store_raced() {
        let store = Some(ScanStore {
            scan_id: 8,
            findings: vec![],
            out_dir: PathBuf::new(),
        });
        let err = music_summary(&store, 7).unwrap_err();
        assert!(err.contains("retry"));
    }

    #[test]
    fn summary_no_store() {
        let err = music_summary(&None, 7).unwrap_err();
        assert!(err.contains("retry"));
    }
}
