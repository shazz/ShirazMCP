# ShirazMCP — Brainstorm

> Capture of the design conversation. This is the messy-but-organized "why",
> feeding into `ARCHITECTURE.md` (the "how") and `decisions.md` (the ADRs).

## The pitch

A **local MCP server that drives [Hatari](https://hatari.tuxfamily.org/)** (Atari
ST/STE emulator) so an agent can run, debug, freeze, screenshot, and *rip* Atari
ST programs non-interactively from the terminal.

The manual workflow already exists and works — see `../MJJ/DEBUG.md`. It's a pile
of brittle bash: launch-and-wait-for-fifo, `echo cmd > fifo` then `sleep 1` after
*every* command, `kill` the PID, `cat` the log, hand-parse. The MCP server
encapsulates that ritual once so we never touch it again.

## Heritage — the freezer/ripper cartridges 🕹️

This is deliberately a modern reincarnation of the legendary ST hacking
cartridges. The tool surface is modelled on what they actually did:

- **Ultimate Ripper** (famous ripper cartridge, celebrated in *ST NEWS*):
  reset → tap F1/F2 → the telltale black-and-white flashing scan lines → a menu
  that ripped **pictures with correct palette across all 3 resolutions (4 planes)**,
  a **music ripper** that hunted chip tunes + SoundTracker modules, let you
  *listen*, and saved them as data **or as a runnable `.PRG`**.
- **Multiface ST** (Romantic Robot): freeze button, **view/edit memory ("pokes")**,
  disassembler, save program state.

We are building the 2026 headless-agent version of that menu.

## Feature wishlist → tools

| Cartridge feature | Tool | Notes |
|---|---|---|
| Freeze / reset-into-monitor | `pause` / `resume` | maps to Hatari `hatari-stop` / `hatari-cont` |
| Memory view & edit ("pokes") | `read_memory`, `write_memory` | the cheat/poke button |
| Memory search | `search_memory(pattern)` | bytes / ASCII / value; returns offsets + context |
| Disassembler | `disasm(addr)` | symbol- and offset-aware |
| Registers | `registers` | `cpureg` |
| Breakpoints | `breakpoint(action=set\|list\|remove\|clear, …)` | **one** grouped tool, not five |
| Save state | `state(action=save\|load)` | grouped like breakpoint |
| Image Ripper | `find_picture()`, `view_memory_as_screen(addr,res,palette)` | returns PNG as MCP image |
| Music Ripper / recogniser | `identify_music()` | SNDH + YM-anchor + fingerprint (below) |
| Sprite/font Ripper | `find_sprites()` | later |
| Symbols | `load_symbols` | `symbols prg` — DRI/GST from the PRG |
| Screenshot | `screenshot()` | returns the framebuffer PNG as an image |

### Grouping decision

`breakpoint` and `state` are **single tools with an `action` param**, not one
tool per verb. Fewer tool descriptions in the agent's context, naturally grouped.
This follows Anthropic's tool-writing guidance (below).

## SNDH / music-player identification 🎼

