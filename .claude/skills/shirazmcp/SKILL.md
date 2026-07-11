---
name: shirazmcp
description: How to drive the ShirazMCP MCP server — run, freeze/debug, profile, rip (pictures, music, packed data incl. ZX0/MPP, boot sectors), hot-patch 68k code live, persist a hack as a .PRG, and see/hear the sound chip. Use whenever the shiraz-mcp tools are available and you need to boot, inspect, debug, snapshot, reverse-engineer, patch, or extract assets from an emulated Atari ST.
---

# Using the ShirazMCP MCP server

ShirazMCP drives the Hatari Atari ST/STE emulator through **35 MCP tools** — a
"freezer cartridge for agents" with a full Action-Replay toolkit: boot, **freeze**,
inspect CPU/video/sound, disassemble, profile, **rip** graphics/music/packed
data/boot sectors, **hot-patch** 68k live, **persist** the hack, and **watch/search**
memory to reverse-engineer. Prereqs: the `shiraz-mcp` server connected, the `hatari`
binary, an X display (`DISPLAY`); `SHIRAZMCP_VASM` for assemble/patch.

## Mental model: the session lifecycle
```
Stopped ──start──▶ Running ⇄ (pause/resume) ⇄ Paused ──stop──▶ Stopped
```
- **`start`** boots and returns when ready (sentinel-synced). **`stop`** always cleans up.
- **`pause`** = freeze via debugger-break (CPU parks; memory/registers/screenshot/rip all
  work frozen). Never think "hatari-stop" — it's banned; use `pause`.
- Most inspection works Running **or** Paused; **freeze first** when several reads must be
  from the same instant. Single reads/snapshots are already atomic.
- `status` = session state. `machine_state` = the emulated ST's CPU+video+screen. `ym_state`
  = the sound chip's live state. Don't confuse them.

## Tool catalog (35)
**Lifecycle:** `start` · `stop` · `pause` · `resume` · `status`
**Debug & memory:** `registers` · `read_memory` · `write_memory` · `search_memory` ·
`screenshot` · `debug` (raw Hatari debugger command — escape hatch)
**Analyze:** `disasm` (`source=snapshot|live`) · `profile(from,to)` ·
`breakpoint(action=set|list|remove|clear)` · `state(action=save|load)`
**Reverse-engineering (Action-Replay):**
- `watch(addr, on=write)` — **value-change write-watch**; fires when the value at addr
  changes, reports the writing PC (finds "which copy is live / who writes this"). Read-watches
  aren't possible (Hatari limitation) → use `xrefs` for the read case.
- `xrefs(addr)` — static: the instructions that reference an address (from disasm).
- `run_frames(n)` — advance exactly n VBLs then freeze (catch animated/transient states).
- `cheat_scan(action=new|filter|list|reset, predicate?, value?, width?)` — iterative
  memory-diff search (changed/unchanged/increased/decreased/eq/ne/delta) to find a variable
  with NO disassembly. Compose with `run_frames`/`resume` between `filter` steps.
**Ripper:** `rip(kind?,plugin?,depth?)` · `list_ripper_plugins` · `extract(finding_id,as=raw|prg|png)`
· `find_picture` (structure-scored — findings carry a **confidence**, ranked + capped, so noise
no longer floods results) · `identify_music` · `rip_sample(out_path?,force?)` (STE DMA-PCM sample
→ 16-bit WAV consumable by `spectrogram`) · `bootsector(source=ram|disk,path?)` ·
`view_memory_as_screen(addr,res,palette)` · `depack(addr,scheme,len?)` (explicit decompress,
e.g. headerless ZX0 at a known offset)
**Assemble / patch / persist:** `assemble(source,org?)` · `patch(addr,source)` ·
`save_prg(out_path, source_prg?, mode=repatch|snapshot)` — **repatch**: re-emit the original `.PRG`
with your live TEXT/DATA patches baked in; **snapshot**: freeze the whole machine (RAM + CPU +
video/YM/MFP/DMA) into a self-restoring `.PRG` that reboots to the exact frozen instant
(Ultimate-Ripper style; `source_prg` not needed)
**Audio:** `record_audio(seconds)` · `spectrogram(seconds)` (see the music) · `ym_state`
(what the YM2149 is playing now: per-channel note/volume/envelope/mixer)

