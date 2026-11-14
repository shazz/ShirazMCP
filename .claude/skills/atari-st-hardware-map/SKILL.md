---
name: atari-st-hardware-map
description: Atari ST(E) hardware and system-variable memory map — the $FFFF82xx–$FFFF8Axx hardware register banks (Shifter, MMU, DMA, PSG, MFP 68901, ACIA, Blitter, STE DMA sound/DSP) and low-memory system variables. Use when annotating an address with its register name/meaning, interpreting reads/writes in a disassembly or memory dump, or building the MCP address→register mapper..
---

# Atari ST(E) memory map & hardware registers

## Step 0: check the reference files

This skill's tables are summaries. For anything beyond them, read the full third-party
document(s), which are NOT in this repo (licence) and live in your local `refs/` (gitignored):
  - `refs/skills/atari-st-hardware-map/memory_map.txt`

Before relying on them, run:

    python3 tools/fetch_refs.py --check --skill atari-st-hardware-map

- `ok`: read the file(s) above.
- `missing` or `mismatch`: run `python3 tools/fetch_refs.py --fetch --skill atari-st-hardware-map` (restores
  from this repo's git history or the listed source, verified by sha256). If it still reports
  missing, tell the user which document is missing and where to get it (the script prints the
  sources), and continue from this skill's summary only: never fill in a table value from memory
  and present it as coming from the document.

Seed data for the MCP's **address→register annotator**: given any address, return
its region, register name, and meaning so results carry metadata (e.g.
`$FF8240 → "Shifter — palette register 0"`). The full labelled map is bundled at
`src/analyze/memory_map.txt` (the Hollis list, see "Reference" below) — the annotator loads
and indexes it.

## Region overview

| Range | Block | Examples |
|-------|-------|----------|
| `$000000–$0007FF` | System variables | `$44C` shiftmode copy, `$45A` colorptr, VBL/timer vectors |
| `$FFFF8000–$8203` | MMU / memory config | memory controller config |
| `$FFFF8201–$820F` | **Shifter** video base/offset | screen base hi/mid/lo, line width |
| `$FFFF8240–$825E` | **Shifter palette** 0–15 | 16 color words |
| `$FFFF8260` | Shifter resolution | bits 1–0 = res |
| `$FFFF8604–$860D` | **DMA disk** controller | FDC/HDC sector count, data |
| `$FFFF8800 / $8802` | **YM2149 PSG** | register select / write data |
| `$FFFF8900–$8925` | **STE DMA sound** | control, frame start/end, mode |
| `$FFFF8A00–$8A3D` | **Blitter** | source/dest, endmasks, ops (STE) |
| `$FFFF8922/8924` | STE **microwire** (volume/tone) | |
| `$FFFFFA00–$FA2F` | **MFP 68901** | timers A–D, interrupt enable/pending/mask, GPIP |
| `$FFFFFC00–$FC06` | **ACIA** (keyboard/MIDI) | 6850 control/data |

## Key specifics worth annotating precisely

- **Palette:** `$FF8240 + reg*2` = palette register `reg` (0–15).
- **Screen base:** `$FF8201/03/0D` (hi/mid/lo). `$FF8205/07/09` = live read pointer.
- **PSG:** `$FF8800` select, `$FF8802` write — annotate the *selected* register when
  a `move.b #n,$FF8800` precedes (see `ym2149-psg`).
- **MFP timers:** `$FFFA1F/21/23/25` = timer A/B/C/D data; `$FFFA07/09/0B/0D/0F/11`
  = interrupt enable/pending/mask A/B — critical for music/raster timing.
- **Blitter:** whole `$FF8A00` bank (see `atari-st-blitter`).
- **VBL / timer vectors** in low memory (`$70` VBL, `$68` HBL, `$100+` MFP vectors).

## Annotator behavior (design intent)

- Exact hit → register name + field description.
- Range hit (e.g. inside palette bank) → computed index (`palette register N`).
- Nearby (±small) → "N bytes into <register>".
- Context-aware for PSG: track the last `$FF8800` select to name the target reg.

## Reference (not bundled in this skill)

- *Atari ST/STe/MSTe/TT/F030 Hardware Register Listing* v7.0 (Dan Hollis, 1994) - the canonical
  per-address list with bit layouts. Its licence ("may only be copied unmodified, in its entirety,
  freely, not sold") is NOT MIT; this repo's server embeds a copy at `src/analyze/memory_map.txt`
  (see the licence note in README). Do not copy it elsewhere.
- TOS.HYP system variables and BIOS/XBIOS/GEMDOS docs (FreeMiNT project, e.g. freemint.github.io/tos.hyp).
- Atari Compendium; Hatari source (`src/ioMem*.c`) for what the emulator actually decodes.
- Not covered here: the ACSI/DMA disk interface - see `atari-st-acsi-hdd`.
