//! P10 machine_state tool: one-call frozen snapshot of the emulated ST
//! (ADR-20). Composes registers (P1), the view-screen render (P4) and
//! ADR-11 freeze/thaw — no decode logic of its own.

use std::path::PathBuf;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::machine_wire::{self, VideoRegs};
use super::{to_mcp, wrong_state, HatariServer};
use crate::debug::{self, DebugChannel};
use crate::ripper::image::SCREEN_BYTES;
use crate::session::{memory, AnySession, HatariProcess, Running, Session};
use crate::types::{Addr, MemImage};

// `machine_wire` is `pub(crate)` (§5.3: internal wire types, same as
// rip_wire/view_wire), but `MachineStateArgs` is the input type of this
// `pub` tool fn and the live integration test (tests/machine_state_live.rs,
// an external crate) needs to construct one — the same "re-export the item
// through a pub path without making the whole module pub" pattern as
// `types/mod.rs`'s `pub use addr::Addr`.
pub use super::machine_wire::MachineStateArgs;

/// $FFFF8201: base hi/mid + sync + STE base low in one round-trip (§2).
const SHIFTER_BLOCK: Addr = Addr::new(0x00FF_8201);
const SHIFTER_BLOCK_LEN: usize = 13;
const RES_REG: Addr = Addr::new(0x00FF_8260);
const PALETTE_REGS: Addr = Addr::new(0x00FF_8240);
const PALETTE_LEN: usize = 32;

/// Everything read while frozen. `mem`: `None` iff nothing was requested;
/// `Some(Err(reason))` iff requested but unacquirable — machine-derived,
/// never an error (§7.2); `Some(Ok(_))` iff acquired. `pub(crate)` +
/// `pub(crate)` fields: consumed by `machine_artifacts::compose` across the
/// module split (200-line budget).
pub(crate) struct RawSnapshot {
    pub(crate) regs: debug::Registers,
    pub(crate) video: VideoRegs,
    pub(crate) mem: Option<Result<MemImage, String>>,
}

/// Testability seam: driven by a canned-response fake in unit tests.
async fn read_video(chan: &mut impl DebugChannel) -> crate::error::Result<VideoRegs> {
    let blk = debug::read_bytes(chan, SHIFTER_BLOCK, SHIFTER_BLOCK_LEN).await?;
    let res = debug::read_bytes(chan, RES_REG, 1).await?;
    let pal = debug::read_bytes(chan, PALETTE_REGS, PALETTE_LEN).await?;
    machine_wire::video_from_bytes(&blk, res.first().copied().unwrap_or(3), &pal)
}

async fn gather(proc: &mut HatariProcess, want_mem: bool) -> crate::error::Result<RawSnapshot> {
    let regs = debug::registers(proc).await?;
    let video = read_video(proc).await?;
    let ram = proc.config().ram_len();
    let mem = match (want_mem, skip_reason(want_mem, &video, ram)) {
        (false, _) => None,
        (true, Some(reason)) => Some(Err(reason)),
        (true, None) => Some(Ok(
            memory::acquire_range(proc, video.base, SCREEN_BYTES).await?
        )),
    };
    Ok(RawSnapshot { regs, video, mem })
}

/// `Some(reason)` iff `want_mem` but unacquirable (bad res / base OOB).
fn skip_reason(want_mem: bool, video: &VideoRegs, ram: usize) -> Option<String> {
    match (want_mem, video.res) {
        (false, _) => None,
        (true, None) => {
            Some("resolution register $FF8260 reads 3 (invalid) — screen skipped".to_string())
        }
        (true, Some(_)) if video.base.raw() as usize + SCREEN_BYTES > ram => Some(format!(
            "screen base {} + 32000 exceeds ST RAM ({ram} bytes) — screen skipped",
            video.base
        )),
        (true, Some(_)) => None,
    }
}

/// Momentary freeze: pause -> gather -> resume (ADR-20: gather error wins).
async fn freeze_gather_thaw(
    sess: Session<Running>,
    want_mem: bool,
) -> (AnySession, Result<RawSnapshot, McpError>) {
    let mut paused = match sess.pause().await {
        Ok(p) => p,
        Err((s, e)) => return (AnySession::Running(s), Err(to_mcp(e))),
    };
    let snap = gather(paused.process_mut(), want_mem).await.map_err(to_mcp);
    match paused.resume().await {
        Ok(r) => (AnySession::Running(r), snap),
        Err((p, e)) => (AnySession::Paused(p), snap.and(Err(to_mcp(e)))),
    }
}

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = machine_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "machine_state",
        description = "One-call status snapshot of the emulated Atari ST: CPU registers + SR flags, video state \
(resolution, 50/60 Hz, screen base from $FF8201/03/0D, 16-color palette as RGB), and the live \
32000-byte screen rendered inline as PNG. A running session is momentarily frozen and resumed \
so every value comes from one coherent instant; a paused session is read in place (and stays \
paused). include_screen=false skips the render (cheap registers-only call); \
include_raw_dump=true also writes the raw screen bytes to a file and returns its path (never \
the bytes)."
    )]
    pub async fn machine_state(
        &self,
        Parameters(a): Parameters<MachineStateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let flags = RenderFlags {
            include_screen: a.include_screen.unwrap_or(true),
            include_raw: a.include_raw_dump.unwrap_or(false),
        };
        let want_mem = flags.include_screen || flags.include_raw;
        let (work_dir, session, momentary, snap) = self.take_snapshot(want_mem).await?;
        self.compose(snap?, session, momentary, flags, &work_dir)
            .await
    }
}