**Ripper plugins (16):** degas · neochrome · **mpp** (multi-palette) · sndh · ym-player ·
pack-ice(+region) · **atomik**(+region) · **powerpacker**(+region) · **rnc**(+region) ·
bootsector · **zx0** · noop-deprotect.

## Key concepts
- **Addresses are `$`-hex**; load/BSS addresses vary per run — resolve live, don't cache.
- **`Finding` handles, not bytes** — `rip`/`find_picture`/`identify_music`/`bootsector` return
  compact findings; `extract(id, as=…)` materializes (`png` inline + file, `prg`, `raw`).
- **Recursion is free** — `rip(kind=picture, depth=2)` finds a picture packed *inside* Pack-Ice/
  Atomik/PowerPacker/RNC/**ZX0** data; check `via:`.
- **`screenshot`** = Hatari's rendered frame; **`machine_state`**'s render decodes RAM at the
  shifter base (a back buffer on double-buffered demos → shows what's being *drawn*).

## Canonical workflows
- **Boot & look:** `start` → `machine_state` / `screenshot` / `ym_state`.
- **Freeze-then-rip:** `pause` → `rip(kind=picture)` → `extract(id, as=png)`.
- **Peel packed data:** `rip(kind=picture, depth=2)` (auto), or `depack(addr, scheme=zx0)` for a
  headerless blob at a known offset.
- **Identify music:** `identify_music` (SNDH header + `$FF8800` fingerprint; degrades to
  `"unknown"`). `ym_state` for the live chip. `spectrogram(3)` to see it.
- **Debug:** `breakpoint(set, pc=$X)` → `resume` → freezes on hit → `registers`/`disasm(live)`.
  `profile(from,to)` for cycles. `state(save|load)` to snapshot/restore.
- **Reverse-engineer a live value (the RE loop):** to find *who writes* a table/copy, `watch(addr,
  on=write)` → `resume` → the fired PC is the writer. To find a *variable* (score, cursor),
  `cheat_scan(new)` → `run_frames`/`resume` → `cheat_scan(filter, changed|increased)` → narrow →
  `list`. `xrefs(addr)` for static references. See `docs/live-patching.md`.
- **Hot-patch loop:** `pause` → `patch(addr, "…68k…")` (assembles → writes → readback-verifies →
  disasms) → `resume`/step to observe. `assemble` alone for raw bytes.
- **Persist the hack:** after patching, `save_prg(out, source_prg, mode=repatch)` writes a modified
  copy of the original `.PRG` with your live TEXT/DATA patches baked in (BSS/reloc patches can't be
  file-represented — reported, not written). The program must be **unpacked** (a UPX-packed `.PRG`
  is correctly rejected).
- **Freeze the whole machine:** `save_prg(out, mode=snapshot)` on a **paused** session captures all
  of RAM + CPU registers + video/YM/MFP/DMA hardware into a self-restoring `.PRG` — boot it and the
  machine resumes the exact frozen instant (what a hardware freezer cartridge did). Some transient
  state can't be recovered (beam position, in-flight DMA, MFP timer phase) — reported in `not_restored`.

## Gotchas
- **`read_memory` returns at most 1024 bytes/call** (an agent-context guard) and hands back
  `next_addr` — page with it to walk a larger region. Don't request 64 KB expecting 64 KB; you
  get 1 KB. (Whole-RAM work — `rip`, `find_picture`, `machine_state` — uses the internal full-RAM
  dump, not `read_memory`, so it isn't affected.)
- **Don't arm breakpoints via raw `debug("b …")`** — use the managed `breakpoint`/`watch` tools.
- `write_memory`/`patch` chunk long writes internally (Hatari's line limit) — writes of any size work.
- `watch` catches *writes that change the value* (not reads, not same-value writes) — reads → `xrefs`.
- The MJJ demo build: `MJJHACK.PRG` is **UPX-packed and symbol-less**; the sibling `STRGOOSE.PRG`
  is unpacked (use it for `save_prg` / symbol work). Scan live RAM, not the on-disk file.
- Deprotection is interface-only (`noop-deprotect`) — protected programs self-decrypt in RAM anyway.

## Escape hatch
`debug(command)` runs one raw Hatari debugger command (`m`, `evaluate`, `symbols`, `info ym`,
`savebin`, …) and returns its output. See `docs/plan/*.md` (per-feature detail),
`docs/live-patching.md` (RE/patch tutorial), `ARCHITECTURE.md`/`decisions.md`.
