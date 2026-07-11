# ShirazMCP — Multiphase Implementation Plan

> Execution plan for **rust-engineer**, one phase at a time. Architecture in
> [`ARCHITECTURE.md`](ARCHITECTURE.md), rationale in
> [`docs/BRAINSTORM.md`](docs/BRAINSTORM.md), settled choices in
> [`decisions.md`](decisions.md). Do not re-litigate ADRs; if implementation
> reveals a genuine conflict, stop and flag it.

## Overview

Eight phases. P0–P1 build the emulator control plane (session, sentinel fifo,
debugger, memory, annotator). P2 adds offline analysis (Capstone disasm,
cycles/profiler). P3 builds the pure ripping engine + plugin contracts
(including the `Deprotect` seam, dummy-only). P4–P7 are plugin phases: each
adds formats as "one file + one registry line" and one or two façade tools.

**Reference demo for behavioral checks** (see `../MJJ/DEBUG.md`):

```bash
hatari --machine ste --memsize 1 \
    --tos $HOME/projects/MJJ/bin/hatari/TOS/tos162fr.img \
    --harddrive $HOME/projects/MJJ/build \
    --fast-boot on --fast-forward on --confirm-quit off \
    --cmd-fifo /tmp/hatari_fifo
# auto-runs $HOME/projects/MJJ/build/AUTO/STRGOOSE.PRG
```

## Phase dependency graph

```
P0 scaffold
 └─▶ P1 foundation (session/debug/memory/annotator)
      ├─▶ P2 analyze (Capstone + profiler)  ──┐
      └─▶ P3 ripper engine + Deprotect dummy  │
           ├─▶ P4 graphics plugins            │
           ├─▶ P5 packers/depackers (richer   │
           │      tests once P4 detectors     │
           │      exist — run after P4)       │
           ├─▶ P6 music/SNDH  ◀───────────────┘ (fingerprint needs P2 disasm)
           └─▶ P7 boot sector
```

