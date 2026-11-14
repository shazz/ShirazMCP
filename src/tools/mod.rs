pub mod analyze;
pub mod analyze_profile;
pub(crate) mod analyze_wire;
pub mod assemble;
pub(crate) mod assemble_wire;
pub mod audio;
pub(crate) mod audio_artifacts;
pub(crate) mod audio_wire;
pub mod bootsector;
pub(crate) mod bootsector_wire;
pub mod breakpoint;
pub(crate) mod breakpoint_wire;
pub mod cheat;
pub(crate) mod cheat_out;
pub(crate) mod cheat_wire;
pub mod debugger;
pub mod depack;
pub mod extract;
pub mod frames;
pub mod input;
pub(crate) mod input_wire;
pub mod profile_frame;
pub(crate) mod profile_frame_wire;
pub(crate) mod profile_wire;
// `InsnOut` + the shared instruction-mapping helper, split out of
// `analyze_wire.rs` (§7.2 fallback, P8) — used by `disasm` and `patch` readback.
pub(crate) mod insn_out;
pub mod lifecycle;
pub(crate) mod machine_artifacts;
pub(crate) mod machine_pretty;
pub mod machine_state;
pub(crate) mod machine_wire;
pub(crate) mod managed;
pub mod mem;
pub mod music;
pub(crate) mod repatch_wire;
pub(crate) mod resolve;
pub mod rip;
pub mod rip_sample;
pub(crate) mod rip_sample_wire;
pub(crate) mod rip_wire;
pub mod save_prg;
pub(crate) mod save_prg_wire;
mod snapshot_capture;
mod snapshot_prg;
pub(crate) mod snapshot_wire;
mod start_args;
pub mod symbols;
pub(crate) mod symbols_load;
pub(crate) mod symbols_wire;
pub mod view;
// Pure helpers split out of view.rs for the 200-line budget (§7.5 fallback,
// same pattern as rip.rs/rip_wire.rs) — not part of the public tool surface.
pub(crate) mod view_wire;
pub mod watch;
pub(crate) mod watch_wire;
pub mod xrefs;
pub(crate) mod xrefs_wire;
pub(crate) mod ym_pretty;
pub mod ym_state;
pub(crate) mod ym_wire;

use std::sync::{atomic::AtomicU64, Arc, Mutex as StdMutex};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool_handler, ErrorData as McpError, ServerHandler};
use tokio::sync::Mutex;

use crate::error::Error;
use crate::ripper::{default_registry, Registry};
use crate::session::{AnySession, SessionConfig};
use cheat_wire::CheatStore;
use rip_wire::ScanStore;

/// The MCP server: thin adapter over the layers (no business logic here —
/// ARCHITECTURE.md `tools/` contract). Single session for v1.
#[derive(Clone)]
pub struct HatariServer {
    session: Arc<Mutex<AnySession>>,
    shot_seq: Arc<AtomicU64>,
    registry: Arc<Registry>,
    /// ADR-5/ADR-10: the last scan's findings, resolved by `extract`. A
    /// **std** Mutex, deliberately: only ever locked for brief, non-`await`
    /// critical sections, so it cannot block the runtime; the `tokio`
    /// session mutex remains the only lock held across `.await`.
    last_scan: Arc<StdMutex<Option<ScanStore>>>,
    scan_seq: Arc<AtomicU64>,
    /// P15: the in-flight cheat search (candidate set + last snapshot) —
    /// std Mutex, like `last_scan`; bounded ≤ 4 MiB + 512 KiB, one at a time.
    cheat: Arc<StdMutex<Option<CheatStore>>>,
    cheat_seq: Arc<AtomicU64>,
    /// P26: the last successful `symbols(action="load")`'s listing mirror —
    /// std Mutex, like `cheat`/`last_scan`; cleared on `action="free"` and
    /// on session `stop`.
    symbols: Arc<StdMutex<Option<symbols_wire::SymbolStore>>>,
    extract_seq: Arc<AtomicU64>,
    // Per-tool output-file sequence counters (view_memory_as_screen P4,
    // profile/state P2, machine_state P10, audio artifacts P11).
    view_seq: Arc<AtomicU64>,
    profile_seq: Arc<AtomicU64>,
    state_seq: Arc<AtomicU64>,
    machine_seq: Arc<AtomicU64>,
    audio_seq: Arc<AtomicU64>,
}

