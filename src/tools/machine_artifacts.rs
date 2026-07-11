//! Artifact materialization for `machine_state` (P10): renders/writes the
//! PNG and/or raw screen dump, builds the wire output, and assembles the
//! MCP content blocks. Split from machine_state.rs for the 200-line budget
//! — same "write_artifacts helper" grouping the spec calls for (P10.md §7.5).

use std::path::Path;
use std::sync::atomic::Ordering;

use base64::Engine;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::ErrorData as McpError;

use super::machine_pretty;
use super::machine_state::{RawSnapshot, RenderFlags};
use super::machine_wire::{self, VideoMemoryOutput};
use super::{to_mcp, HatariServer};
use crate::error::Error;
use crate::types::MemImage;

impl HatariServer {
    /// Blocks: `[image?] + text(pretty) + text(JSON)` — `extract_as_png`'s pattern.
    pub(crate) async fn compose(
        &self,
        snap: RawSnapshot,
        session: &str,
        momentary: bool,
        flags: RenderFlags,
        work_dir: &Path,
    ) -> Result<CallToolResult, McpError> {
        let n = self.machine_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let (image_block, mem) = write_artifacts(&snap, flags, work_dir, n).await?;
        let out = machine_wire::build_output(session, momentary, snap.regs, &snap.video, mem);
        let json = serde_json::to_string(&out).map_err(|e| {
            McpError::internal_error(format!("serialize machine_state output: {e}"), None)
        })?;
        let mut blocks: Vec<ContentBlock> = image_block.into_iter().collect();
        blocks.push(ContentBlock::text(machine_pretty::pretty(&out)));
        blocks.push(ContentBlock::text(json));
        Ok(CallToolResult::success(blocks))
    }
}

/// `None` iff `!include_screen && !include_raw` (nothing to acquire).
async fn write_artifacts(
    snap: &RawSnapshot,
    flags: RenderFlags,
    work_dir: &Path,
    n: u64,
) -> Result<(Option<ContentBlock>, Option<VideoMemoryOutput>), McpError> {
    if !flags.include_screen && !flags.include_raw {
        return Ok((None, None));
    }
    let acquired = snap.mem.as_ref().and_then(|r| r.as_ref().ok());
    let (image_block, png_path) = match (flags.include_screen, acquired) {
        (true, Some(_)) => write_png(snap, work_dir, n).await?,
        _ => (None, None),
    };
    let raw_path = match (flags.include_raw, acquired) {
        (true, Some(mem)) => {
            let name = format!("machine-{n:04}-screen.bin");
            Some(write_bin(work_dir, name, mem.bytes()).await?)
        }
        _ => None,
    };
    let out = VideoMemoryOutput {
        len: acquired.map_or(0, MemImage::len),
        png_path,
        raw_path,
        note: snap.mem.as_ref().and_then(|r| r.as_ref().err()).cloned(),
    };
    Ok((image_block, Some(out)))
}

/// Renders + writes the PNG when a screen was acquired; `(None, None)` else.
async fn write_png(
    snap: &RawSnapshot,
    work_dir: &Path,
    n: u64,
) -> Result<(Option<ContentBlock>, Option<String>), McpError> {
    let acquired = snap.mem.as_ref().and_then(|r| r.as_ref().ok());
    let Some((mem, res)) = acquired.zip(snap.video.res) else {
        return Ok((None, None));
    };
    let png = super::view::render_screen(mem.bytes(), res, &snap.video.words)?;
    let path = write_bin(work_dir, format!("machine-{n:04}.png"), &png).await?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    Ok((Some(ContentBlock::image(b64, "image/png")), Some(path)))
}

/// Write `bytes` under `work_dir/name`; return the saved path (display form).
async fn write_bin(work_dir: &Path, name: String, bytes: &[u8]) -> Result<String, McpError> {
    let path = work_dir.join(name);
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|e| to_mcp(Error::Io(e)))?;
    Ok(path.display().to_string())
}
