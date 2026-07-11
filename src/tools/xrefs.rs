//! `xrefs` — static data cross-references over a RAM snapshot (P14 §7.4).
//! Pure analysis: acquires one whole-RAM snapshot, then delegates to
//! `analyze::xrefs` (no further emulator I/O).

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::mem::scan::parse_addr;
use super::xrefs_wire::{XrefOut, XrefsOutput};
use super::{to_mcp, wrong_state, HatariServer};
use crate::analyze::{self, XrefScan};
use crate::error::Error;
use crate::session::memory;
use crate::types::Addr;

// Re-exported so live integration tests (`tests/re_live.rs`) can call
// `HatariServer::xrefs` directly with typed args — `xrefs_wire` itself
// stays `pub(crate)` (analyze.rs/watch.rs convention).
pub use super::xrefs_wire::XrefsArgs;

#[tool_router(router = xrefs_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "xrefs",
        description = "Static data cross-references: scan a RAM snapshot \
                       for instructions whose operands reference a target \
                       address — answers 'what code reads/writes this \
                       table?' without running anything. Covers absolute \
                       (abs.l/abs.w) and PC-relative (d16(pc), d8(pc,ix)) \
                       addressing; register-indirect consumers (movea.l \
                       ptr,a0 / (a0)) are invisible to static scanning — \
                       use watch for those. target_len widens the target \
                       to a range (e.g. 32 for a palette). access: read | \
                       write (includes read-modify-write) | ea (address \
                       taken: lea/pea — the pointer escapes, consider \
                       watch) | exec (jsr/jmp). Default 32 hits, max 128."
    )]
    pub async fn xrefs(
        &self,
        Parameters(a): Parameters<XrefsArgs>,
    ) -> Result<Json<XrefsOutput>, McpError> {
        let target = parse_addr(&a.addr)?;
        let target_len = validate_target_len(a.target_len)?;
        let from = a
            .from
            .as_deref()
            .map(parse_addr)
            .transpose()?
            .unwrap_or(Addr::new(0));
        let limit = a.limit.unwrap_or(32).min(128) as usize;

        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        let mem = memory::acquire(proc).await.map_err(to_mcp)?;
        drop(guard);

        let len = a.len.map(|l| l as usize).unwrap_or(mem.len());
        let scan =
            analyze::xrefs(&mem, target, target_len, from, len, limit).map_err(map_xrefs_err)?;
        Ok(Json(scan_output(
            target, target_len, from, len as u32, scan,
        )))
    }
}

fn validate_target_len(v: Option<u32>) -> Result<u32, McpError> {
    let target_len = v.unwrap_or(2);
    if (1..=64).contains(&target_len) {
        Ok(target_len)
    } else {
        Err(McpError::invalid_params("target_len must be 1..=64", None))
    }
}

/// The one xrefs-specific mapping (§8): a bad scan range is the caller's
/// fault, not an internal failure. Every other `analyze::xrefs` error
/// (none currently possible beyond the range check) falls through to the
/// uniform `to_mcp`.
fn map_xrefs_err(e: Error) -> McpError {
    match &e {
        Error::Parse { context, .. } if context == "xrefs range" => {
            McpError::invalid_params(e.to_string(), None)
        }
        _ => to_mcp(e),
    }
}

fn scan_output(
    target: Addr,
    target_len: u32,
    from: Addr,
    scan_len: u32,
    scan: XrefScan,
) -> XrefsOutput {
    XrefsOutput {
        target: target.to_string(),
        target_len,
        from: from.to_string(),
        scan_len,
        hits: scan.hits.into_iter().map(XrefOut::from).collect(),
        truncated: scan.truncated,
        candidates_capped: scan.candidates_capped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_target_len_bounds() {
        assert!(validate_target_len(None).is_ok());
        assert!(validate_target_len(Some(1)).is_ok());
        assert!(validate_target_len(Some(64)).is_ok());
        assert!(validate_target_len(Some(0)).is_err());
        assert!(validate_target_len(Some(65)).is_err());
    }

    #[test]
    fn map_xrefs_err_range_is_invalid_params() {
        let e = Error::Parse {
            context: "xrefs range".to_string(),
            input: "$10".to_string(),
        };
        let mapped = map_xrefs_err(e);
        assert!(mapped.message.contains("xrefs range") || mapped.message.contains("$10"));
    }

    #[test]
    fn map_xrefs_err_other_falls_through() {
        let e = Error::Session("no live session".to_string());
        let mapped = map_xrefs_err(e);
        assert!(mapped.message.contains("session"));
    }
}
