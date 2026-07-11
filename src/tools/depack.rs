//! `depack` — the explicit-address depack path (ADR-25): decompress a
//! HEADERLESS-scheme stream (zx0) the auto-engine cannot locate, then scan
//! the result exactly like `rip`. Wire structs + a pure core (unit-testable
//! without a session) + a thin adapter, mirroring `rip.rs`/`rip_wire.rs`.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::mem::scan::parse_addr;
use super::rip_wire::{page_findings, FindingOut, DEPTH_MAX, RIP_DEFAULT_LIMIT};
use super::HatariServer;
use crate::ripper::registry::DEPTH_DEFAULT;
use crate::ripper::{Finding, PluginRole, Registry};
use crate::types::{Addr, MemImage, PluginId};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DepackArgs {
    /// Bus address where the packed stream starts ("$4A000", "0x4A000", or decimal).
    pub addr: String,
    /// Window length from addr in bytes (default: to the end of RAM).
    pub len: Option<usize>,
    /// Depacker plugin id (see list_ripper_plugins), e.g. "zx0", "pack-ice".
    pub scheme: String,
    /// Depack→rescan depth for the unpacked buffer (default 2, max 4). 0 = detectors only.
    pub depth: Option<u8>,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct DepackOutput {
    /// Pass to `rip {scan_id}` to page findings beyond this first page.
    pub scan_id: u64,
    pub scheme: String,
    pub addr: String,
    /// Bytes of the window the depacker consumed — for headerless schemes
    /// (zx0) the exact packed-stream length; magic-scanning schemes
    /// attribute the whole window.
    pub consumed: usize,
    pub unpacked_len: usize,
    pub total: u32,
    /// First page (limit 32). findings[0] is ALWAYS the whole depacked
    /// blob (kind "raw", via=[scheme]) — extract it for the raw bytes.
    pub findings: Vec<FindingOut>,
    pub has_more: bool,
}

/// Window bounds within the snapshot. Err = agent-facing message.
fn window(mem: &MemImage, addr: Addr, len: Option<usize>) -> Result<(usize, usize), String> {
    let base = mem.base().raw();
    if addr.raw() < base {
        return Err(format!("{addr} is before the snapshot base {}", mem.base()));
    }
    let off = (addr.raw() - base) as usize;
    if off >= mem.len() {
        let top = Addr::new(base + mem.len() as u32);
        return Err(format!(
            "{addr} is outside the snapshot ({}..{top})",
            mem.base()
        ));
    }
    if len == Some(0) {
        return Err("len must not be 0".to_string());
    }
    let end = off
        .saturating_add(len.unwrap_or(mem.len() - off))
        .min(mem.len());
    Ok((off, end))
}

/// Depack + provenance finding + rescan, all pure CPU (runs inside
/// spawn_blocking). None = the scheme does not decode at the window start.
pub(crate) fn depack_and_scan(
    reg: &Registry,
    mem: &Arc<MemImage>,
    off: usize,
    end: usize,
    plugin: PluginId,
    depth: u8,
) -> Option<(usize, usize, Vec<Finding>)> {
    let (out, consumed) = reg.unpack_prefix(plugin, &mem.bytes()[off..end])?;
    let unpacked = Arc::new(MemImage::new(out, Addr::new(0)));
    let via = [plugin];
    let label = format!(
        "{plugin} depacked blob ({consumed} packed -> {} unpacked bytes)",
        unpacked.len()
    );
    let mut findings = vec![Finding::whole_buffer(&unpacked, plugin, &via, label)];
    findings.extend(reg.scan_via(unpacked, None, depth, &via));
    let unpacked_len = findings[0].len;
    Some((consumed, unpacked_len, findings))
}

/// `scheme` -> the registered depacker's `PluginId` (never construct one
/// from user input — the inner str must be `'static`).
fn resolve_scheme(registry: &Registry, scheme: &str) -> Result<PluginId, McpError> {
    let depackers: Vec<_> = registry
        .summaries()
        .into_iter()
        .filter(|s| s.role == PluginRole::Depacker)
        .collect();
    depackers
        .iter()
        .find(|s| s.id.as_str() == scheme)
        .map(|s| s.id)
        .ok_or_else(|| {
            let ids: Vec<&str> = depackers.iter().map(|s| s.id.as_str()).collect();
            McpError::invalid_params(
                format!("unknown depacker scheme '{scheme}': registered ids are {ids:?}"),
                None,
            )
        })
}

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = depack_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "depack",
        description = "Depack a compressed stream at an explicit RAM \
address and scan the result. This is the path for HEADERLESS schemes (zx0) \
that rip cannot auto-locate: find the stream start yourself (disasm, \
search_memory), then point this tool at it. Decompression and scanning run \
server-side; you get compact findings — findings[0] is always the whole \
depacked blob (kind raw): pass its id to extract for the bytes. Page more \
findings via rip with the returned scan_id. scheme: any depacker id from \
list_ripper_plugins."
    )]
    pub async fn depack(
        &self,
        Parameters(a): Parameters<DepackArgs>,
    ) -> Result<Json<DepackOutput>, McpError> {
        let plugin = resolve_scheme(&self.registry, &a.scheme)?;
        let addr = parse_addr(&a.addr)?;
        let depth = a.depth.unwrap_or(DEPTH_DEFAULT).min(DEPTH_MAX);
        let (consumed, unpacked_len, findings, out_dir) =
            self.depack_run(plugin, addr, a.len, depth).await?;

        let scan_id = self.scan_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let page = page_findings(scan_id, &findings, 0, RIP_DEFAULT_LIMIT);
        self.store_scan(scan_id, findings, out_dir);
        Ok(Json(DepackOutput {
            scan_id,
            scheme: a.scheme,
            addr: addr.to_string(),
            consumed,
            unpacked_len,
            total: page.total,
            findings: page.findings,
            has_more: page.has_more,
        }))
    }
}