impl HatariServer {
    pub fn new(config: SessionConfig) -> Self {
        Self {
            session: Arc::new(Mutex::new(AnySession::new(config))),
            shot_seq: Arc::new(AtomicU64::new(0)),
            registry: Arc::new(default_registry()),
            last_scan: Arc::new(StdMutex::new(None)),
            scan_seq: Arc::new(AtomicU64::new(0)),
            cheat: Arc::new(StdMutex::new(None)),
            cheat_seq: Arc::new(AtomicU64::new(0)),
            symbols: Arc::new(StdMutex::new(None)),
            extract_seq: Arc::new(AtomicU64::new(0)),
            view_seq: Arc::new(AtomicU64::new(0)),
            profile_seq: Arc::new(AtomicU64::new(0)),
            state_seq: Arc::new(AtomicU64::new(0)),
            machine_seq: Arc::new(AtomicU64::new(0)),
            audio_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Combines the per-file routers. `router = a + b + c` directly in the
    /// `#[tool_handler]` attribute breaks: quote-splicing appends
    /// `.call(tcc).await` right after the attribute expression's tokens, and
    /// postfix `.` binds tighter than `+`, so it lands on the last summand
    /// only (`rmcp-2.2-notes.md` fallback; P1.md §8.1's anticipated case).
    fn combined_router() -> ToolRouter<Self> {
        Self::lifecycle_router()
            + Self::debugger_router()
            + Self::mem_router()
            + Self::rip_router()
            + Self::depack_router()
            + Self::extract_router()
            + Self::view_router()
            + Self::music_router()
            + Self::analyze_router()
            + Self::profile_router()
            + Self::profile_frame_router()
            + Self::breakpoint_router()
            + Self::bootsector_router()
            + Self::assemble_router()
            + Self::machine_router()
            + Self::audio_router()
            + Self::save_prg_router()
            + Self::watch_router()
            + Self::frames_router()
            + Self::input_router()
            + Self::xrefs_router()
            + Self::cheat_router()
            + Self::ym_router()
            + Self::rip_sample_router()
            + Self::symbols_router()
    }
}

#[tool_handler(router = Self::combined_router())]
impl ServerHandler for HatariServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("shiraz-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Drives the Hatari Atari ST emulator. Lifecycle: \
                 start/stop/pause/resume/status. Debug: debug (raw debugger \
                 command), registers, screenshot. Memory: \
                 read_memory/write_memory/search_memory (hardware addresses \
                 come back annotated). Rip: rip (scan a RAM snapshot for \
                 artifacts — returns compact findings), find_picture (rip, \
                 pictures only), list_ripper_plugins, extract (materialize a \
                 finding to a file: raw bytes, runnable prg, or png with an \
                 inline image preview), depack (decompress a packed stream \
                 at an explicit address — the path for headerless schemes \
                 like zx0 that rip cannot auto-locate; the depacked blob \
                 and anything found inside it become extractable \
                 findings), view_memory_as_screen (render any \
                 RAM address as the ST screen, live or explicit palette), \
                 identify_music (SNDH header + YM2149 replay fingerprint: \
                 player name, confidence, replay address), bootsector \
                 (analyze a floppy boot sector — $1234 checksum, DOS BPB, \
                 loader/virus heuristics — from RAM or a .ST image). \
                 Symbols: symbols (register a label map — then profile/disasm/breakpoint take \
                 names like BlitSphere). \
                 Analyze: disasm (Capstone 68k over a snapshot, or Hatari \
                 live), profile (measured + blind/contended static cycles \
                 between two PCs, as raw cycles AND vbl_fraction / hbl_count \
                 raster budget), profile_frame (per-frame measured cycles \
                 over N whole VBL frames — min/avg/max + 50Hz fit + \
                 headroom; from/to are an address FILTER for 'your code', \
                 not a measurement window), breakpoint (managed one-shot \
                 run-to-break), state (save/load emulator snapshots). \
                 Status: machine_state \
                 (one-call frozen snapshot — CPU registers + SR flags, \
                 video resolution/sync/base/palette, and the live screen \
                 rendered inline; raw 32000-byte dump to disk on request). \
                 Audio: record_audio (record N real seconds of ST sound \
                 output to a WAV — fast-forward is suspended for the \
                 window), spectrogram (record or reuse a WAV, return an \
                 STFT spectrogram PNG inline + axis facts — see the music, \
                 e.g. before/after a patch). \
                 YM: ym_state (decode the live YM2149 PSG — per-channel \
                 note/frequency/volume/envelope, noise period, envelope shape, mixer; \
                 one non-intrusive register read, never touches $FF8800). \
                 Sample: rip_sample (dump the STE DMA-PCM sample the hardware is \
                 playing — the $FF8900 frame registers point right at the buffer — to a \
                 16-bit WAV; compose with spectrogram(wav_path=…) to see it). \
                 Patch: assemble (68000 source → machine code via vasm), \
                 patch (assemble at addr + write into RAM + \
                 readback-verified disasm — hot-patch with no PRG rebuild). \
                 Persist: save_prg (mode=\"repatch\": re-emit the running, patched program as a \
                 modified .PRG, reloc/BSS patches reported not persisted; mode=\"snapshot\": \
                 freeze the whole machine — RAM+CPU+hardware — into a self-restoring .PRG, no \
                 source file needed; paused sessions only). \
                 RE: watch (freeze when a memory value changes — reports \
                 the PC that changed it), xrefs (static cross-references \
                 to an address over a snapshot), run_frames (run exactly \
                 N VBL frames then freeze). \
                 Input: send_key (inject IKBD key/mouse events, optionally \
                 run N frames after, for headless interactive capture). \
                 Cheat: cheat_scan (iterative memory-change search — snapshot, run, filter \
                 changed/increased/eq…, list survivors: find any variable with no disassembly).",
            )
    }
}

/// Internal failure → MCP error. Category + context only; never leaks paths
/// beyond what the agent supplied, never a backtrace.
pub(crate) fn to_mcp(e: Error) -> McpError {
    match &e {
        Error::Timeout { .. }
        | Error::Parse { .. }
        | Error::Fifo { .. }
        | Error::Io(_)
        | Error::Session(_)
        | Error::Ripper { .. }
        | Error::Assembler { .. }
        | Error::ProcessExited { .. } => McpError::internal_error(e.to_string(), None),
    }
}

/// Wrong-state rejection (the ONLY `invalid_request` producer in P1).
pub(crate) fn wrong_state(current: &str, need: &str) -> McpError {
    McpError::invalid_request(format!("session is {current}, tool requires {need}"), None)
}