"Identify SNDH" is **not** "scan for audio data" — it's **fingerprinting the
replay routine** (the ripper's "oh, this is a Mad Max player" recognition).
SNDH = the original songfile + replaycode with a header bolted on top, exposing a
unified init/play/exit interface over whatever driver is underneath.

Three layers, easiest → cleverest:

1. **SNDH header scan** — the literal `SNDH` magic sits in RAM with its tag block
   (`TITL`, `COMM` composer, `RIPP`, subtune count, timer/replay-freq). Just
   `search_memory("SNDH")` → parse header → title/composer for free.
2. **Universal anchor: the YM write loop** — *every* YM2149 player pokes the chip
   through **`$FFFF8800`** (register select) / **`$FFFF8802`** (data). Scanning for
   code writing those constants locates the *active* replay regardless of player,
   and is relocation-invariant (survives Pack-Ice `ICE!` depack + relocation).
3. **Signature DB → name it** — match relocation-tolerant opcode sequences around
   the loop against a fingerprint DB (Hippel/SidMon, Mad Max, TCB Tracker,
   maxYMiser, Lucas/Quartet…). We seed from prior art: [`frno7/sndhfix`] and the
   labelled [SNDH archive] corpus. Honest hard part: absolute addresses differ per
   relocation, so signatures key on opcodes + immediates + register-table shape,
   ignoring address operands.

`identify_music() → { sndh_header?, replay_addr, player, confidence }`

[`frno7/sndhfix`]: https://github.com/frno7/sndhfix
[SNDH archive]: https://sndh.atari.org/fileformat.php

## Ripping = a plugin system 🔌 (core requirement)

The ripper must **not** be a fixed set of hardcoded scanners. Adding a new
format = **one new file + one registry line**. Two small traits carry it:

- **`RipPlugin`** — a detector. Given a memory image, returns findings with
  offsets. E.g. `degas-pi1`, `neochrome-neo`, `sndh`, `ym-player`.
- **`Depacker`** — a transformer. If a region matches its packer scheme, returns
  the *unpacked* bytes. E.g. `pack-ice`, `atomik`, `automation`.

The engine runs detectors over the raw RAM image, runs depackers, and for every
successful depack **re-runs the detectors over the unpacked buffer** (bounded
recursion depth). That gives *"find packer → depack → find the PI1 inside"* for
free — the exact ripper-cartridge magic.

Seed plugins:

| id | kind | notes |
|---|---|---|
| `degas-pi1/pi2/pi3` | Picture | uncompressed: 2-byte res + 16-word palette + 32000B |
| `degas-pc1/pc2/pc3` | Picture | RLE-compressed Degas Elite |
| `neochrome-neo` | Picture | 32128B, header + palette |
| `sndh` | Music | `SNDH` header + tags |
| `ym-player` | Music | `$FF8800` anchor + fingerprint DB |
| `pack-ice` | Depacker | `ICE!` magic → unpack → rescan |
| `bootsector` | Boot | 512B sector, `$1234` checksum, BPB decode, virus/loader heuristics |

**Bootsector plugin** (for a separate security/analysis project): extracts and
analyzes ST floppy boot sectors from disk images (`.ST/.MSA/.DIM`) or RAM — the
classic virus/antivirus/boot-loader surface. Executable iff word-sum == `$1234`;
decodes the (little-endian DOS) BPB; disassembles the boot code; flags virus tells
(vector hooks, self-copy, disk writes) and fingerprints known virus families. Same
plugin contract — see the `atari-st-bootsector` skill. Adds a `Boot` `RipKind`.

Plugins **self-describe** (`id`, `kind`, one-line desc) so a `list_ripper_plugins`
tool can enumerate them for discovery. Generic `rip(kind?, plugin?)` runs matching
plugins; `find_picture`/`identify_music` are thin façades filtering by kind.
`extract(finding, as=png|prg|raw)` materializes a hit (render PI1 → PNG, dump
depacked bytes, save music as runnable `.PRG`).

## Language: Rust 🦀 (decided)

Honest split of the argument:

- **Orchestration path (fifo/debug) will NOT be faster in Rust** — it's I/O-bound
  on the fifo round-trip inside Hatari. Language is irrelevant there.
- **Ripping/search path WILL be faster** — repeated full-RAM sweeps, rendering
  every 32000-byte offset as a candidate ST screen and scoring it, brute-forcing
  fingerprints across 4 MB: compute-bound, SIMD-friendly, embarrassingly parallel.
  `rayon` + `memchr` smoke an interpreted scan and make an interactive
  "scrub memory as pictures" mode feel live.
- Plus: single static binary (no venv), and **we want to experiment**.

Decision recorded in `decisions.md`. Chosen for experimentation + native ripping
perf — *not* for the orchestration path (stated honestly so nobody later assumes
Rust bought us faster debugging).

## Context-efficiency principles (Anthropic guidance)

From ["Writing effective tools for agents"], ["Effective context engineering"],
and ["Code execution with MCP"]:

- **Never pour 4 MB into the agent's context.** The server holds the megabytes and
  returns only compact hits — offsets + a few bytes of context — paginated with a
  sensible default limit. `read_memory` gets a default `count` cap.
- **Screenshots return as image content blocks**, not file paths — the agent sees
  the framebuffer inline (kills the old `screenshot → write PNG → Read` dance).
- **Tool descriptions are prompt.** Write them like onboarding a new hire; make
  implicit ST context explicit (hex `$`, planar layout, per-run load addresses).
- **Future flex:** expose the ripper as a scriptable code API so the agent writes
  one "scan every offset, score, return top 5" call instead of many round-trips.

["Writing effective tools for agents"]: https://www.anthropic.com/engineering/writing-tools-for-agents
["Effective context engineering"]: https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents
["Code execution with MCP"]: https://www.anthropic.com/engineering/code-execution-with-mcp

## Key reliability win: kill the blind `sleep`s

DEBUG.md sleeps after every fifo write because the fifo is async and output only
returns via the log. The server replaces that with a **sentinel**: send the
command, then send a marker (e.g. `evaluate $DEADBEEF`), tail the log until the
marker's output appears = command done. Deterministic instead of "sleep 1 and
hope."

## Environment facts (verified 2026-07-08)

- `rustc`/`cargo` **1.92**; `rmcp = "2.2.0"` on crates.io.
- `hatari` installed at `/usr/bin/hatari`.
- `DISPLAY=:0` is live — Hatari's SDL window works now.
- **`Xvfb` NOT installed** — headless/CI runs will need it (or `hatari --no-gui`
  isn't enough; Hatari needs an SDL surface). Deferred until we care about CI.
- Reference TOS + demo live in `../MJJ` (`bin/hatari/TOS/tos162fr.img`,
  `build/AUTO/STRGOOSE.PRG`) — handy for integration testing.

## Analysis features (added in session)

### Disassembler — Hatari for live, embedded Capstone for bulk
Hatari's debugger **does** provide a 68000 disassembler (`disasm <addr>`), and it's
fine for live single-address peeks over the fifo. But it's line-by-line through the
log (pagination, round-trips) — poor for bulk/offline work. So: **use Hatari's
`disasm` for interactive queries, and embed `capstone` (M68K) for fast, offline
disassembly over an acquired `MemImage`.** The embedded path is what the ripper
(player fingerprinting) and the profiler (walking instructions to sum cycles) need —
no emulator round-trips, works on a frozen snapshot. Decision → `decisions.md`.

### Register annotator (address → metadata)
The MCP enriches every result touching hardware with metadata: `$FF8240 →
"Shifter — palette register 0"`, `$FF8800 → "YM2149 register select"`. Seed data is
MJJ's `memory_map.txt` (bundled in the `atari-st-hardware-map` skill). Behavior:
exact hit → register + field; range hit → computed index (`palette register N`);
PSG-aware (track last `$FF8800` select to name the target reg). Woven into
`read_memory`/`disasm`/`search_memory` results as an optional `annotations` field —
not a separate tool. Keeps raw addresses meaningful without me re-deriving them.

### Profiler / cycle-counter (between breakpoints)
Estimate performance of a code range — does it fit a scanline/VBL budget?
- **Ground truth:** Hatari's CPU profiler (`profile on` → per-address/symbol cycle
  counts) between two breakpoints — measured, includes real ST bus/video contention.
- **Static estimate:** disassemble (embedded Capstone) + sum 68000 cycle tables —
  fast, offline, contention-blind (mark as estimate).
- `profile(from, to) → { measured_cycles?, static_cycles, instructions, vbl_fraction }`

Backing knowledge in the `m68000-cycles` skill (reuses MJJ's `68000_execution_cycles.md`
+ `cycle_counter_plan.md` contention model). 50 Hz VBL = 160256 cycles; scanline ≈ 512.

## Reference skills created (`.claude/skills/`)

On-demand Atari ST domain knowledge for the agents and the MCP:
`atari-st-bitplanes`, `atari-st-degas` (PI1/PC1), `atari-st-neochrome` (NEO),
`ym2149-psg`, `atari-st-ym-players` (SNDH + fingerprinting), `atari-st-blitter`
(bundles MJJ blitter manual), `atari-st-packers` (ICE!/ATOM + depacker list),
`m68000-cycles` (bundles MJJ timing docs), `atari-st-hardware-map` (bundles MJJ
`memory_map.txt` — the annotator's dataset).

## External reference repos (cloned into `refs/orionfuzion/`)

Orion (The Replicants & Fuzion) — Atari ST cracking/ripping sources, git-ignored,
documented in `refs/README.md`:

- **`r1pp3r`** — boot-cracktro ripper (sandbox + XBios hook + save-as-PRG). The
  blueprint for our bootsector→ripper→PRG flow.
- **`dec0de`** — deprotector (Copylock, Sly Packer, Anti-bitos, …); single-file C +
  runnable Linux binary + samples. Backs the new `atari-st-protections` skill.
- **`intros`** — Orion's cracktro/demo sources, built & tested on Hatari 2.0 + TOS
  1.62 (our setup) → integration-test corpus.
- **`newcracks`** — documented crack/pack sources (didactic).

### Deprotection dimension (new)
Protections (encryption/anti-debug) are distinct from packers (compression). A
`Deprotect` capability can either **wrap** `dec0de` (shell out for on-disk `.PRG`s)
or **port** specific unprotectors into plugins. Often moot in a live session — a
protected program self-decrypts in RAM, so scanning live memory sidesteps it (same
as packers). See `atari-st-protections`.

## Open questions

- **Sessions:** single Hatari instance (simple, matches DEBUG.md) vs named
  sessions. Start single.
- **Memory acquisition for ripping:** does Hatari expose a whole-RAM dump to file,
  or do we assemble it from ranged `m` dumps? Affects search latency a lot —
  investigate before building the ripper layer.
- **Palette source for `view_memory_as_screen`:** read STE palette regs
  (`$FFFF8240…`) live, or take a palette arg? Probably both.
- **Signature DB format + seeding pipeline** for `identify_music`.