impl HatariServer {
    /// Acquire a snapshot, slice the window, and run `depack_and_scan` in a
    /// blocking task — split out of `depack` to stay under the line ceiling
    /// (the rip.rs `acquire_snapshot`/`run_scan` precedent).
    async fn depack_run(
        &self,
        plugin: PluginId,
        addr: Addr,
        len: Option<usize>,
        depth: u8,
    ) -> Result<(usize, usize, Vec<Finding>, std::path::PathBuf), McpError> {
        let (mem, out_dir) = self.acquire_snapshot().await?;
        let mem = Arc::new(mem);
        let (off, end) = window(&mem, addr, len).map_err(|m| McpError::invalid_params(m, None))?;

        let reg = Arc::clone(&self.registry);
        let mem_task = Arc::clone(&mem);
        let result = tokio::task::spawn_blocking(move || {
            depack_and_scan(&reg, &mem_task, off, end, plugin, depth)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("depack task failed: {e}"), None))?;

        let (consumed, unpacked_len, findings) = result.ok_or_else(|| {
            McpError::invalid_params(
                format!(
                    "no {plugin} stream decodes at {addr} — check the address, or try \
                     another scheme from list_ripper_plugins"
                ),
                None,
            )
        })?;
        Ok((consumed, unpacked_len, findings, out_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ripper::default_registry;

    const Z2: &[u8] = &[0x87, 0x41, 0x55, 0x55, 0x80];

    fn buf_with_z2_at(off: usize, total: usize) -> Arc<MemImage> {
        let mut buf = vec![0u8; total];
        buf[off..off + Z2.len()].copy_from_slice(Z2);
        Arc::new(MemImage::new(buf, Addr::new(0)))
    }

    #[test]
    fn window_bounds() {
        let mem = MemImage::new(vec![0u8; 0x100], Addr::new(0x1000));
        assert_eq!(window(&mem, Addr::new(0x1000), None), Ok((0, 0x100)));
        assert_eq!(
            window(&mem, Addr::new(0x1080), Some(0x40)),
            Ok((0x80, 0xC0))
        );
        // len clamped at buffer end
        assert_eq!(
            window(&mem, Addr::new(0x1080), Some(0x1000)),
            Ok((0x80, 0x100))
        );
        assert!(window(&mem, Addr::new(0x0FFF), None).is_err());
        assert!(window(&mem, Addr::new(0x1100), None).is_err());
        assert!(window(&mem, Addr::new(0x1000), Some(0)).is_err());
    }

    #[test]
    fn depack_and_scan_happy() {
        let reg = default_registry();
        let mem = buf_with_z2_at(0x10, 0x40);
        let (consumed, unpacked_len, findings) =
            depack_and_scan(&reg, &mem, 0x10, 0x40, PluginId::new("zx0"), 2).unwrap();
        assert_eq!(consumed, 5);
        assert_eq!(unpacked_len, 6);
        assert_eq!(findings[0].len, 6);
        assert_eq!(findings[0].via(), [PluginId::new("zx0")]);
        assert!(findings[0].label.contains("5 packed -> 6"));
    }

    #[test]
    fn depack_and_scan_miss() {
        let reg = default_registry();
        let mem = buf_with_z2_at(0x10, 0x40);
        assert!(depack_and_scan(&reg, &mem, 0, 0x40, PluginId::new("zx0"), 2).is_none());
        assert!(depack_and_scan(&reg, &mem, 0x10, 0x40, PluginId::new("nope"), 2).is_none());
    }
}
