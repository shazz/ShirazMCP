//! `extract` — materialize a finding from the last rip scan: raw bytes,
//! GEMDOS PRG wrap, or (P4) a rendered PNG returned inline as an MCP image
//! block plus saved to disk. Moved out of rip.rs in P4 (line budget +
//! disjoint deps); the scan/store logic stays in rip.rs / rip_wire.rs.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use base64::Engine;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::rip_wire::{resolve_finding, ExtractArgs, ExtractOutput};
use super::{to_mcp, HatariServer};
use crate::error::Error;
use crate::ripper::{ExtractFormat, Extracted, Finding, FindingId};

/// Tool-layer extract dispatch: byte-container formats vs the png render
/// pipeline (ADR-12 — png is a render, not an `ExtractFormat`). Lives here
/// (not rip_wire.rs) so rip_wire.rs stays under the 200-line budget — see
/// P4.md §7.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireFormat {
    Bytes(ExtractFormat),
    Png,
}

fn format_from_str(s: &str) -> Result<WireFormat, String> {
    match s {
        "raw" => Ok(WireFormat::Bytes(ExtractFormat::Raw)),
        "prg" => Ok(WireFormat::Bytes(ExtractFormat::Prg)),
        "png" => Ok(WireFormat::Png),
        other => Err(format!(
            "unknown extract format '{other}': use raw, prg or png"
        )),
    }
}

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = extract_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "extract",
        description = "Materialize a finding from the last \
rip scan to a file under <work_dir>/ripped/ and return its path + size \
(never raw bytes). as: \"raw\" (exact bytes, .bin), \"prg\" (minimal GEMDOS \
PRG wrap, runnable on the ST), or \"png\" (decode a picture finding — the \
PNG is saved AND returned inline as an image)."
    )]
    pub async fn extract(
        &self,
        Parameters(a): Parameters<ExtractArgs>,
    ) -> Result<CallToolResult, McpError> {
        let id = FindingId::parse(&a.finding_id)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let wire = format_from_str(a.r#as.as_deref().unwrap_or("raw"))
            .map_err(|m| McpError::invalid_params(m, None))?;
        let (finding, out_dir) = self.resolve_stored_finding(id)?;
        let seq = self.extract_seq.fetch_add(1, Ordering::Relaxed) + 1;
        match wire {
            WireFormat::Bytes(f) => self.extract_bytes(id, finding, f, out_dir, seq).await,
            WireFormat::Png => self.extract_as_png(id, finding, out_dir, seq).await,
        }
    }
}

impl HatariServer {
    /// Moved VERBATIM from rip.rs (P3 §6.2) — do not reformat.
    fn resolve_stored_finding(&self, id: FindingId) -> Result<(Finding, PathBuf), McpError> {
        let guard = self
            .last_scan
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let finding = resolve_finding(&guard, id).map_err(|m| McpError::invalid_params(m, None))?;
        let out_dir = guard
            .as_ref()
            .map_or_else(PathBuf::new, |s| s.out_dir.clone());
        Ok((finding, out_dir))
    }

    async fn extract_bytes(
        &self,
        id: FindingId,
        finding: Finding,
        format: ExtractFormat,
        out_dir: PathBuf,
        seq: u64,
    ) -> Result<CallToolResult, McpError> {
        let ex = tokio::task::spawn_blocking(move || {
            crate::ripper::extract(&finding, format, &out_dir, seq)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("extract task failed: {e}"), None))?
        .map_err(to_mcp)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(out_json(
            id,
            format.as_str(),
            &ex,
        )?)]))
    }

    async fn extract_as_png(
        &self,
        id: FindingId,
        finding: Finding,
        out_dir: PathBuf,
        seq: u64,
    ) -> Result<CallToolResult, McpError> {
        let reg = Arc::clone(&self.registry);
        let plugin = finding.plugin; // PluginId is Copy — keep for messages
        let rendered =
            tokio::task::spawn_blocking(move || render_and_write(&reg, &finding, &out_dir, seq))
                .await
                .map_err(|e| McpError::internal_error(format!("extract task failed: {e}"), None))?
                .map_err(to_mcp)?;
        let Some((ex, png)) = rendered else {
            return Err(McpError::invalid_params(
                format!(
                    "finding {id} (plugin '{plugin}') is not renderable as \
                     png — only picture findings support as=png"
                ),
                None,
            ));
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        Ok(CallToolResult::success(vec![
            ContentBlock::image(b64, "image/png"),
            ContentBlock::text(out_json(id, "png", &ex)?),
        ]))
    }
}

/// Render via the plugin that produced the finding, then materialize.
/// Ok(None) = plugin can't render these bytes (caller → invalid_params).
fn render_and_write(
    reg: &crate::ripper::Registry,
    finding: &Finding,
    out_dir: &std::path::Path,
    seq: u64,
) -> Result<Option<(Extracted, Vec<u8>)>, Error> {
    let bytes = finding.bytes().ok_or_else(|| Error::Ripper {
        plugin: finding.plugin,
        message: "finding has no attached source buffer".to_string(),
    })?;
    let Some(png) = reg.render_png(finding.plugin, bytes) else {
        return Ok(None);
    };
    let ex = crate::ripper::extract_png(finding, &png, out_dir, seq)?;
    Ok(Some((ex, png)))
}

fn out_json(id: FindingId, format: &str, ex: &Extracted) -> Result<String, McpError> {
    serde_json::to_string(&ExtractOutput {
        finding_id: id.to_string(),
        format: format.to_string(),
        path: ex.path.display().to_string(),
        size: ex.size,
    })
    .map_err(|e| McpError::internal_error(format!("serialize extract output: {e}"), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_all_three() {
        assert_eq!(
            format_from_str("raw"),
            Ok(WireFormat::Bytes(ExtractFormat::Raw))
        );
        assert_eq!(
            format_from_str("prg"),
            Ok(WireFormat::Bytes(ExtractFormat::Prg))
        );
        assert_eq!(format_from_str("png"), Ok(WireFormat::Png));
    }

    #[test]
    fn format_unknown_is_err() {
        assert_eq!(
            format_from_str("gif"),
            Err("unknown extract format 'gif': use raw, prg or png".to_string())
        );
    }
}
