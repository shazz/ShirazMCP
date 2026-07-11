//! `bootsector` — analyze a boot sector from a live RAM snapshot
//! (source=ram, via rip(kind=boot) → the TOS `_dskbufp` buffer) or a raw
//! `.ST` file (source=disk, sector 0). A façade like `identify_music` (P6):
//! findings stay in the last-scan store so `extract(finding_id,
//! as="raw"|"prg")` works immediately after. Structured facts
//! (checksum/BPB/tells) are recomputed from finding bytes via
//! `bootsector::analyze_sector` — never scraped from the label.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::bootsector_wire::*;
use super::rip_wire::{RipArgs, RIP_MAX_LIMIT};
use super::{to_mcp, HatariServer};
use crate::ripper::RipKind;
use crate::types::{Addr, MemImage};

#[tool_router(router = bootsector_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "bootsector",
        description = "Analyze an Atari ST floppy boot sector: the $1234 \
executability checksum, the DOS BPB geometry, and loader-vs-virus \
heuristics (vector hooks, self-copy loops, disk traps). source=\"ram\" \
reads the TOS disk buffer from a running/paused session; source=\"disk\" \
reads sector 0 of a raw .ST image at `path`. Heuristic — reports tells \
with a confidence, never a certain virus verdict. Pass the returned \
finding_id to extract (as=\"raw\" for the 512 bytes, \"prg\" for a \
runnable wrap)."
    )]
    pub async fn bootsector(
        &self,
        Parameters(a): Parameters<BootsectorArgs>,
    ) -> Result<Json<BootsectorOutput>, McpError> {
        let depth = a.depth;
        match validate_source(&a)? {
            Source::Ram => self.bootsector_ram(depth).await,
            Source::Disk(path) => self.bootsector_disk(path).await,
        }
    }
}

impl HatariServer {
    async fn bootsector_ram(&self, depth: Option<u8>) -> Result<Json<BootsectorOutput>, McpError> {
        let Json(page) = self
            .rip(Parameters(RipArgs {
                kind: Some("boot".to_string()),
                plugin: None,
                depth,
                limit: Some(RIP_MAX_LIMIT),
                offset: Some(0),
                scan_id: None,
            }))
            .await?;
        let guard = self
            .last_scan
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        boot_summary(&guard, page.scan_id)
            .map(Json)
            .map_err(|m| McpError::internal_error(m, None))
    }

    async fn bootsector_disk(&self, path: PathBuf) -> Result<Json<BootsectorOutput>, McpError> {
        let sector =
            tokio::task::spawn_blocking(move || crate::ripper::disk::read_boot_sector(&path))
                .await
                .map_err(|e| McpError::internal_error(format!("disk read task failed: {e}"), None))?
                .map_err(to_mcp)?;
        let reg = Arc::clone(&self.registry);
        let findings = tokio::task::spawn_blocking(move || {
            let mem = Arc::new(MemImage::new(sector, Addr::new(0)));
            reg.scan(mem, Some(RipKind::Boot), 0)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("scan task failed: {e}"), None))?;
        let scan_id = self.scan_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let out_dir = std::env::temp_dir().join("shirazmcp-bootsector");
        self.store_scan(scan_id, findings, out_dir);
        let guard = self
            .last_scan
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        boot_summary(&guard, scan_id)
            .map(Json)
            .map_err(|m| McpError::internal_error(m, None))
    }
}
