---
name: atari-st-blitter
description: Atari ST(E) Blitter (BLiTTER, $FFFF8A00) — register map (source/dest addresses & increments, endmasks, logical & halftone operations, skew/NFSR/FXSR, line/word counts), how block transfers work, and timing. Use when analyzing or interpreting blitter-driven graphics, disassembly touching $FF8Axx, or estimating blit cost. Full Atari BLiTTER manual is bundled in reference/.
---

# Atari ST(E) Blitter

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
FXSR adds a read per line. See `reference/blitter_execution_times.md` for the cost
table and `reference/blitter_manual.md` for the full official manual (bundled from
the MJJ project). Use these when the MCP estimates blit cost between breakpoints
(see the profiler / cycle-counter feature).

## Reference files

- `reference/blitter_manual.md` — full Atari BLiTTER manual.
- `reference/blitter_execution_times.md` — timing/cost notes.