/// Which artifacts to materialize (resolved `MachineStateArgs`). `pub(crate)`
/// + `pub(crate)` fields: read by `machine_artifacts::compose`.
#[derive(Clone, Copy)]
pub(crate) struct RenderFlags {
    pub(crate) include_screen: bool,
    pub(crate) include_raw: bool,
}

type SnapshotTaken = (PathBuf, &'static str, bool, Result<RawSnapshot, McpError>);

impl HatariServer {
    /// Lock held across the whole choreography (see `grab_screen`, view.rs).
    async fn take_snapshot(&self, want_mem: bool) -> Result<SnapshotTaken, McpError> {
        let mut guard = self.session.lock().await;
        let config = guard.config().clone();
        let work_dir = config.work_dir.clone();
        let (back, session, momentary, snap) =
            match std::mem::replace(&mut *guard, AnySession::new(config)) {
                AnySession::Stopped(s) => {
                    let name = s.state_name();
                    *guard = AnySession::Stopped(s);
                    return Err(wrong_state(name, "running or paused"));
                }
                AnySession::Paused(mut s) => {
                    let snap = gather(s.process_mut(), want_mem).await.map_err(to_mcp);
                    (AnySession::Paused(s), "paused", false, snap)
                }
                AnySession::Running(s) => {
                    let (back, snap) = freeze_gather_thaw(s, want_mem).await;
                    (back, "running", true, snap)
                }
            };
        *guard = back;
        Ok((work_dir, session, momentary, snap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        responses: Vec<String>,
        sent: Vec<String>,
    }

    impl DebugChannel for Fake {
        async fn debug(&mut self, cmd: &str) -> crate::error::Result<String> {
            self.sent.push(cmd.to_string());
            Ok(self.responses.remove(0))
        }
    }

    /// The three §2 raw `m` blocks verbatim (16-byte rounding + gutter),
    /// pinning that `parse_memdump` + truncation still feed
    /// `video_from_bytes` correctly.
    fn probe_blocks() -> Vec<String> {
        vec![
            "00FF8201: 03 00 22 00 00 00 00 00 00 02 00 00 00 00 0c 00   ..\".............."
                .to_string(),
            "00FF8260: 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................"
                .to_string(),
            "00FF8240: 00 00 07 77 05 55 00 02 00 04 00 06 00 17 00 27   ...w.U.........'\n\
00FF8250: 00 47 00 57 01 03 02 04 03 05 04 07 04 27 00 01   .G.W.........'.."
                .to_string(),
        ]
    }

    #[tokio::test]
    async fn read_video_commands_and_decode() {
        let mut fake = Fake {
            responses: probe_blocks(),
            sent: vec![],
        };
        let v = read_video(&mut fake).await.unwrap();
        assert_eq!(
            fake.sent,
            vec![
                "m $ff8201 13".to_string(),
                "m $ff8260 1".to_string(),
                "m $ff8240 32".to_string()
            ]
        );
        assert_eq!(v.base, Addr::new(0x032200));
        assert_eq!(v.sync_hz, 50);
        assert_eq!(v.res, Some(crate::ripper::image::ScreenRes::Low));
    }

    #[tokio::test]
    async fn read_video_invalid_res_degrades() {
        let mut blocks = probe_blocks();
        blocks[1] = "00FF8260: 03 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................"
            .to_string();
        let mut fake = Fake {
            responses: blocks,
            sent: vec![],
        };
        let v = read_video(&mut fake).await.unwrap();
        assert_eq!(v.res, None);
    }

    #[test]
    fn args_defaults() {
        let a: MachineStateArgs = serde_json::from_str("{}").unwrap();
        assert_eq!(a.include_screen, None);
        assert_eq!(a.include_raw_dump, None);
        assert!(a.include_screen.unwrap_or(true));
        assert!(!a.include_raw_dump.unwrap_or(false));
    }
}
