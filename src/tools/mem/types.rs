//! Wire schema for the memory tools (§8.4). Split out of `mem/mod.rs`
//! purely to keep the router/tool-method file under the line ceiling — this
//! is schema only, no behavior.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const READ_DEFAULT: u32 = 256;
pub const READ_MAX: u32 = 1024;
/// Max bytes per `write_memory` tool CALL — a token-budget/ergonomics cap,
/// unrelated to Hatari's own per-`memwrite`-line limit (`debug::mod::
/// MEMWRITE_CHUNK_BYTES`, §22): `debug::write_bytes` chunks internally to
/// whatever size the debugger console can actually parse, so any value up
/// to this cap is safe in one call.
pub const WRITE_MAX: usize = 256;
pub const SEARCH_DEFAULT_LIMIT: u32 = 16;
pub const SEARCH_MAX_LIMIT: u32 = 64;
pub const HIT_CONTEXT: usize = 16;
pub const ANNOTATIONS_CAP: usize = 32;

#[derive(Debug, Serialize, JsonSchema)]
pub struct AnnotationOut {
    pub addr: String,
    pub label: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadMemoryArgs {
    /// Address: "$FF8240", "0xff8240", or decimal.
    pub addr: String,
    /// Bytes to read (default 256, max 1024). Page with `next_addr`.
    pub count: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ReadMemoryOutput {
    pub addr: String,
    pub count: u32,
    /// Contiguous lowercase hex, 2 chars/byte ("00ff8240…").
    pub bytes_hex: String,
    /// Pass as `addr` to continue reading (pagination cursor).
    pub next_addr: String,
    pub annotations: Vec<AnnotationOut>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteMemoryArgs {
    pub addr: String,
    /// Even-length hex string, max 256 bytes ("0badf00d").
    pub bytes_hex: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WriteMemoryOutput {
    pub addr: String,
    pub written: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchMemoryArgs {
    /// Hex byte pattern ("4e75"). Exactly one of pattern_hex/pattern_ascii.
    pub pattern_hex: Option<String>,
    /// ASCII pattern, matched as raw bytes.
    pub pattern_ascii: Option<String>,
    /// Search window start (default $0).
    pub start: Option<String>,
    /// Search window end, exclusive (default: top of RAM).
    pub end: Option<String>,
    /// Max hits returned (default 16, max 64).
    pub limit: Option<u32>,
    /// Skip the first N hits (pagination: offset += limit).
    pub offset: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchHit {
    pub addr: String,
    /// `HIT_CONTEXT` bytes starting at the hit, lowercase hex.
    pub context_hex: String,
    pub annotations: Vec<AnnotationOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchMemoryOutput {
    pub hits: Vec<SearchHit>,
    pub offset: u32,
    /// More hits exist beyond `offset + hits.len()` — call again with a
    /// larger offset.
    pub has_more: bool,
}
