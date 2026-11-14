---
name: atari-st-blitter
description: Atari ST(E) Blitter (BLiTTER, $FFFF8A00) — register map (source/dest addresses & increments, endmasks, logical & halftone operations, skew/NFSR/FXSR, line/word counts), how block transfers work, and timing. Use when analyzing or interpreting blitter-driven graphics, disassembly touching $FF8Axx, or estimating blit cost..
---

# Atari ST(E) Blitter

## Step 0: check the reference files

This skill's tables are summaries. For anything beyond them, read the full third-party
document(s), which are NOT in this repo (licence) and live in your local `refs/` (gitignored):
  - `refs/skills/atari-st-blitter/blitter_manual.md`
  - `refs/skills/atari-st-blitter/blitter_execution_times.md`

Before relying on them, run:

    python3 tools/fetch_refs.py --check --skill atari-st-blitter

- `ok`: read the file(s) above.
- `missing` or `mismatch`: run `python3 tools/fetch_refs.py --fetch --skill atari-st-blitter` (restores
  from this repo's git history or the listed source, verified by sha256). If it still reports
  missing, tell the user which document is missing and where to get it (the script prints the
  sources), and continue from this skill's summary only: never fill in a table value from memory
  and present it as coming from the document.

Hardware block-transfer coprocessor at **`$FFFF8A00`** (not present on TT). Moves
rectangular bit-blocks with per-word masking, a 16-way logic op, bit-shift (skew),
and an optional halftone/pattern. Runs at ~8 or 16 MHz (`$FFFA` blitter-done bit).

## Register map (`$FFFF8A00`–`$FFFF8A3D`)

| Addr | Size | Register |
|------|------|----------|
| `$FF8A00`–`$FF8A1E` | 16 words | Halftone RAM (pattern), words 0–15 |
| `$FF8A20` | word | Source X increment (signed, even) |
| `$FF8A22` | word | Source Y increment (signed, even) |
| `$FF8A24` | long | Source address (24-bit, even) |
| `$FF8A28` | word | Endmask 1 (first word write of each line) |
| `$FF8A2A` | word | Endmask 2 (middle word writes) |
| `$FF8A2C` | word | Endmask 3 (last word write of each line) |
| `$FF8A2E` | word | Destination X increment (signed, even) |
| `$FF8A30` | word | Destination Y increment (signed, even) |
| `$FF8A32` | long | Destination address (24-bit, even) |
| `$FF8A36` | word | Words per line (X count; 0 = 65536) |
| `$FF8A38` | word | Lines per block (Y count; 0 = 65536) |
| `$FF8A3A` | byte | Halftone operation (bits 1–0) |
| `$FF8A3B` | byte | **Logical operation** (bits 3–0) |
| `$FF8A3C` | byte | Line number (bits 3–0) + **BUSY bit 7** + HOG bit 6 + SMUDGE bit 5 |
| `$FF8A3D` | byte | **SKEW** (bits 3–0) + **NFSR** bit 6 + **FXSR** bit 7 |

## Logical operation (`$FF8A3B`, 16 ops)

`dest = f(source, dest)` per the 4-bit op number. Notable ones:

| Op | Result |
|----|--------|
| `0` | 0 (clear) |
| `3` | source (copy) |
| `4` | source AND NOT dest |
| `6` | source XOR dest |
| `7` | source OR dest |
| `11` | source OR NOT dest |
| `15` | 1 (set) |

Masked replace (op 3 + endmasks) is the workhorse for sprites; op 6 (XOR) for
reversible draws.

## Skew / FXSR / NFSR

- **SKEW** (0–15): right-shift the source by N bits to align odd pixel positions.
- **FXSR** (First eXtra Source Read): pre-load an extra source word at line start
  when the shift needs data from before the first dest word.
- **NFSR** (No Final Source Read): suppress the last source read when the shift
  doesn't need the trailing word. Getting FXSR/NFSR wrong shifts or corrupts the
  right edge.

## Start & synchronization

Set BUSY (bit 7 of `$FF8A3C`) to launch. In **HOG mode** (bit 6) the blitter grabs
the bus until done; otherwise it shares the bus with the CPU in 64-bus-cycle bursts.
Poll BUSY or use the MFP "blitter done" interrupt (`$FFFA` GPIP bit 3, vector at
`$0000010C`).

## Timing

Roughly **1 bus word / clock** once running, plus per-line and setup overhead;
FXSR adds a read per line. Cost per word depends on HOP x LOP: a plain copy (HOP=2, LOP=3)
runs about 2 NOP-equivalents/word and the cheapest ops (LOP 0, 12, 15) about 1, assuming the
blitter is the only bus master (rule of thumb from the Atari-Forum wiki timing table - verify with
`profile` on the real routine). Use these when estimating blit cost between breakpoints.

## Reference material (not bundled - third-party copyright)

- *Atari BLiTTER manual* (Atari Corp., 1987) - the register/bit-level authority. Find it on the
  Atari-Forum wiki / atari-wiki.com ("Blitter") or in the Atari Compendium.
- *The Atari ST(E) BLiTTER in brief* (The Paranoid of Paradox, 2012) - practical FAQ: HOG vs
  cooperative restart idioms, common mistakes.
- Atari-Forum wiki "Blitter Execution Times" - the HOP x LOP timing table.
- See also the `ste-blitter-expert` agent (`.claude/agents/`).