Strict order: P0 → P1 → P2 → P3 → P4 → P5 → P6 → P7. (P2 before P3 keeps the
Hatari-facing work contiguous; P6 consumes P2's disassembler.)

## Conventions — every phase, non-negotiable

- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test` green before a phase is "done".
- Files ≤ ~200 lines, functions ≤ ~30 lines. Split before you exceed.
- `thiserror` in all `src/` layers; `anyhow` only in `main.rs`/tool entry.
- No `unwrap`/`expect`/`panic!` in library paths (tests/fixtures exempt).
- Newtypes (`Addr`, `Offset`, `PluginId`) at every boundary — no raw `u32`
  addresses in public signatures.
- Layering: `session` → `debug` → `analyze` → `ripper` → `tools`. Dependencies
  point downward only. `ripper` never imports `session` (it receives
  `&MemImage`). `tools` contains no business logic.
- Token efficiency: every tool that can return bulk data has a default limit +
  pagination; findings are compact handles; images return as MCP image content
  blocks; large artifacts go to files, tools return paths.
- Integration tests that need Hatari/X are `#[ignore]` by default and run via
  `cargo test -- --ignored` on a machine with `DISPLAY` (documented in README).
- New plugin phases must NOT edit the engine — only add a plugin file and a
  registry line. If a phase forces an engine edit, that is an architecture bug:
  flag it to rust-architect before proceeding.

---

## P0 — Scaffold

**Goal:** a compiling `rmcp` stdio server with the error taxonomy, newtypes,
and the type-state Session skeleton — no Hatari yet.

**Depends on:** nothing.

**Deliverables**

| Task | Files | Notes / references |
|---|---|---|
| Cargo setup | `Cargo.toml`, `.gitignore`, `rustfmt.toml` | Single crate. Pinned deps: `rmcp = "2.2"`, `tokio`, `serde`, `serde_json`, `schemars`, `thiserror`, `anyhow`. Add `image`, `memchr`, `rayon`, `capstone` now (workspace stability) but no code uses them yet. Lockfile committed. |
| Error taxonomy | `src/error.rs` | `Error` enum via `thiserror`: `Session` (bad state), `Fifo` (I/O), `Timeout` (sentinel never echoed), `Parse` (log output), `Ripper`, `Io(#[from])`. Matchable categories, contextual messages. |
| Newtypes | `src/types.rs` | `Addr(u32)`, `Offset(usize)`, `PluginId(&'static str)` — private inner fields, `Display` as `$XXXXXX` for `Addr`, no `Deref` leaks. `MemImage` stub (owned bytes + base `Addr`). |
| Type-state Session skeleton | `src/session/mod.rs`, `src/session/state.rs` | `Session<Stopped>`, `Session<Running>`, `Session<Paused>` with phantom states; transitions consume `self` and return the next state. Methods stubbed (`todo` returns `Err(Error::Session)`, never `todo!()`). Runtime holder for the tool layer: `enum AnySession { Stopped(..), Running(..), Paused(..) }` behind `tokio::sync::Mutex` — the type-state enforces transitions, the enum gives tools runtime dispatch. |
| rmcp bootstrap | `src/main.rs`, `src/tools/mod.rs`, `src/tools/lifecycle.rs` (stub) | Stdio server starts, registers one `status` tool returning `{ state, hatari_path, version }`. `anyhow` here only. |
| Smoke tests | `tests/` + unit tests | FSM: `Stopped→(start type only in P1)`; compile-fail expectations documented as unit tests on `AnySession` transitions; error `Display` snapshots. |

**New MCP tools:** `status`.

**Acceptance criteria**

- fmt + clippy (`-D warnings`) + tests green.
- `cargo run` starts, responds to MCP `initialize` + `tools/list` over stdio
  (verify with a scripted JSON-RPC handshake in an integration test or via
  `printf` piping); `status` returns `state: "stopped"`.
- Invalid FSM transitions do not compile (e.g. no `pause()` on
  `Session<Stopped>` — assert by API shape, not runtime test).

---

## P1 — Foundation: session, sentinel fifo, debugger, memory, annotator

**Goal:** full emulator control plane — start/stop/pause/resume, `debug(cmd)`,
`registers`, `screenshot` (MCP image), `read/write/search_memory`, register
annotations. This is the user-decided Phase 1 scope.

**Depends on:** P0.

**Deliverables**

| Task | Files | Notes / references |
|---|---|---|
| Process lifecycle | `src/session/mod.rs`, `src/session/spawn.rs` | Spawn Hatari (`tokio::process`) with the DEBUG.md flag set (machine/tos/harddrive/fast-boot/fast-forward/confirm-quit/cmd-fifo); config struct with sane defaults + overrides. Wait-for-fifo loop (Hatari creates it — never `mkfifo`). Kill + reap on `stop` (RAII: `Drop` kills as last resort). Booting→Running gate: fifo exists + a sentinel round-trip succeeds (no blind boot sleep beyond a bounded poll). |
| Sentinel fifo | `src/session/fifo.rs` | The load-bearing primitive: `debug(cmd)` writes `hatari-debug <cmd>` then `hatari-debug evaluate $DEADBEEF`, tails the log (`tokio` file tail) until the marker echoes, returns intervening output. Bounded timeout → `Error::Timeout`. Also `hatari-stop`/`hatari-cont`/`hatari-shortcut` wrappers. Consult `../MJJ/DEBUG.md` for wrapper syntax and log format. |
| Pause/resume in FSM | `src/session/state.rs` | `Session<Running>::pause() -> Session<Paused>`, `resume()` back; `stop()` from Running/Paused → `Session<Stopped>`. |
| Memory acquisition (spike + impl) | `src/session/memory.rs` | **Spike first:** does Hatari expose a whole-RAM dump to file, or do we assemble ranged `m` dumps? (open question in BRAINSTORM §Open questions). Timebox the spike; implement whichever works into `acquire() -> MemImage`; record the answer as a short ADR in `decisions.md`. |
| Debugger wrappers + parsing | `src/debug/mod.rs`, `src/debug/parse.rs` | Typed `evaluate`, `cpureg` → `Registers` struct, `m` memdump → bytes, `symbols prg`. Encapsulate quirks: `$` hex, `m` refuses indirection (do `evaluate (ptr)` then literal `m`). Parse from real captured log fixtures (commit fixtures under `tests/fixtures/`). Skill: `atari-st-hardware-map` for context. |
| Register annotator | `src/analyze/annotate.rs`, `src/analyze/mod.rs` | Dataset from the `atari-st-hardware-map` skill (`memory_map.txt`, bundled in its `reference/`). Exact hit → name; range hit → computed index (`palette register N`); PSG-aware (track last `$FF8800` select). Pure function over `Addr` (+ optional select state) — unit-testable, no emulator. |
| MCP tools | `src/tools/lifecycle.rs`, `src/tools/debugger.rs`, `src/tools/mem.rs` | `start/stop/pause/resume`; `debug(cmd)`; `registers`; `screenshot` (via `hatari-shortcut screenshot` or `screenshot <path>` debug cmd → read PNG → return as **MCP image content block**); `read_memory(addr, count≤default 256, paginated)`, `write_memory`, `search_memory(pattern: bytes|ascii, limit+offset pagination, hits = addr + few context bytes)`. All memory results carry optional `annotations`. |
| Integration test | `tests/integration.rs` | `#[ignore]`d: boots the MJJ demo, waits Running, reads `$FF8240`, screenshots, stops. |

**New MCP tools:** `start`, `stop`, `pause`, `resume`, `debug`, `registers`,
`screenshot`, `read_memory`, `write_memory`, `search_memory`.

**Acceptance criteria**

- fmt + clippy + unit tests green; parse tests pass on committed log fixtures.
- Behavioral (against the MJJ demo): `start` → Running with **zero blind
  sleeps** (sentinel only); `registers` returns a typed PC; `screenshot`
  returns a valid PNG image block showing the demo;
  `search_memory` finds a known byte sequence from STRGOOSE.PRG;
  `read_memory($FF8240, …)` result carries a `"Shifter — palette register 0"`
  annotation; `stop` leaves no orphan `hatari` process.
- No tool returns more than its default cap without explicit pagination params.

---

## P2 — Analyze: Capstone disassembler + profiler/cycles

**Goal:** fast offline M68K disassembly over a `MemImage`, static cycle
estimates, Hatari-profiler ground truth, grouped `breakpoint`/`state` tools.

**Depends on:** P1 (`MemImage` acquisition, debug layer for profiler/breakpoints).

**Deliverables**

| Task | Files | Notes / references |
|---|---|---|
| Embedded disasm | `src/analyze/disasm.rs` | `capstone` crate, M68K arch. `disasm(mem: &MemImage, at: Addr, count) -> Vec<Insn>` — pure, offline. Typed `Insn { addr, mnemonic, ops, bytes }`. Hatari's live `disasm` stays available via `debug(cmd)` for single-address queries. |
| Cycle tables + static estimate | `src/analyze/cycles.rs` (+ `cycles_tables.rs` if >200 lines) | 68000 timing per the `m68000-cycles` skill (bundled MJJ `68000_execution_cycles.md`): base + EA cost, even rounding; flag as *estimate* (contention-blind). Constants: VBL = 160256 cycles @50 Hz, scanline ≈ 512. |
| Hatari profiler read | `src/debug/profile.rs` | Drive `profile on` / breakpoint-to-breakpoint run / parse per-address counts from the log (fixtures again). |
| Grouped debugger tools | `src/tools/debugger.rs` (extend), `src/tools/analyze.rs` | `breakpoint(action=set\|list\|remove\|clear, …)` and `state(action=save\|load)` — **one tool each with an `action` param**. `disasm(addr, count≤default 32, source=snapshot\|live)` with annotations on hardware operands. `profile(from, to) -> { measured_cycles?, static_cycles, instructions, vbl_fraction }`. |

**New MCP tools:** `disasm`, `profile`, `breakpoint`, `state`.

**Acceptance criteria**

- fmt + clippy + tests green; disasm unit tests on hand-assembled 68k fixture
  bytes (`move.w #$777,$ffff8240` etc.); cycle estimates match the skill's
  tables on a curated instruction list.
- Behavioral: disassemble the MJJ demo's entry (via `symbols prg` TEXT symbol)
  from a snapshot and see plausible 68k; `profile` across one demo routine
  returns both a static estimate and a measured count, with
  `vbl_fraction` computed; `breakpoint(action=set)` + resume actually halts.

---

## P3 — Ripper engine + plugin contracts (+ `Deprotect` dummy)

**Goal:** the pure, emulator-independent plugin engine: traits, registry,
recursive depack→rescan, `Finding` handles, extraction to disk. Ships with
zero real format plugins — proven by test-only fixture plugins.

**Depends on:** P1 (`MemImage`); no dependency on P2.

**Deliverables**

| Task | Files | Notes / references |
|---|---|---|
| Traits + Finding | `src/ripper/mod.rs`, `src/ripper/finding.rs` | `RipPlugin` / `Depacker` exactly as in ARCHITECTURE.md (object-safe, `Send + Sync`); `RipKind { Picture, Music, Sprite, Packer, Boot, Raw }`; `Finding { plugin, kind, offset: Offset, len, label }` + a stable `FindingId` handle so `extract` can reference a prior scan (server keeps the last scan's findings in session state). |
| Registry + engine | `src/ripper/registry.rs` | `Registry::scan(mem, kind?, depth)` — detectors in parallel via `rayon` (sync, never async); depackers tried, successful unpack → recurse with `depth-1`; findings from depacked buffers record their provenance chain (`via: ["pack-ice"]`). Bounded depth (default 2). One `fn default_registry()` = THE single registration point. |
| `Deprotect` seam (dummy only) | `src/ripper/deprotect.rs` | Per decision: **interface only, no real unprotectors.** `trait Deprotect: Send + Sync { fn id(&self); fn detect(&self, data: &[u8]) -> Option<ProtectionInfo>; fn strip(&self, data: &[u8]) -> Result<Vec<u8>, Error>; }` + `NoopDeprotect` that detects nothing. Registry holds `Vec<Box<dyn Deprotect>>` and reports detections as `Finding`s (kind label), but the engine never auto-strips. Background: `atari-st-protections` skill; `refs/orionfuzion/dec0de/src/dec0de.c` for what the seam must eventually accommodate — do not port it. |
| Extraction | `src/ripper/extract.rs` | `extract(finding, as=raw\|prg) -> path`: materialize bytes to a file under an output dir, return the **path + size**, never the bytes. (`as=png` arrives in P4.) `.PRG` wrapper: minimal GEMDOS header around ripped data (Ultimate Ripper's save-as-PRG trick; layout ref: `refs/orionfuzion/r1pp3r/src/R1PP3R.S`). |
| MCP tools | `src/tools/rip.rs` | `rip(kind?, plugin?, depth?)` → compact findings list (id, plugin, kind, offset, len, label, via) with default limit + pagination; `list_ripper_plugins` (id, kind, one-line desc — progressive disclosure); `extract(finding_id, as)`. |
| Fixture plugins + tests | `tests/ripper_engine.rs` | Test-only `RipPlugin`/`Depacker` (magic-byte toy format + toy "packer") proving: parallel scan, kind filter, recursion depth bound, provenance, "add plugin = one file + one line". No Hatari needed. |

**New MCP tools:** `rip`, `list_ripper_plugins`, `extract`.

**Acceptance criteria**

- fmt + clippy + tests green; engine fully unit-tested on fixtures with **no
  emulator**; `ripper/` compiles with zero `session`/`debug` imports (assert
  with a test or CI grep).
- Behavioral: `rip()` on a live MJJ-demo `MemImage` runs (returns empty or
  Raw findings — no format plugins yet) within a reasonable time on 1 MB;
  `list_ripper_plugins` lists the fixture-free default registry (just
  `noop-deprotect` visibility rules decided and documented).
- `NoopDeprotect` registered; `strip` never called by the engine (test).

---

## P4 — Graphics plugins: Degas, NeoChrome, raw bitplane→PNG

**Goal:** picture ripping — the Ultimate Ripper headline feature.

**Depends on:** P3 (engine), P1 (memory acquisition).

**Deliverables**

| Task | Files | Notes / references |
|---|---|---|
| Shared planar decode | `src/ripper/image.rs` | Interleaved bitplanes → chunky → RGBA via `image` crate; all 3 ST resolutions; ST 3-bit + STE 4-bit palette words. Skill: `atari-st-bitplanes` (decode ref: `Atari_ST_Sources/ASM/Lineout/Stone Tower/src/STLOWC2P.S` reversed; `C/Warwick Grigg/C_series.5/DEGAS.C`). |
| Degas plugin | `src/ripper/plugins/degas.rs` | PI1/PI2/PI3 (res word + 16-word palette + 32000 B) and PC1/PC2/PC3 (PackBits-style RLE, plane-by-plane de-interleave). Skill: `atari-st-degas`; port decode from `DEGAS.C`. One registry line. |
| NeoChrome plugin | `src/ripper/plugins/neochrome.rs` | 32128 B: 128-byte header (flag, res, palette, anim fields) + 32000 B screen. Skill: `atari-st-neochrome`; ref `Atari_ST_Sources/C/Jean Lusetti/Vision 4.1a/TOOLS/NEO_IO.C`. One registry line. |
| PNG extraction | `src/ripper/extract.rs` (extend) | `extract(finding, as=png)` → decode planar → PNG file → **also** return as MCP image content block (small; the agent sees the picture inline). |
| View-as-screen tool | `src/tools/rip.rs` (extend) | `view_memory_as_screen(addr, res, palette=live\|explicit)` — render arbitrary RAM as a screen (palette read live from `$FF8240…` when requested). `find_picture()` façade = `rip(kind=picture)`. |
| Fixtures | `tests/fixtures/` + `tests/graphics.rs` | Real small PI1/PC1/NEO fixture files (generate or rip from the archive); golden-image decode tests. |

**New MCP tools:** `find_picture`, `view_memory_as_screen` (+ `extract as=png`).

**Acceptance criteria**

- fmt + clippy + tests green; golden decodes for PI1, PC1, NEO fixtures across
  resolutions.
- Behavioral: boot the MJJ demo, `pause` (freeze-then-rip),
  `view_memory_as_screen` at the live screen base (from `$FF8201/03` — see
  `atari-st-hardware-map`) with live palette returns a PNG matching
  `screenshot` visually; `rip(kind=picture)` on a memory image containing a
  planted PI1 fixture finds and extracts it.

---

## P5 — Packers/depackers: Pack-Ice + recursion proof

**Goal:** detect ICE!/ATOM-packed regions, depack, and rescan — the
"find packer → depack → find the PI1 inside" cartridge magic, end to end.

**Depends on:** P3 (engine); run after P4 so recursion has real detectors.

**Deliverables**

| Task | Files | Notes / references |
|---|---|---|
| Pack-Ice depacker | `src/ripper/plugins/pack_ice.rs` | `Depacker` for Pack-Ice: `ICE!`/`Ice!` (and ATOM-variant) magic + header (packed/unpacked lengths) → sanity-check → depack. Port from `Atari_ST_Sources/C/Toyoyo/stvn/deps/pack-ice/ice_decrunch.c` (handles both header variants). Skill: `atari-st-packers`. One file + one registry line. |
| Packed-region detector | same file or `pack_detect.rs` if >200 lines | `RipPlugin` (kind `Packer`) reporting packed regions as `Finding`s (magic offset, claimed sizes) even when depack is deferred — cheap discovery without paying depack cost on every scan. |
| Recursion integration test | `tests/depack_rescan.rs` | Fixture: PI1 packed with Pack-Ice (pack via the reference `ice_crunch.c` once, commit the packed fixture). Assert `rip(kind=picture, depth=2)` finds the picture **inside** the packed blob with `via: ["pack-ice"]`. |

**New MCP tools:** none — the existing `rip`/`extract` gain the capability
silently (token-efficient: no new surface).

**Acceptance criteria**

- fmt + clippy + tests green; depack unit tests on both header variants;
  corrupted-header input returns `None` (never panics).
- Behavioral: the packed-PI1 fixture planted into live demo RAM
  (`write_memory`) is found and extracted as a correct PNG via depth-2 scan.
- Engine source untouched by this phase (registry line + plugin file only).

---

## P6 — Music: SNDH parse + YM player fingerprint

**Goal:** `identify_music()` — SNDH header data plus "this is a Mad Max
player" replay-routine recognition.

**Depends on:** P3 (engine), P2 (Capstone disasm for fingerprinting).

**Deliverables**

| Task | Files | Notes / references |
|---|---|---|
| SNDH plugin | `src/ripper/plugins/sndh.rs` | `search` for `SNDH` magic; parse tag block (`TITL`, `COMM`, `RIPP`, subtunes, timer/freq); init/exit/play entry offsets. Skill: `atari-st-ym-players`; ref `Atari_ST_Sources/C/Toyoyo/stvn/src/sndh.c`. |
| YM write-loop anchor | `src/ripper/plugins/ym_player.rs` | Relocation-invariant anchor: scan (via P2 disasm over the `MemImage`) for code writing `$FFFF8800`/`$FFFF8802` — locates the *active* replay regardless of driver. Skills: `ym2149-psg`, `atari-st-ym-players` (ref `Atari_ST_Sources/ASM/Mcoder/unreleased/MADMAX/DISK/MEGARIP.S`). |
| Signature DB | `src/ripper/plugins/ym_signatures.rs` (data) | Relocation-tolerant fingerprints (opcodes + immediates + register-table shape, address operands masked) for the seed set: Hippel/Mad Max, TCB Tracker, maxYMiser, Lucas/Quartet. Seed methodology per BRAINSTORM (`frno7/sndhfix`, labelled SNDH archive). Keep DB as data (const tables), not code. |
| Façade tool | `src/tools/rip.rs` (extend) | `identify_music() -> { sndh_header?, replay_addr, player, confidence }` = façade over `rip(kind=music)`. `extract(finding, as=prg)` saves a runnable player (P3's PRG wrapper). |
| Tests | `tests/music.rs` | Fixtures: 2–3 real SNDH files (header parse goldens); a known replay blob for one fingerprint; anchor-scan unit test on synthetic `move.b dN,$ffff8800.w` sequences. |

**New MCP tools:** `identify_music`.

**Acceptance criteria**

- fmt + clippy + tests green; SNDH tag parse goldens; anchor scan finds the
  planted YM loop; at least one signature matches its labelled fixture with
  stated confidence; unknown players degrade to
  `{ replay_addr, player: "unknown", confidence: low }` — never an error.
- Behavioral: if the MJJ demo plays YM music, `identify_music` on the live
  session returns a `replay_addr` near the code the profiler saw hitting
  `$FF8800`; otherwise run against an SNDH fixture loaded into RAM.

---

## P7 — Boot sector extractor

**Goal:** extract + analyze ST boot sectors from `.ST` disk images and RAM:
`$1234` checksum, BPB decode, virus/loader heuristics.

**Depends on:** P3 (engine, `Boot` kind), P2 (disasm of boot code).

**Deliverables**

| Task | Files | Notes / references |
|---|---|---|
| Bootsector plugin | `src/ripper/plugins/bootsector.rs` | 512-byte sector: executable iff big-endian word-sum == `$1234`; decode **little-endian** DOS BPB; classify (plain data / bootable loader / suspicious). Skill: `atari-st-bootsector` (refs: `Atari_ST_Sources/ASM/Adrenalin U.K (ADR)/BOOT_BLK/ZR_BLK.S`, `ADR_BOOT.1_1/`; flow blueprint: `refs/orionfuzion/r1pp3r/src/R1PP3R.S`). |
| Heuristics | `src/ripper/plugins/boot_heur.rs` (if >200 lines) | Virus tells via P2 disasm of the boot code: vector hooks (`hdv_bpb`/`$472` etc., reset vector `$426`), self-copy loops, raw disk writes; label each finding with the triggered heuristics. |
| Disk-image source | `src/ripper/disk.rs` | Read sector 0 from a `.ST` image file (plain sector dump; `.MSA/.DIM` out of scope — note as future work) → feed the same plugin. Path input validated (no traversal). |
| MCP tool | `src/tools/rip.rs` (extend) | `bootsector(source: ram\|disk, path?)` → finding (checksum status, BPB fields, heuristic flags, label); `extract` yields the raw 512 bytes and/or annotated disasm text file. |
| Tests | `tests/bootsector.rs` | Fixtures: a bootable sector (checksum passes), a plain FAT sector, a synthetic "virus-like" sector (vector hook + self-copy) → heuristics fire; checksum edge cases. |

**New MCP tools:** `bootsector`.

**Acceptance criteria**

- fmt + clippy + tests green; checksum/BPB/heuristic goldens on fixtures.
- Behavioral: run `bootsector(source=disk)` on any `.ST` image from
  `refs/`/the archive → correct executability verdict + BPB; `bootsector(
  source=ram)` on a live session with a floppy image inserted reads the
  TOS-buffered sector or reports a clean not-found.

---

## Post-P7 backlog (explicitly out of this plan)

- Real `Deprotect` implementations / dec0de wrapping (interface exists; see ADR).
- Sprite/font ripper (`RipKind::Sprite` reserved).
- Additional depackers (Atomik, Automation, Speed Packer).
- Scriptable code-API "power ripper" mode; named multi-sessions; Xvfb/CI headless.
- `.MSA`/`.DIM` disk-image containers.
