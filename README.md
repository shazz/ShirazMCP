# ShirazMCP

[![CI](https://github.com/shazz/ShirazMCP/actions/workflows/ci.yml/badge.svg)](https://github.com/shazz/ShirazMCP/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Rust](https://img.shields.io/badge/rust-1.92-orange.svg)

A local [MCP](https://modelcontextprotocol.io/) server that drives the
[Hatari](https://hatari.tuxfamily.org/) Atari ST/STE emulator so an AI agent can
**run, debug, freeze, screenshot, rip, and hot-patch** Atari ST programs —
non-interactively, over stdio.

It's a 2026, headless-agent reincarnation of the classic ST freezer/ripper
cartridges — **Ultimate Ripper** (rip pictures + music, save as data or a runnable
`.PRG`) and **Multiface ST** (freeze, memory view/edit, disassembler). `pause`/`resume`
are that freeze button; `rip`/`extract` are that ripper menu; `save_prg mode=snapshot`
is the "save the frozen machine" feature the cartridges were famous for.

> **The name.** Shiraz — after **[Shiraz Shivji](https://en.wikipedia.org/wiki/Shiraz_Shivji)**,
> the lead hardware architect of the Atari ST. A tool that boots, freezes and reverse-engineers
> the ST, named for the engineer who designed it.

## Highlights

- **Freeze & inspect** — `pause` parks the CPU via a debugger break; read registers, memory,
  the video/sound state, and screenshots all from the same frozen instant.
- **Rip** — graphics (Degas/NeoChrome/MPP), music (SNDH/YM, with a player-fingerprint DB),
  packed data (Pack-Ice/Atomik/PowerPacker/RNC/**ZX0**, recursively), boot sectors, and
  **STE DMA-PCM samples** → WAV. Picture findings are **structure-scored** (ranked, capped —
  no false-positive floods).
- **Reverse-engineer live** — value-change **watchpoints**, static **xrefs**, deterministic
  **frame stepping**, and an Action-Replay **cheat-scan** memory-diff search.
- **Hot-patch 68k live** — `assemble`/`patch` (via vasm), then **persist** the hack: `save_prg`
  re-emits a patched `.PRG`, or **snapshots the whole machine** into a self-restoring `.PRG`.
- **See & hear** the hardware — cycle-accurate profiling with an STE bus-contention model,
  `spectrogram` ("see the music"), and live YM2149 decode (`ym_state`).

## Prerequisites

**To *run* ShirazMCP** (drive the emulator):

| Need | For | Notes |
|---|---|---|
| the `shiraz-mcp` binary | everything | build it (below) — no prebuilt releases yet |
| **hatari** | the emulator | default `/usr/bin/hatari`; `apt install hatari` or build from source |
| a **TOS ROM** image | booting the ST | user-supplied; path is configurable |
| an **X display** (`DISPLAY`) | Hatari rendering | no headless (`Xvfb`) path yet |
| **vasm** (`vasmm68k_mot`) | *optional* — `assemble`/`patch` | path via `SHIRAZMCP_VASM`; the snapshot stub ships pre-assembled |

**To *build* from source:** **Rust 1.92** (2021 edition). Rust is build-time only — running
the compiled binary needs nothing from the Rust toolchain.

## Build

```sh
cargo build --release        # -> target/release/shiraz-mcp
```

## Add the MCP server

ShirazMCP speaks MCP over **stdio**, launched by an MCP client. Register it with Claude Code:

```sh
claude mcp add shirazmcp --env DISPLAY=:0 -- /abs/path/to/target/release/shiraz-mcp
```

Then `/mcp` (or restart the session) to load the tools — they appear as `mcp__shirazmcp__*`.
See **[DEVELOPMENT.md](DEVELOPMENT.md)** for the alternative "harness" mode (drive the binary
directly over JSON-RPC — handy while iterating on the server itself).

## Install the skill

The repo ships a usage skill that teaches an agent how to drive the tools:

- **Project-local:** it lives at `.claude/skills/shirazmcp/` and is picked up automatically when
  you work in this repo.
- **Global:** copy that folder to `~/.claude/skills/shirazmcp/` to use it from any project.

## Tools (35)

| Group | Tools |
|---|---|
| **Lifecycle** | `start` · `stop` · `pause` · `resume` · `status` |
| **Debug & memory** | `registers` · `read_memory` · `write_memory` · `search_memory` · `screenshot` · `machine_state` · `debug` |
| **Analyze** | `disasm` · `profile` · `breakpoint` · `state` |
| **Reverse-engineering** | `watch` · `xrefs` · `run_frames` · `cheat_scan` |
| **Ripper** | `rip` · `list_ripper_plugins` · `extract` · `find_picture` · `identify_music` · `rip_sample` · `bootsector` · `view_memory_as_screen` · `depack` |
| **Assemble / patch / persist** | `assemble` · `patch` · `save_prg` (`mode=repatch\|snapshot`) |
| **Audio** | `record_audio` · `spectrogram` · `ym_state` |

Full behavior, workflows, and gotchas are in the **[shirazmcp skill](.claude/skills/shirazmcp/SKILL.md)**.

## A worked example

`docs/live-patching.md` walks through hot-patching a running demo — changing its scroller text,
gradient colours, and swapping its logo rows **live in RAM**, then persisting the result as a
standalone `.PRG` — entirely through MCP tool calls.

## Platform support

Builds and runs on **Linux** and **macOS**. **Windows** is not yet supported: the emulator is
controlled through a Unix FIFO, and stock Hatari has no external command channel on Windows.
A cross-platform transport adapter (targeting the `tattlemuss/hatari` hrdb fork's TCP debug
protocol) is designed and tracked — see the issues.

## Documentation

- **[DEVELOPMENT.md](DEVELOPMENT.md)** — build, run, and drive the server in dev
- **[docs/live-patching.md](docs/live-patching.md)** — the live RE/hot-patch tutorial
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — layers, structure, patterns
- **[decisions.md](decisions.md)** — architecture decision records
- **`docs/plan/*.md`** — per-feature implementation specs

## License

[MIT](LICENSE) © shazz
