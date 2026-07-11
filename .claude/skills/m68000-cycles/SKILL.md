---
name: m68000-cycles
description: Motorola 68000 instruction timing on the Atari ST (8 MHz) — cycle counting rules, effective-address timing, even-cycle rounding, ST bus/video contention, and how to estimate the cost of a code range. Use when profiling, cycle-counting between breakpoints, or reasoning about whether a routine fits in a scanline/VBL budget. Bundled reference tables in reference/.
---

# 68000 instruction timing (Atari ST)

The ST CPU is a **68000 at 8 MHz** → 1 clock = 125 ns. A VBL at 50 Hz is
**160256 cycles**; one scanline ≈ 512 cycles (313 lines/frame). Fitting an effect
in-budget is the whole game — this skill backs the MCP's profiler.

## Core rules

- **Everything is even.** The 68000 takes an even number of clock cycles per
  instruction (bus cycles are 4 clocks; internal ops pad to even).
- **Cost = internal cycles + memory-access cycles.** Timing tables give a base per
  instruction plus **effective-address (EA) calculation** time for each operand.
- **Word vs long.** Long operations on memory cost an extra bus access (≈ +4/+8).
- **Branches:** `Bcc` taken vs not-taken differ (e.g. taken ≈ 10, not-taken ≈ 8/12
  depending on size). `DBcc` loop overhead matters in tight loops.

Rough anchors (register-direct, .w unless noted):

| Op | Cycles |
|----|--------|
| `move.w Dn,Dm` | 4 |
| `move.w (An),Dn` | 8 |
| `move.w d(An),Dn` | 12 |
| `move.l` (mem) | +4 over .w |
| `add/sub/and/or Dn,Dm` | 4 |
| `muls/mulu` | ~70 (data-dependent) |
| `divs/divu` | ~140/158 (data-dependent) |
| `bra` | 10 · `bsr` 18 · `rts` 16 |
| `movem` | 8/12 + 4–8 per register |

Use the bundled `reference/68000_execution_cycles.md` for the full table (EA modes,
per-instruction detail) rather than trusting these anchors for anything tight.

## Atari ST contention (why static ≠ real)

Static instruction sums are a **lower bound**. On the ST the Shifter and MMU steal
bus cycles from the CPU during active display, and RAM/ROM/hardware regions differ:

- Reads/writes to video RAM during display can stall the CPU.
- Access to `$FFFF82xx`/`$FFFF88xx` hardware and the MFP has its own timing.
- The `cycle_counter_plan.md` reference documents the ST-specific contention model
  worked out for the MJJ demo — reuse it.

## Two ways the MCP measures cost

1. **Ground truth — Hatari profiler.** Hatari's debugger can profile the CPU
   (`profile on`, then read counts/cycles per address/symbol). Between two
   breakpoints this gives *measured* cycles including real contention. Prefer this
   for accuracy.
2. **Static estimate — disassemble + sum.** Walk the instruction stream (embedded
   Capstone M68K over an acquired `MemImage`) and sum table cycles. Fast, offline,
   no run needed — good for "what-if" and for annotating a disassembly, but it
   ignores contention, so mark it as an estimate.

`profile(from, to)` → `{ measured_cycles?, static_cycles, instructions, vbl_fraction }`

## Reference files

- `reference/68000_execution_cycles.md` — full instruction/EA timing tables.
- `reference/cycle_counter_plan.md` — ST bus/video contention model (from MJJ).
