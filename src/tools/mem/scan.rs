//! Pure helpers for the memory tools: hex codecs, address parsing, and the
//! `search_memory` window/scan logic over an acquired `MemImage`.

use std::fmt::Write as _;

use rmcp::ErrorData as McpError;

use super::types::{AnnotationOut, SearchHit, SearchMemoryArgs, ANNOTATIONS_CAP, HIT_CONTEXT};
use crate::analyze;
use crate::types::{Addr, MemImage};

pub(crate) fn annotations_for(addr: Addr, len: usize) -> Vec<AnnotationOut> {
    (0..len as u32)
        .filter_map(|i| analyze::annotate(Addr::new(addr.raw() + i), None))
        .take(ANNOTATIONS_CAP)
        .map(|a| AnnotationOut {
            addr: a.addr.to_string(),
            label: format!("{} — {}", a.register, a.detail),
        })
        .collect()
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub(super) fn decode_hex(s: &str) -> std::result::Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("hex string must have even length".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| format!("invalid hex byte at offset {i}"))
        })
        .collect()
}

pub(crate) fn parse_addr(s: &str) -> Result<Addr, McpError> {
    Addr::parse(s).map_err(|e| McpError::invalid_params(e.to_string(), None))
}

pub(super) fn search_needle(a: &SearchMemoryArgs) -> Result<Vec<u8>, McpError> {
    let needle = match (&a.pattern_hex, &a.pattern_ascii) {
        (Some(h), None) => decode_hex(h).map_err(|m| McpError::invalid_params(m, None))?,
        (None, Some(s)) => s.as_bytes().to_vec(),
        _ => {
            let msg = "exactly one of pattern_hex/pattern_ascii is required";
            return Err(McpError::invalid_params(msg, None));
        }
    };
    if needle.is_empty() {
        return Err(McpError::invalid_params("pattern must not be empty", None));
    }
    Ok(needle)
}

pub(super) fn search_window(mem: &MemImage, a: &SearchMemoryArgs) -> Result<(u32, u32), McpError> {
    let top = mem.base().raw() + mem.len() as u32;
    let start = a
        .start
        .as_deref()
        .map(parse_addr)
        .transpose()?
        .map_or(mem.base().raw(), |a| a.raw());
    let end = a
        .end
        .as_deref()
        .map(parse_addr)
        .transpose()?
        .map_or(top, |a| a.raw());
    Ok((start.min(top), end.min(top)))
}

/// Slice `mem` to `[start, end)` (absolute bus addresses), returning the
/// window bytes plus the window's own base address.
pub(super) fn window_slice(mem: &MemImage, start: u32, end: u32) -> (&[u8], u32) {
    let base = mem.base().raw();
    let s = start.saturating_sub(base).min(mem.len() as u32) as usize;
    let e = end.saturating_sub(base).min(mem.len() as u32).max(s as u32) as usize;
    (&mem.bytes()[s..e], base + s as u32)
}

pub(super) fn find_hits(
    window: &[u8],
    needle: &[u8],
    window_base: u32,
    offset: u32,
    limit: u32,
) -> (Vec<SearchHit>, bool) {
    let mut it = memchr::memmem::find_iter(window, needle).skip(offset as usize);
    let hits = it
        .by_ref()
        .take(limit as usize)
        .map(|pos| build_hit(window, needle.len(), window_base, pos))
        .collect();
    (hits, it.next().is_some())
}

fn build_hit(window: &[u8], needle_len: usize, window_base: u32, pos: usize) -> SearchHit {
    let addr = Addr::new(window_base + pos as u32);
    let ctx_end = (pos + HIT_CONTEXT).min(window.len());
    SearchHit {
        addr: addr.to_string(),
        context_hex: hex_encode(&window[pos..ctx_end]),
        annotations: annotations_for(addr, needle_len),
    }
}
