//! P4 graphics tools: render arbitrary RAM as the ST screen, and the
//! picture-only rip façade. Wire structs live here (they have no other
//! consumer; rip_wire.rs is at its line budget).

use base64::Engine;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router, ErrorData as McpError};
use schemars::JsonSchema;
use serde::Deserialize;

use super::rip_wire::{RipArgs, RipOutput};
use super::view_wire::{parse_palette_arg, res_from_str};
use super::{to_mcp, wrong_state, HatariServer};
use crate::debug;
use crate::ripper::image::{
    decode_planar, palette_words, png_bytes, Palette, ScreenRes, SCREEN_BYTES,
};
use crate::session::memory;
use crate::types::Addr;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ViewScreenArgs {
    /// Screen-data address: "$78000", "0x78000", or decimal. Tip: the live
    /// screen base is at $FF8201 (hi) / $FF8203 (mid).
    pub addr: String,
    /// "low" | "medium" | "high". Omit to read the live resolution ($FF8260).
    pub res: Option<String>,
    /// Exactly 16 palette words, e.g. ["$0777","$0700",…]. Omit to read the
    /// live palette registers ($FF8240..$FF825E).
    pub palette: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindPictureArgs {
    /// Depack→rescan depth (default 2, max 4).
    pub depth: Option<u8>,
    /// Findings per page (default 32, max 128).
    pub limit: Option<u32>,
    /// Skip the first N findings (pagination).
    pub offset: Option<u32>,
}

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = view_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "view_memory_as_screen",
        description = "Render 32000 bytes of emulator RAM at addr as an ST \
screen and return the PNG inline plus the saved file path. res and palette \
default to the LIVE Shifter state ($FF8260 / $FF8240) — point it at the \
screen base from $FF8201/03, or at any suspected off-screen buffer."
    )]
    pub async fn view_memory_as_screen(
        &self,
        Parameters(a): Parameters<ViewScreenArgs>,
    ) -> Result<CallToolResult, McpError> {
        let addr =
            Addr::parse(&a.addr).map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let want_res = a.res.as_deref().map(res_from_str).transpose()?;
        let want_pal = a.palette.map(parse_palette_arg).transpose()?;
        let (mem, res, words, work_dir) = self.grab_screen(addr, want_res, want_pal).await?;
        let png = render_screen(mem.bytes(), res, &words)?;
        let n = self
            .view_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let path = work_dir.join(format!("view-{n:04}.png"));
        write_view_png(&path, &png).await?;
        Ok(view_result(&path, res, addr, &png))
    }

    #[tool(
        name = "find_picture",
        description = "Scan a snapshot of emulator RAM for pictures (Degas, \
NeoChrome, …) — a façade for rip(kind=picture). Returns compact findings, \
ranked by structural confidence and capped per detector; call extract with \
as=\"png\" to see one."
    )]
    pub async fn find_picture(
        &self,
        Parameters(a): Parameters<FindPictureArgs>,
    ) -> Result<Json<RipOutput>, McpError> {
        self.rip(Parameters(RipArgs {
            kind: Some("picture".to_string()),
            plugin: None,
            depth: a.depth,
            limit: a.limit,
            offset: a.offset,
            scan_id: None,
        }))
        .await
    }
}

/// `grab_screen`'s return: acquired screen bytes + the resolution/palette
/// that were effective (either the caller's or live-read) + the work dir.
type ScreenGrab = (
    crate::types::MemImage,
    ScreenRes,
    [u16; 16],
    std::path::PathBuf,
);

impl HatariServer {
    /// Lock the session once; gather screen bytes + effective res/palette.
    /// All awaits on `proc` happen inside this one lock scope; the guard
    /// drops before the (sync, CPU-bound) decode.
    async fn grab_screen(
        &self,
        addr: Addr,
        res: Option<ScreenRes>,
        pal: Option<[u16; 16]>,
    ) -> Result<ScreenGrab, McpError> {
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        check_screen_bounds(addr, proc.config().ram_len())?;
        let work_dir = proc.config().work_dir.clone();
        let res = match res {
            Some(r) => r,
            None => live_res(proc).await?,
        };
        let words = match pal {
            Some(w) => w,
            None => live_palette(proc).await?,
        };
        let mem = memory::acquire_range(proc, addr, SCREEN_BYTES)
            .await
            .map_err(to_mcp)?;
        Ok((mem, res, words, work_dir))
    }
}

fn check_screen_bounds(addr: Addr, ram_len: usize) -> Result<(), McpError> {
    if addr.raw() as usize + SCREEN_BYTES > ram_len {
        return Err(McpError::invalid_params(
            format!("{addr} + 32000 exceeds ST RAM ({ram_len} bytes)"),
            None,
        ));
    }
    Ok(())
}

pub(crate) fn render_screen(
    screen: &[u8],
    res: ScreenRes,
    words: &[u16; 16],
) -> Result<Vec<u8>, McpError> {
    let pal = Palette::from_words(words);
    let img = decode_planar(screen, res, &pal).map_err(to_mcp)?;
    png_bytes(&img).map_err(to_mcp)
}

async fn write_view_png(path: &std::path::Path, png: &[u8]) -> Result<(), McpError> {
    tokio::fs::write(path, png)
        .await
        .map_err(|e| to_mcp(crate::error::Error::Io(e)))
}

fn view_result(path: &std::path::Path, res: ScreenRes, addr: Addr, png: &[u8]) -> CallToolResult {
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    CallToolResult::success(vec![
        ContentBlock::image(b64, "image/png"),
        ContentBlock::text(format!(
            "saved: {} — {}x{} {} @ {addr}",
            path.display(),
            res.width(),
            res.height(),
            res.name()
        )),
    ])
}

/// $FF8260 bits 1-0: 0 low, 1 medium, 2 high, 3 invalid.
async fn live_res(proc: &mut crate::session::HatariProcess) -> Result<ScreenRes, McpError> {
    let b = debug::read_bytes(proc, Addr::new(0x00FF_8260), 1)
        .await
        .map_err(to_mcp)?;
    let v = b.first().copied().unwrap_or(3) & 3;
    ScreenRes::from_word(v as u16).ok_or_else(|| {
        McpError::internal_error("resolution register $FF8260 reads 3 (invalid)", None)
    })
}

/// Palette registers $FF8240..$FF825E — 16 big-endian words.
async fn live_palette(proc: &mut crate::session::HatariProcess) -> Result<[u16; 16], McpError> {
    let b = debug::read_bytes(proc, Addr::new(0x00FF_8240), 32)
        .await
        .map_err(to_mcp)?;
    palette_words(&b).ok_or_else(|| McpError::internal_error("short palette-register read", None))
}
