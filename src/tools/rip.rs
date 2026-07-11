//! `rip` / `list_ripper_plugins` — the ripper's MCP surface. Thin adapters:
//! validation + wire mapping live in `rip_wire.rs`; the actual scan logic
//! is `crate::ripper` (emulator-free, ADR-2). The `extract` tool moved to
//! `tools/extract.rs` in P4 (line budget: the png branch needs base64,
//! ContentBlock, and registry render, all disjoint from rip's deps).

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::rip_wire::*;
use super::{to_mcp, wrong_state, HatariServer};
use crate::ripper::registry::DEPTH_DEFAULT;
use crate::ripper::{Finding, Registry, RipKind};
use crate::types::MemImage;

fn normalize_kind(kind: &Option<String>) -> Result<Option<RipKind>, McpError> {
    kind.as_deref()
        .map(kind_from_str)
        .transpose()
        .map_err(|m| McpError::invalid_params(m, None))
}

fn validate_plugin(registry: &Registry, plugin: Option<&str>) -> Result<(), McpError> {
    match plugin {
        Some(p) if !registry.plugin_ids().iter().any(|id| id.as_str() == p) => {
            Err(McpError::invalid_params(
                format!("unknown plugin '{p}' — see list_ripper_plugins"),
                None,
            ))
        }
        _ => Ok(()),
    }
}

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = rip_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "rip",
        description = "Scan a snapshot of emulator RAM with \
every registered ripper plugin (parallel, server-side — the agent sees only \
compact findings: id, plugin, kind, offset, size, label, depack provenance). \
Packed regions are unpacked and rescanned up to `depth` levels. Findings \
stay in server memory: pass a finding id to `extract`. Default 32 findings \
per page; page with offset + the returned scan_id."
    )]
    pub async fn rip(
        &self,
        Parameters(a): Parameters<RipArgs>,
    ) -> Result<Json<RipOutput>, McpError> {
        validate_plugin(&self.registry, a.plugin.as_deref())?;
        let limit = a.limit.unwrap_or(RIP_DEFAULT_LIMIT).min(RIP_MAX_LIMIT);
        let offset = a.offset.unwrap_or(0);
        if let Some(scan_id) = a.scan_id {
            return self.page_stored_scan(scan_id, offset, limit);
        }
        let kind = normalize_kind(&a.kind)?;
        let depth = a.depth.unwrap_or(DEPTH_DEFAULT).min(DEPTH_MAX);
        let (mem, out_dir) = self.acquire_snapshot().await?;
        let scan_id = self.scan_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let mut findings = self.run_scan(mem, kind, depth).await?;
        if let Some(p) = &a.plugin {
            findings.retain(|f| f.plugin.as_str() == p.as_str());
        }
        let page = page_findings(scan_id, &findings, offset, limit);
        self.store_scan(scan_id, findings, out_dir);
        Ok(Json(page))
    }

    #[tool(
        name = "list_ripper_plugins",
        description = "List every registered \
ripper plugin: detectors (with the artifact kind they find), depackers, and \
deprotectors. Discovery call — use it to pick `kind`/`plugin` filters for \
`rip`."
    )]
    pub async fn list_ripper_plugins(&self) -> Result<Json<ListPluginsOutput>, McpError> {
        let plugins = self.registry.summaries().iter().map(plugin_out).collect();
        Ok(Json(ListPluginsOutput { plugins }))
    }
}

impl HatariServer {
    /// `pub(crate)`: also used by `tools/depack.rs` (§P12) — the store_scan
    /// precedent.
    pub(crate) async fn acquire_snapshot(&self) -> Result<(MemImage, PathBuf), McpError> {
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        let out_dir = proc.config().work_dir.join("ripped");
        let mem = crate::session::memory::acquire(proc)
            .await
            .map_err(to_mcp)?;
        Ok((mem, out_dir))
    }

    async fn run_scan(
        &self,
        mem: MemImage,
        kind: Option<RipKind>,
        depth: u8,
    ) -> Result<Vec<Finding>, McpError> {
        let reg = Arc::clone(&self.registry);
        let mem = Arc::new(mem);
        tokio::task::spawn_blocking(move || reg.scan(mem, kind, depth))
            .await
            .map_err(|e| McpError::internal_error(format!("scan task failed: {e}"), None))
    }

    /// `pub(crate)`: also called by `tools/bootsector.rs`'s disk-source path
    /// (§8.1, P7 — the one allowed edit to this file).
    pub(crate) fn store_scan(&self, scan_id: u64, findings: Vec<Finding>, out_dir: PathBuf) {
        let mut guard = self
            .last_scan
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *guard = Some(ScanStore {
            scan_id,
            findings,
            out_dir,
        });
    }

    fn page_stored_scan(
        &self,
        scan_id: u64,
        offset: u32,
        limit: u32,
    ) -> Result<Json<RipOutput>, McpError> {
        let guard = self
            .last_scan
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match guard.as_ref() {
            Some(store) if store.scan_id == scan_id => {
                Ok(Json(page_findings(scan_id, &store.findings, offset, limit)))
            }
            Some(store) => Err(McpError::invalid_params(
                format!(
                    "stale scan_id {scan_id}: latest scan is {}; re-run rip",
                    store.scan_id
                ),
                None,
            )),
            None => Err(McpError::invalid_params(
                "no scan in memory: run rip first",
                None,
            )),
        }
    }
}
