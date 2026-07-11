# Architecture

## Overview

**ShirazMCP** is a local [MCP](https://modelcontextprotocol.io/) server that drives
the [Hatari](https://hatari.tuxfamily.org/) Atari ST/STE emulator so an agent can
run, debug, freeze, screenshot, and **rip** Atari ST programs non-interactively.
It wraps Hatari's command-fifo debugger protocol behind clean, token-efficient
tools, and layers a **plugin-based ripping engine** (pictures, music players,
packers/depackers) modelled on the classic freezer cartridges (Ultimate Ripper,
Multiface ST). Design rationale lives in [`docs/BRAINSTORM.md`](docs/BRAINSTORM.md);
decisions in [`decisions.md`](decisions.md).

## Tech Stack

- **Language:** Rust (2021 edition, toolchain 1.92). Chosen for native
  ripping/scan performance and experimentation — *not* the orchestration path,
  which is I/O-bound (see `decisions.md`).
- **MCP server:** `rmcp` 2.2 + `tokio` (async runtime, process + fifo I/O).
- **Serialization / schemas:** `serde`, `serde_json`, `schemars`.
- **Imaging:** `image` (read Hatari PNGs; encode ST planar → PNG).
- **Fast scanning:** `memchr` (byte search) + `rayon` (parallel sweeps).
- **Disassembly:** `capstone` (M68K) for fast offline disassembly over a snapshot;
  Hatari's own `disasm` is used only for live single-address queries.
- **Audio (P11):** `hound` (read the WAVs Hatari records) + `rustfft`
  (STFT for the spectrogram tool); the DSP parameters replicate the proven
  STEIsNotDead pipeline (ADR-23).
- **External:** `hatari` binary (`/usr/bin/hatari`); an X display (`DISPLAY`,
  or `Xvfb` for headless — not yet installed); `vasmm68k_mot` (vasm Motorola
  syntax) for the `assemble`/`patch` tools — path via `SHIRAZMCP_VASM`,
  default `$HOME/projects/MJJ/bin/vasm/vasmm68k_mot`; absent binary
  degrades to a typed error, never a panic (ADR-18).

## Project Structure

```
ShirazMCP/
├── Cargo.toml
├── ARCHITECTURE.md          # this file
├── decisions.md             # ADRs
├── README.md
├── docs/
│   └── BRAINSTORM.md        # design rationale / heritage
├── src/
│   ├── main.rs              # rmcp bootstrap + tool registration
│   ├── error.rs            # error types (thiserror)
│   ├── types/
│   │   ├── mod.rs           # Offset, PluginId, MemImage
│   │   └── addr.rs          # Addr (ST bus address newtype)
│   ├── session/
│   │   ├── mod.rs           # Session<S> FSM: start/stop/pause/resume
│   │   ├── state.rs         # sealed SessionState/LiveState type-state markers
│   │   ├── any.rs           # AnySession — runtime dispatch for tools/
│   │   ├── run.rs           # Session<Paused>::run_until_break (ADR-15 managed run-to-break)
│   │   ├── fifo/
│   │   │   ├── mod.rs       # sentinel-synced debug<W>/write_wrapper/continue_and_wait (ADR-11/-15: W = fifo or stdin)
│   │   │   ├── log_tail.rs  # LogTail — polling tail of the Hatari log
│   │   │   └── sentinel.rs  # per-call-unique sentinel values (ADR-21)
│   │   ├── spawn/
│   │   │   ├── mod.rs       # HatariProcess: config, spawn, debug_with_timeout, wait_for_log, control_and_wait (P11)
│   │   │   ├── boot.rs      # spawn steps 1-5 (process, fifo wait, boot-gate, --alert-level fatal)
│   │   │   └── control.rs   # ADR-11 freeze()/thaw() + ADR-15 continue_until_break() + shutdown (P11 line-budget move)
│   │   ├── memory.rs        # acquire whole-RAM image for ripping
│   │   └── record.rs        # P11: soundout/recsound/fast-forward driver over the control fifo (ADR-23)
│   ├── audio/                # pure DSP, no session/tools/debug/ripper imports (P11, ADR-23)
│   │   ├── mod.rs           # AudioBuf + png_encode helper
│   │   ├── wav.rs            # hound WAV reader → mono f32 + WavInfo; PCM8→PCM16 writer (P20)
│   │   ├── magma.rs          # const 256-entry magma colormap LUT
│   │   ├── spectrogram.rs    # Hann/STFT (rustfft)/dB/colormap → PNG
│   │   └── waveform.rs       # min-max amplitude envelope → PNG
│   ├── debug/
│   │   ├── mod.rs           # debugger command wrappers
│   │   ├── basepage.rs      # GEMDOS basepage discovery — the live load address (P16)
│   │   ├── breakpoint.rs    # typed `b` (conditional CPU breakpoint) wrappers (P2)
│   │   ├── profile.rs       # Hatari CPU profiler driver + save-file parser (P2)
│   │   └── parse/
│   │       ├── mod.rs       # parse_err, fixture_block, re-exports
│   │       ├── cpureg.rs    # cpureg parsing
│   │       ├── values.rs    # evaluate/m (numeric extraction)
│   │       ├── status.rs    # symbols/screenshot (outcome lines)
│   │       ├── info_ym.rs   # info ym (PSG register dump) parsing (P17)
│   │       └── info_mfp.rs  # info mfp (MFP 68901 internal-struct dump) parsing (P22)
│   ├── analyze/
│   │   ├── mod.rs           # analysis over a MemImage / debug output
│   │   ├── disasm.rs        # embedded Capstone M68K (bulk/offline, P2)
│   │   ├── cycles/          # 68000 static cycle estimation (P2) + STE contention model (P18)
│   │   │   ├── mod.rs       # EaMode/ea_cost, round_st, estimate, vbl_fraction
│   │   │   ├── tables.rs    # const cycle-cost data (MOVE matrix, jmp/jsr/lea/pea/movem, fixed costs)
│   │   │   ├── decode.rs    # opcode classifier: MOVE/MOVEQ/ADD/SUB/AND/OR/EOR/CMP/shifts/branches
│   │   │   ├── decode_bits.rs # 0x0xxx nibble: bit ops + ORI/ANDI/…/CMPI immediate group
│   │   │   ├── decode_misc.rs # 0x4xxx misc bucket + 0x5xxx ADDQ/SUBQ/Scc/DBcc
│   │   │   ├── contention.rs # Region + display-region bus-scale factors + contended_cycles (P18)
│   │   │   ├── contention_bus.rs # bus/data-access classifier, nibbles 1-3/6-9/B-E (P18)
│   │   │   └── contention_bus_misc.rs # bus/data-access classifier, nibbles 0x0/0x4/0x5 (P18)
│   │   ├── annotate.rs      # address → ST register metadata (memory_map dataset)
│   │   ├── xrefs.rs         # static xrefs scan + writer_at backward decode (P14, ADR-27)
│   │   ├── xrefs_ops.rs     # Capstone op_str resolution + read/write/ea/exec classify (P14)
│   │   ├── xrefs_scan.rs    # byte-level candidate scan, split from xrefs.rs for the line ceiling (P14)
│   │   ├── cheat_set.rs     # cheat-search candidate bitset (P15)
│   │   ├── cheatscan.rs     # memory-diff search engine: Width/Predicate/seed/filter (P15)
│   │   ├── ym.rs            # pure YM2149 register decode (P17)
│   │   └── dmasnd.rs        # pure STE DMA-sound $FF8900 register decode (P20)
│   ├── ripper/
│   │   ├── mod.rs           # RipPlugin/Depacker traits + engine + registry
│   │   ├── image/           # ST planar decode helpers (shared, P4)
│   │   │   ├── mod.rs       # ScreenRes/Palette/is_ste/palette_words/neo_header_at
│   │   │   ├── render.rs    # decode_planar/png_bytes (split for the line ceiling, P10)
│   │   │   └── score.rs     # picture structure scorer: gate/rank picture findings (P21)
│   │   ├── disk.rs          # raw .ST sector-0 reader, path-validated (P7)
│   │   ├── prg_file.rs      # GEMDOS .PRG container parse: header/segments/reloc table, path-validated read (P16)
│   │   ├── repatch.rs       # RAM→file back-mapping: expected image, diff→plan, apply (P16)
│   │   ├── snapshot/        # freeze-the-machine .PRG builder (P22, ADR-38/-39)
│   │   │   ├── mod.rs       # CpuRegs/HwRegs/SnapshotFacts + build_snapshot_prg orchestration
│   │   │   ├── pack/        # SNAP1 codec (200-line budget split)
│   │   │   │   ├── mod.rs   # Run decomposition + the pack demote-retry search
│   │   │   │   └── codec.rs # stream encode/decode: build_from_runs/verify_inplace/unpack
│   │   │   ├── stub.rs      # committed stub.bin + field-offset patcher
│   │   │   ├── stub.s        # vasm source for the restore stub (assembly, not Rust budget)
│   │   │   └── stub.bin      # committed assembled blob (drift-guarded against stub.s)
│   │   └── plugins/         # ONE FILE PER FORMAT — the extension point
│   │       ├── degas.rs     # pi1/pi2/pi3, pc1/pc2/pc3 (P4)
│   │       ├── degas_rle.rs # Degas RLE codec + linear↔interleaved mapping, split from degas.rs (P21)
│   │       ├── neochrome.rs # .neo (P4)
│   │       ├── mpp.rs       # MPP multi-palette pictures: header, gates, detector (P13)
│   │       ├── mpp_decode.rs # MPP per-scanline palette bitstream + change engine (P13, ADR-26)
│   │       ├── ice_decode.rs # Pack-Ice bit-stream decoder (pack_ice.rs's helper, P5)
│   │       ├── pack_ice.rs  # Pack-Ice Depacker + Packer-kind region detector (P5)
│   │       ├── sndh_header.rs # SNDH header/tag parsing, pure over bytes (P6)
│   │       ├── sndh.rs      # SNDH blob detector (P6)
│   │       ├── ym_signatures.rs # masked-word fingerprint matcher (P6; DB in ym_sig_db.rs, P19)
│   │       ├── ym_sig_db.rs     # corpus-derived signature entries + provenance (P19 local sources; P24 SNDH archive, 20 entries)
│   │       ├── ym_player.rs # $FFFF8800/02 anchor scan + clustering (P6, ADR-16)
│   │       ├── boot_bpb.rs  # $1234 checksum + little-endian DOS BPB decode (P7)
│   │       ├── boot_heur.rs # disasm-based virus-tell heuristics (P7, ADR-17)
│   │       ├── bootsector.rs # BootSector detector: checksum+BPB+tells → class (P7)
│   │       ├── atomik_bits.rs # Atomik bit-stream reader + static tables (atomik_decode.rs's helper, P9)
│   │       ├── atomik_decode.rs # Atomik ATM5 chunk/match decoder (P9)
│   │       ├── atomik.rs    # Atomik ATM5 Depacker + region detector (P9, ADR-19)
│   │       ├── pp_decode.rs # PowerPacker PP20 bit-stream decoder (powerpacker.rs's helper, P9)
│   │       ├── powerpacker.rs # PowerPacker PP20 Depacker + region detector (derived end, P9, ADR-19)
│   │       ├── rnc_m1.rs    # RNC ProPack method-1 decoder: LE-word bits + canonical Huffman (P9)
│   │       ├── rnc_m2.rs    # RNC ProPack method-2 decoder: forward MSB-first prefix LZ (P9)
│   │       ├── rnc.rs       # RNC ProPack Depacker + region detector, CRC-16 gated (P9, ADR-19)
│   │       ├── zx0.rs       # ZX0 v2 Depacker (headerless — exact-gate auto, explicit via depack tool)
│   │       └── zx0_decode.rs # ZX0 bit-stream decoder (P12, ADR-25)
│   ├── assemble/             # external 68k assembler driver (P8, ADR-18)
│   │   ├── mod.rs           # vasm subprocess driver — argv-only, temp-file RAII, bounded diagnostics
│   │   └── source.rs        # pure org injection + org/rorg-conflict check + diagnostics parse
│   └── tools/               # thin MCP tool defs → call into layers
│       ├── mod.rs
│       ├── lifecycle.rs     # start / stop / pause / resume
│       ├── debugger.rs      # debug / registers / screenshot
│       ├── mem/              # read_memory / write_memory / search_memory
│       │   ├── mod.rs
│       │   ├── scan.rs      # pure helpers: parse_addr, hex_encode, search window/scan
│       │   └── types.rs     # wire schema
│       ├── rip.rs           # rip / list_ripper_plugins
│       ├── rip_wire.rs      # wire schema + pure helpers
│       ├── depack.rs        # depack tool: explicit-address depack + rescan (P12, ADR-25)
│       ├── extract.rs       # extract (raw / prg / png, P4)
│       ├── view.rs          # view_memory_as_screen / find_picture (P4)
│       ├── view_wire.rs     # wire schema + pure helpers
│       ├── music.rs         # identify_music (SNDH + ym-player façade, P6)
│       ├── analyze.rs       # disasm tool (P2)
│       ├── analyze_profile.rs # profile tool (P2, split from analyze.rs for the line ceiling)
│       ├── analyze_wire.rs  # wire schema + annotate_ops/track_psg/parse_live_line (P2)
│       ├── profile_wire.rs  # profile wire schema + region plumbing + build_output (P18, moved from analyze_wire.rs)
│       ├── insn_out.rs      # InsnOut + insns_out mapper, shared by disasm + patch readback (P8)
│       ├── breakpoint.rs    # breakpoint / state tools (grouped, P2)
│       ├── breakpoint_wire.rs # wire schema + save/load snapshot helpers (P2)
│       ├── managed.rs       # managed run-to-breakpoint choreography (ADR-15, P2)
│       ├── bootsector.rs    # bootsector tool: ram (rip façade) / disk sources (P7)
│       ├── bootsector_wire.rs # wire schema + boot_summary + validate_source (P7)
│       ├── assemble.rs      # assemble / patch tools (P8, ADR-18)
│       ├── assemble_wire.rs # wire schema + from_result/failed_assembly/readback_listing (P8)
│       ├── machine_state.rs # machine_state: one-call frozen ST status snapshot (P10, ADR-20)
│       ├── machine_wire.rs  # wire schema + video-register decode (P10)
│       ├── machine_pretty.rs # text pretty-printer, split from machine_wire.rs for the line ceiling (P10)
│       ├── machine_artifacts.rs # PNG/raw-dump materialization + block assembly, split from machine_state.rs (P10)
│       ├── audio.rs         # record_audio / spectrogram tools (P11, ADR-23)
│       ├── audio_wire.rs    # wire schema + pure validation/mapping (P11)
│       ├── audio_artifacts.rs # spectrogram/waveform PNG materialization + block assembly, split from audio.rs (P11)
│       ├── save_prg.rs      # save_prg: mode dispatch (repatch P16 / snapshot P22) + the repatch body
│       ├── save_prg_wire.rs # args + mode/path validation + the untagged SavePrgOutput enum (P16/P22)
│       ├── repatch_wire.rs  # mode=repatch output shape + region plumbing + summarize, split from save_prg_wire.rs (P22)
│       ├── snapshot_prg.rs  # mode=snapshot: Paused gate + one-lock-hold capture (P22 §8.3, ADR-39)
│       ├── snapshot_capture.rs # pure register/hardware-block → CpuRegs/HwRegs mapping, split from snapshot_prg.rs (P22)
│       ├── snapshot_wire.rs # mode=snapshot wire schema + the fixed not_restored list (P22)
│       ├── watch.rs         # watch: managed value-change breakpoint + writer backscan (P14, ADR-27)
│       ├── watch_wire.rs    # wire schema + condition/value-format helpers (P14)
│       ├── frames.rs        # run_frames: exact VBL frame stepping (tool + wire in one file, P14)
│       ├── xrefs.rs         # xrefs: static cross-reference scan tool (P14)
│       ├── xrefs_wire.rs    # wire schema + mode/access string mapping (P14)
│       ├── cheat.rs         # cheat_scan: iterative memory-change search (P15)
│       ├── cheat_wire.rs    # input side: args, CheatStore, parse/validate helpers (P15)
│       ├── cheat_out.rs     # output side: CheatScanOutput/CandidateOut, pagination (P15, ADR-29)
│       ├── ym_state.rs      # ym_state: live YM2149 decode (P17)
│       ├── ym_wire.rs       # wire schema + build (P17)
│       ├── ym_pretty.rs     # text pretty-printer, split from ym_wire.rs for the line ceiling (P17)
│       ├── rip_sample.rs    # rip_sample: STE DMA-PCM sample ripper (P20, ADR-20 momentary freeze)
│       └── rip_sample_wire.rs # wire schema + plan/build helpers (P20)
└── tests/
    ├── integration.rs       # drives Hatari + ../MJJ demo end-to-end
    ├── bootsector.rs        # boot-sector checksum/BPB/heuristics fixtures (P7)
    ├── ym_sig_corpus.rs     # ignored: SNDH-archive fingerprint regression (P24; needs refs/sndh_lf)
    ├── analyze_live.rs      # P2: disasm/profile/breakpoint/state against a live MJJ session
    ├── re_live.rs           # P14: watch/xrefs/run_frames against a live MJJ session (ignored)
    ├── cheat_live.rs        # P15: cheat_scan against a live MJJ session (ignored)
    ├── assemble.rs          # P8: vasm-gated goldens + live patch round-trip (ignored)
    ├── audio_live.rs        # P11: record_audio/spectrogram against a live MJJ session (ignored)
    ├── rip_sample_live.rs   # P20: rip_sample against a staged DHS Uhane session (ignored)
    └── snapshot_prg_live.rs # P22: save_prg mode=snapshot freeze→emit→reboot→verify (ignored)
```

## Layer Responsibilities

### `session/` — Hatari lifecycle & I/O
**Does:** spawn/kill the Hatari process, create-and-wait the cmd-fifo, tail the log,
send commands **synchronized by sentinel** (no blind `sleep`s), acquire RAM images.
**Must NOT:** know anything about ripping, tool schemas, or ST file formats.

The sentinel is the key reliability primitive — send the real command, then a
marker whose log echo signals completion. `HatariProcess` owns **two control
planes**, selected by session state (ADR-11):

| State | Channel | Command written | Confirmed by |
|---|---|---|---|
| Running | fifo | `hatari-debug <cmd>` | fifo sentinel round-trip |
| Paused | Hatari's stdin (piped, held open for the session's life) | `<cmd>` (raw, no wrapper prefix) | stdin sentinel round-trip |

`fifo::debug<W>` is generic over `W: AsyncWrite + Unpin + Send` (`tokio::fs::File`
for the fifo, `tokio::process::ChildStdin` while frozen) so both channels share one
sentinel/log-tail implementation; `HatariProcess::debug_with_timeout` just picks `W`
and the wrapper prefix based on `self.control`:

```rust
// src/session/fifo/mod.rs
pub async fn debug<W: AsyncWrite + Unpin + Send>(
    writer: &mut W, log: &mut LogTail, prefix: &str, cmd: &str, timeout_ms: u64, what: &str,
) -> Result<String> {
    write_line(writer, &format!("{prefix}{cmd}")).await?;      // e.g. "hatari-debug evaluate pc"
    let lines = read_sentinel_with_retry(writer, log, prefix, what, ..).await?;
    Ok(strip_protocol_lines(lines))   // drops echoes, blanks, AND stray sentinel echoes
}
```

**`pause`/`resume` do not use `hatari-stop`/`hatari-cont`** — sending `hatari-stop`
over the fifo permanently deadlocks Hatari 2.4.1 (the fifo is only serviced from the
running main loop; `hatari-stop` halts that loop without entering the debugger).
Instead (ADR-11): `pause()` arms a one-shot breakpoint over the fifo
(`hatari-debug b pc > 0 :once`, no sentinel — it would queue behind the freeze and
replay on resume) and confirms the freeze with a sentinel round-trip **over stdin**
(DebugUI reads stdin once the CPU is parked). `resume()` writes `c` to stdin (the
last stdin write of the episode — nothing may follow it) and confirms with a fifo
sentinel round-trip. `hatari-stop` must never be written to the fifo, by anything,
ever — enforced by convention and a grep-gate (`grep -rn "hatari-stop" src/` → no
hits), not by the type system.

**ADR-21 addendum:** `freeze()` and a fired managed breakpoint
(`continue_until_break`, ADR-15) each drain stray stdin-entry text with one
extra, discarded sentinel round-trip right after their own confirm — text
that arrives in the same poll as, but after, a confirm sentinel's echo is
otherwise left queued in `LogTail::pending` and corrupts the NEXT real
command. Every sentinel round-trip also gets its own unique value
(`fifo/sentinel.rs`: `$dead{counter:04x}`, not a fixed `$DEADBEEF`) so a
stale echo left over from an earlier call's `SENTINEL_RETRY_BUDGET` retry
can never satisfy a later call's completion check — confirmed live under
CPU contention to be the deeper cause of truncated `savebin`/
`memory::acquire` reads that the drain alone does not close.

`record.rs` (P11) drives Hatari's native WAV recorder over the control
fifo — pinned wire strings, fast-forward suspend/restore, real-time record
window; synchronization is per-command log echoes, not the debugger
sentinel (ADR-23).

### `debug/` — typed debugger operations
**Does:** wrap `evaluate`, `disasm`, `cpureg`, `m` (memdump), symbols; `breakpoint.rs`
wraps `b` (arm/list/remove/clear conditional CPU breakpoints, P2); `profile.rs`
drives Hatari's CPU profiler (`profile on/save/off`) and parses its per-address save
file. All parse Hatari's textual log output into typed values. Encapsulates ST
quirks: `$` hex, `m` refuses indirect (do `evaluate (ptr)` → literal `m`), per-run
load addresses. **Must NOT:** touch the process/fifo directly (goes through
`session`).

### `analyze/` — disassembly, profiling, annotation
**Does:** disassemble a `MemImage` region with embedded Capstone (`disasm.rs`, fast,
offline, no round-trips); statically estimate 68000 cycle cost of a decoded
instruction stream — `cycles/`, blind base estimate + `contention.rs`'s STE Shifter
bus model (P18): memory-access cycles scale by display region, internal cycles
never; the profile tool reports blind + contended, and Hatari's measured profiler
remains ground truth (split into `tables.rs` const data and `decode.rs`/
`decode_bits.rs`/`decode_misc.rs` opcode classifiers plus their P18 bus-cost mirrors
`contention_bus.rs`/`contention_bus_misc.rs`, each an opcode nibble range, to stay
under the file-size ceiling); annotate any address with its ST hardware register
(`$FF8240 → "palette register 0"`) from the `memory_map` dataset.
Hatari's own *measured* profiler counts are read by `debug/profile.rs` — `analyze/`
only supplies the static estimate that measurement is compared against. `cheat_set.rs`/
`cheatscan.rs` (P15) add the cheat-search engine: a candidate bitset plus a pure
seed/filter diff over `MemImage` pairs, same purity contract as the rest of the layer.
`ym.rs` (P17) decodes a raw 16-byte PSG register dump into per-channel
frequency/note/volume, noise, envelope and mixer state — pure math ported from
STEIsNotDead's `ym2149.py`, same purity contract as the rest of the layer.
**Must NOT:** own the emulator connection — it reads snapshots/log output the other
layers provide.

Annotation is **enrichment, not a tool**: `read_memory`, `disasm`, and
`search_memory` results carry an optional `annotations` field so hardware accesses
are self-describing. The annotator is PSG-aware (tracks the last `$FF8800` select to
name the target register).

```rust
// src/analyze/annotate.rs
pub struct Annotation { pub addr: Addr, pub register: &'static str, pub detail: String }

pub fn annotate(addr: Addr) -> Option<Annotation> { /* index into memory_map dataset */ }
```

### `ripper/` — plugin-based extraction engine
**Does:** hold the plugin registry, run detectors + depackers over a RAM image,
recurse into unpacked buffers, decode ST planar graphics. **Must NOT:** talk to
Hatari (it receives an already-acquired `Arc<MemImage>`). This keeps ripping
pure, unit-testable on fixture buffers, and independent of the emulator. P6:
`ripper` may call `analyze` (pure disasm over `MemImage`) to confirm the
YM2149 replay anchor; `analyze` must never import `ripper`. P7: `ripper::disk`
reads sector 0 of a raw `.ST` image (path-validated); the `bootsector`
detector analyzes it and, over a whole-RAM snapshot, the `_dskbufp` (`$4C6`)
TOS disk buffer.

Two small traits are the entire extension contract — **add a format = one file
implementing one trait + one registry line:**

```rust
// src/ripper/mod.rs
pub enum RipKind { Picture, Music, Sprite, Packer, Boot, Raw }

/// `offset`/`len` are into whichever buffer the finding was found in (the
/// root image when `via()` is empty, a depacked buffer otherwise); the
/// engine attaches the source `Arc<MemImage>` and the `via` depack
/// provenance chain after a plugin returns (ADR-5/ADR-10), so `extract` can
/// slice the exact bytes later without re-running depackers.
pub struct Finding { pub plugin: PluginId, pub kind: RipKind,
                     pub offset: Offset, pub len: usize, pub label: String,
                     pub confidence: Option<u8> } // structure score, P21

/// A detector. Scans a memory image, returns zero or more findings.
pub trait RipPlugin: Send + Sync {
    fn id(&self) -> PluginId;                // "degas-pi1"
    fn kind(&self) -> RipKind;
    fn describe(&self) -> &'static str;      // shown by list_ripper_plugins
    fn scan(&self, mem: &MemImage) -> Vec<Finding>;
    /// Render this plugin's bytes as PNG. Default `None` (P4).
    fn render_png(&self, bytes: &[u8]) -> Option<Vec<u8>> { None }
}

/// A transformer. If `data` is packed by this scheme, return unpacked bytes.
/// The engine re-runs all detectors over the result (bounded depth).
pub trait Depacker: Send + Sync {
    fn id(&self) -> PluginId;                // "pack-ice"
    fn describe(&self) -> &'static str;
    fn try_unpack(&self, data: &[u8]) -> Option<Vec<u8>>;
}
```

P5 ships the first real depacker (Pack-Ice, both `ICE!`/`Ice!` headers); the
depack→rescan recursion is proven end-to-end in `tests/depack_rescan.rs`
(ADR-14). P9 adds three more depacker pairs (Atomik `ATM5`, PowerPacker
`PP20`, RNC ProPack methods 1/2 with CRC-16 gates); each is proven by a
packed-PI1 recursion test (ADR-19). P12 adds the first headerless depacker
(ZX0 v2): the auto-engine unpacks only whole-buffer-exact streams; arbitrary
in-RAM streams go through the `depack` tool's prefix-tolerant seam
(`Depacker::try_unpack_prefix`, ADR-25). P13 adds `mpp`, a third picture
detector alongside `degas`/`neochrome` — multi-palette pictures with a
per-scanline palette-rewrite renderer (ADR-26). P21 adds structure-scored
picture detection — `image::score` gates Degas candidates and annotates
NeoChrome, findings carry `confidence`, and picture plugins rank-and-cap
their own output (ADR-37). P22 adds `ripper::snapshot`, a pure *builder*
(not a `RipPlugin`/`Depacker`): frozen RAM image + `CpuRegs`/`HwRegs`
structs in, a self-restoring `.PRG`'s bytes + facts out. `snapshot::pack`
is SNAP1, a purpose-built codec whose packer proves in-place decodability
at pack time against the committed restore-stub binary (`stub.rs`/
`stub.s`/`stub.bin`, drift-guarded by a vasm-gated test) — see ADR-38/-39.

A third trait, `Deprotect` (ADR-6), is the deprotection seam: `detect(&[u8])
-> Option<ProtectionInfo>` lets the engine *report* a recognized protection
scheme as a `Finding`; `strip(&[u8]) -> Result<Vec<u8>>` exists on the trait
but is never called by the engine (P3 ships only `NoopDeprotect`). All three
traits register through `registry::default_registry()`, the single point
where "add a format" becomes "one file + one line". Picture plugins also
implement `render_png`; `extract as=png` resolves the producing plugin
through the registry (ADR-12).

The engine ties them together — detect, depack, recurse:

```rust
// src/ripper/registry.rs
impl Registry {
    pub fn scan(&self, mem: Arc<MemImage>, kind: Option<RipKind>, depth: u8) -> Vec<Finding> {
        let mut out: Vec<Finding> = self.detectors.par_iter()   // rayon
            .filter(|p| kind.is_none_or(|k| p.kind() == k))
            .flat_map_iter(|p| p.scan(&mem))
            .collect();
        if depth > 0 {
            for d in &self.depackers {
                if let Some(bytes) = d.try_unpack(mem.bytes()) {
                    let child = Arc::new(MemImage::new(bytes, Addr::new(0)));
                    out.extend(self.scan(child, kind, depth - 1));
                }
            }
        }
        out
    }
}
```

### `assemble/` — external 68k assembler (vasm)
**Does:** drive `vasmm68k_mot` over a temp source file (argv vector, no
shell), inject `org`, parse/sanitize/bound diagnostics, return raw bytes.
**Must NOT:** touch the session/fifo, be imported by anything but `tools/` —
it is a leaf like `analyze`, but async (subprocess I/O), which is exactly
why it is not *in* `analyze` (that layer stays pure and synchronous).

### `audio/` — pure DSP (P11)
**Does:** decode WAV to mono f32; compute STFT spectrograms and waveform
PNGs — pure, synchronous, deterministic. **Must NOT:** touch the
session/fifo, import `tools`/`session`/`ripper`, or perform any I/O beyond
the byte buffers it is given (file reads happen in `tools`, via
`wav::read_mono`'s path — the one seam, kept in this layer for hound's
streaming reader). A leaf like `analyze`, enforced by a grep-gate
(`grep -rn "use crate::\(session\|tools\|debug\|ripper\)" src/audio/` → no
hits, ADR-23).

### `tools/` — MCP surface
**Does:** define `rmcp` tools with clear, ST-aware descriptions; validate params;
call into the layers; return **token-efficient** results (paginated hits, images as
content blocks). **Must NOT:** contain business logic — tools are thin adapters.
Related verbs are grouped into one tool via an `action` param (`breakpoint`,
`state`, `cheat_scan`) to conserve context. `assemble`/`patch` (P8) are the exception to
"tools are thin": `patch` orchestrates three existing layers in sequence
(`assemble` → `debug::write_bytes` → `session::memory::acquire_range` →
`analyze::disasm`) but adds no business logic of its own — it is choreography,
not a new capability, mirroring `managed.rs`'s run-to-breakpoint pattern.
`machine_state` (P10) composes pause/resume + registers + the view-screen
render into a single coherent snapshot (momentary freeze, ADR-20) and is
choreography-not-capability in the same sense as `patch`. `depack` (P12) is
the explicit-address counterpart to `rip`: the agent supplies an address and
scheme, the tool slices a window and scans the depacked result exactly like
`rip` (ADR-25). P14 adds three interactive RE primitives on the same
ADR-15 managed lifecycle as `breakpoint`/`profile`: `watch` (a managed
value-change breakpoint, since Hatari has no bus-access breakpoints —
ADR-27 — plus a best-effort backward decode of the writing instruction),
`run_frames` (exact VBL frame stepping via a managed breakpoint on
Hatari's `VBL` variable), and `xrefs` (pure static analysis — one RAM
acquire, then `analyze::xrefs` — no further session mutation, unlike its
two siblings). P15 adds `cheat_scan` (action=new|filter|list|reset): an
iterative memory-change search over server-held snapshot pairs — `new`/
`filter` acquire and diff (pure engine in `analyze::cheatscan`), `list`/
`reset` are pure over the store, and the tool never changes session state
(ADR-29). Its wire schema splits input (`cheat_wire.rs`) from output
(`cheat_out.rs`, the `insn_out.rs` precedent) purely for the file-size
ceiling — `cheat.rs` itself stays choreography-only. P16 adds `save_prg`
(choreography-not-capability, Paused-only): it composes GEMDOS basepage
discovery (`debug::basepage`) + `session::memory::acquire_range` + pure
diff/apply (`ripper::repatch`) to re-emit the running, patched program as
a modified copy of its original `.PRG`. `ym_state` (P17) is the audio
sibling of `machine_state` — one `info ym` debugger round-trip (a
non-intrusive dump of Hatari's emulated PSG register file; `$FF8800` is
never written) decoded by `analyze::ym`; unlike `machine_state` it needs no
momentary freeze because a single remote debugger command is coherent by
construction (the ADR-33 no-freeze decision). `rip_sample` (P20) is the DMA
sibling of `ym_state`: register-driven sample dump under the ADR-20
momentary freeze, choreography-not-capability. P25 adds `profile_frame`:
the frame-windowed sibling of `profile` — the measurement window is N
single-VBL managed legs (`frames.rs` primitives) and the PC range is
demoted to a pure address filter; one `profile save` per paused boundary
exploits Hatari's reset-on-continue profiler semantics (ADR-42).
Choreography-not-capability, like its siblings.

## Data Flow

Example — *"rip the picture on screen right now"*:

1. Agent calls `rip(kind="picture")`.
2. `tools/rip.rs` → `session::memory::acquire()` dumps the ST's RAM into a
   `MemImage` via `savebin` (ADR-9).
3. `ripper::Registry::scan(mem, Some(Picture), depth=2)` runs every `Picture`
   detector in parallel (`degas-*`, `neochrome-*`); `pack-ice` depacks any packed
   regions and the detectors re-run on the result.
4. Findings return as a **compact list** (plugin id, offset, label) — *not* pixels.
5. Agent calls `extract(finding, as="png")`; `ripper::image` decodes the planar
   region → PNG → returned as an **MCP image content block** the agent sees inline.

Example — *"what's playing?"*: `identify_music()` → façade over
`scan(Some(Music))` → `sndh` parses any `SNDH` header (title/composer),
`ym-player` locates the `$FF8800` write loop and fingerprints the driver →
`{ sndh_header?, replay_addr, player, confidence }`.

Example — *"is this disk's boot sector a virus?"*: `bootsector(source="disk",
path=…)` → `ripper::disk::read_boot_sector` (path-validated) → `BootSector`
analysis (checksum + BPB + disasm tells) → `{ executable, bpb,
classification, tells, confidence }`; the agent `extract`s the 512 bytes on
demand.

Example — *"patch this routine in place"*: `patch(addr=$78000, source=…)` →
`assemble::assemble` (vasm, org forced to addr) → `debug::write_bytes`
chunked `memwrite` → `memory::acquire_range` readback → byte-compare +
Capstone disasm → `{ verified, readback }`; the agent steps/observes and
iterates — no PRG rebuild.

Example — *"keep this hack"*: `pause` → patch → `save_prg(out, source_prg)`
→ basepage (`$4F2`→`p_run`) → ranged `savebin` of TEXT+DATA → diff vs file
image + relocation → patched copy on disk; reloc/BSS patches reported,
never persisted.

Example — *"what state is the machine in?"*: `machine_state()` → momentary
freeze (ADR-11) → `cpureg` + 3 video-register reads + ranged `savebin` →
thaw → pretty text + JSON + inline PNG; raw dump stays on disk as a path
handle (ADR-5).

Example — *"did my patch change the music?"*: `spectrogram(seconds=5)` →
fast-forward off → `recsound` … `recsound` (5 real seconds) → fast-forward
on → hound WAV decode → STFT (2048/512, Hann) → dB → magma PNG inline +
JSON axis facts; WAV stays on disk as a path handle → `patch` the replay
routine → `spectrogram(seconds=5)` again → compare the two images.

Example — *"where is the timer variable?"*: `cheat_scan(action="new")` →
snapshot RAM, seed every candidate address → run the machine (`run_frames`
or `resume`/`pause`) → `cheat_scan(action="filter", predicate="changed")`,
repeated, narrowing → `cheat_scan(action="list")` → `write_memory`/`watch`
the survivor; the server holds the 4 MiB snapshots and the candidate
bitset, the agent sees only counts and a ≤64-row sample per step (ADR-5).

Example — *"what is the chip playing?"*: `ym_state()` → `debug::ym_registers`
(`info ym`, one sentinel-synced round-trip, session untouched) →
`analyze::ym::decode` → pretty text + JSON.

Example — *"rip the digi-music sample"*: `rip_sample()` → momentary freeze →
`m $ff8900 34` → `analyze::dmasnd::decode` → ranged `savebin
[frame_start, frame_end)` → thaw → `audio::wav::write_pcm8` →
`{ wav_path, rate_hz, duration_s, … }`; the agent `spectrogram(wav_path=…)`s
it.

## Key Domain Concepts

- **Session** — one running Hatari process + its fifo/log. Single-session for v1.
- **MemImage** — an acquired snapshot of ST RAM (≤ 4 MB) the ripper operates on;
  decoupled from Hatari so plugins are pure and testable.
- **RipPlugin / Depacker** — the extension contract (above).
- **Finding** — a located artifact (offset + len + kind + label); lightweight
  identifier the agent later `extract`s just-in-time.
- **Symbols** — DRI/GST symbols from the PRG (`symbols prg`); TEXT symbols resolve
  by name, BSS pointer *values* change per run (dereference live via `evaluate`).
- **Replay anchor** — disasm-confirmed `$FFFF8800`/`$FFFF8802` reference cluster;
  the relocation-invariant location of a music driver (P6, ADR-16).
- **Player signature** — const masked-word fingerprint naming a replay driver;
  unknown always degrades to `("unknown", 0.25)`, never errors (P6, ADR-16).
  DB entries are corpus-derived with cited provenance (P19, ADR-35); adding
  a driver = one const entry + one excerpt test in `ym_sig_db.rs`. P24
  (ADR-41) scaled the DB against the SNDH archive (5,897 tunes,
  refs/sndh_lf, gitignored): 20 entries naming 30.5% of the archive;
  24-bit `$FF8800.l` accesses are now anchor-confirmed; the `#[ignore]`d
  `ym_sig_corpus` test re-validates provenance, the best-match histogram,
  and per-driver-dir exclusivity.
- **Boot sector** — 512-byte logical sector 0; executable iff the big-endian
  word-sum equals `$1234`; classified plain-data / loader / suspicious by
  disasm-based heuristics — tells, never a verdict (P7, ADR-17).
- **Repatch plan** — the diff between the running program's TEXT+DATA RAM and
  its original `.PRG` image, split into file-representable patch runs and
  non-representable relocated-pointer skips; the unit `save_prg` persists
  (P16, ADR-30).
- **Sample buffer** — the `[frame_start, frame_end)` RAM window the STE DMA
  sound engine streams as signed 8-bit PCM; located by registers, not by
  scanning (P20).
- **Snapshot PRG** — a frozen machine (full RAM + CPU + reinstallable
  hardware registers) persisted as a self-restoring `.PRG`, `save_prg`
  mode="snapshot" (P22, ADR-38/-39). The restore-stub hole ($1C0..$400,
  68000 exception vectors 112-255 + the crash page) and the 6 bytes below
  the frozen SSP are the honest, bounded residue every output reports; a
  fixed `not_restored` list (video beam position, MFP timer phase, IKBD
  mode, blitter, …) is never hidden.

## State Machines

**Session lifecycle:**

```
Stopped ──start──▶ Booting ──(fifo ready + boot wait)──▶ Running
   ▲                                                      │  │
   │                                          pause │     │  │ debug/rip/screenshot
   └──────────────── stop ───────────────────       ▼     │  │ (stay Running)
                                                  Paused ──┘  ◀┘
                                                     │ resume
                                                     ▼
                                                  Running
```

- `start` fails if already `Running`/`Paused` (single-session).
- Ripping/memory acquisition is valid in `Running` or `Paused`; `Paused` gives a
  stable snapshot (the freezer-cartridge "freeze then rip" workflow).
- `stop` from any state → `Stopped` (kills the process, frees symbols; if the
  session was frozen, thaws it first — a frozen Hatari never reads the fifo quit).
- `pause` = a debugger freeze (ADR-11), **not** `hatari-stop`/`hatari-cont`:
  `hatari-stop` is banned — it deadlocks Hatari 2.4.1 permanently by halting the
  main loop without entering the debugger, so the fifo can never be serviced
  again. `pause` arms a one-shot breakpoint and confirms the freeze over Hatari's
  stdin (held open for the session's life); `resume` writes `c` to stdin and
  confirms over the fifo. See the `session/` layer section above for the two
  control planes this implies.
- **Managed run-to-break (ADR-15):** from `Paused`, `Session::run_until_break`
  deliberately arms + runs to a breakpoint and adopts the result:
  `Paused ──run_until_break──▶ Paused` (fired, frozen at the breakpoint) **or**
  `Paused ──run_until_break──▶ Running` (timeout, breakpoint disarmed) — every
  outcome is a compile-time-known session state, closing the "breakpoint fires
  behind the FSM's back" hole ADR-11 left open. `tools/breakpoint.rs`
  (action=set) and `tools/analyze_profile.rs` (`profile`) are its only callers.
