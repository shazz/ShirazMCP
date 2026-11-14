// `pub(crate)`: `analyze_wire.rs`/`breakpoint.rs`/`cheat_wire.rs` reuse
// `parse_addr`/`hex_encode`/`annotations_for` and `AnnotationOut` (§8.4) —
// no duplication across tool files.
pub(crate) mod scan;
pub(crate) mod types;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use self::scan::{find_hits, hex_encode, parse_addr, search_needle, search_window, window_slice};
use self::types::{READ_DEFAULT, READ_MAX, WRITE_MAX};
use super::{to_mcp, wrong_state, HatariServer};
use crate::debug;
use crate::session::memory;
use crate::types::Addr;

// Re-exported so live integration tests (`tests/assemble.rs`, P8;
// `tests/save_prg_live.rs`, P16) can call `HatariServer::{read,write,search}_memory`
// directly with typed args — `mem::types` stays `pub(crate)` (analyze.rs/assemble.rs convention).
pub use self::types::{
    ReadMemoryArgs, ReadMemoryOutput, SearchMemoryArgs, SearchMemoryOutput, WriteMemoryArgs,
    WriteMemoryOutput,
};

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = mem_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "read_memory",
        description = "Read emulator memory. Returns hex bytes plus \
                       annotations naming any ST hardware registers / \
                       system variables in the range. Default 256 bytes, \
                       max 1024 — use next_addr to page."
    )]
    pub async fn read_memory(
        &self,
        Parameters(a): Parameters<ReadMemoryArgs>,
    ) -> Result<Json<ReadMemoryOutput>, McpError> {
        let addr = parse_addr(&a.addr)?;
        let count = a.count.unwrap_or(READ_DEFAULT).min(READ_MAX);
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        proc.config()
            .check_addr(addr, 0)
            .map_err(|m| McpError::invalid_params(m, None))?;
        let bytes = debug::read_bytes(proc, addr, count as usize)
            .await
            .map_err(to_mcp)?;
        Ok(Json(ReadMemoryOutput {
            addr: addr.to_string(),
            count,
            bytes_hex: hex_encode(&bytes),
            next_addr: Addr::new(addr.raw().wrapping_add(count)).to_string(),
            annotations: scan::annotations_for(addr, count as usize),
        }))
    }

    #[tool(
        name = "write_memory",
        description = "Write bytes into emulator memory (max 256 per \
                       call). Bytes as an even-length hex string."
    )]
    pub async fn write_memory(
        &self,
        Parameters(a): Parameters<WriteMemoryArgs>,
    ) -> Result<Json<WriteMemoryOutput>, McpError> {
        let addr = parse_addr(&a.addr)?;
        let bytes =
            scan::decode_hex(&a.bytes_hex).map_err(|m| McpError::invalid_params(m, None))?;
        if bytes.len() > WRITE_MAX {
            let msg = format!("too many bytes: {} > {WRITE_MAX}", bytes.len());
            return Err(McpError::invalid_params(msg, None));
        }
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        proc.config()
            .check_addr(addr, 0)
            .map_err(|m| McpError::invalid_params(m, None))?;
        debug::write_bytes(proc, addr, &bytes)
            .await
            .map_err(to_mcp)?;
        Ok(Json(WriteMemoryOutput {
            addr: addr.to_string(),
            written: bytes.len() as u32,
        }))
    }

    #[tool(
        name = "search_memory",
        description = "Search all emulator RAM for a byte or ASCII pattern \
                       (server-side scan over a snapshot — the agent only \
                       sees compact hits: address + 16 context bytes + \
                       annotations). Default 16 hits; page with offset."
    )]
    pub async fn search_memory(
        &self,
        Parameters(a): Parameters<SearchMemoryArgs>,
    ) -> Result<Json<SearchMemoryOutput>, McpError> {
        let needle = search_needle(&a)?;
        let limit = a
            .limit
            .unwrap_or(types::SEARCH_DEFAULT_LIMIT)
            .min(types::SEARCH_MAX_LIMIT);
        let offset = a.offset.unwrap_or(0);
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        let mem = memory::acquire(proc).await.map_err(to_mcp)?;
        let (start, end) = search_window(&mem, &a)?;
        let (window, window_base) = window_slice(&mem, start, end);
        let (hits, has_more) = find_hits(window, &needle, window_base, offset, limit);
        Ok(Json(SearchMemoryOutput {
            hits,
            offset,
            has_more,
        }))
    }
}
