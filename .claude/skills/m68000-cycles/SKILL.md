---
name: m68000-cycles
description: Motorola 68000 instruction timing on the Atari ST (8 MHz) — cycle counting rules, effective-address timing, even-cycle rounding, ST bus/video contention, and how to estimate the cost of a code range. Use when profiling, cycle-counting between breakpoints, or reasoning about whether a routine fits in a scanline/VBL budget.
---

# 68000 instruction timing (Atari ST)

## Step 0: check the reference files

This skill's tables are summaries. For anything beyond them, read the full third-party
document(s), which are NOT in this repo (licence) and live in your local `refs/` (gitignored):
  - `refs/skills/m68000-cycles/68000_execution_cycles.md`

Before relying on them, run:

    python3 tools/fetch_refs.py --check --skill m68000-cycles

- `ok`: read the file(s) above.
- `missing` or `mismatch`: run `python3 tools/fetch_refs.py --fetch --skill m68000-cycles` (restores
  from this repo's git history or the listed source, verified by sha256). If it still reports
  missing, tell the user which document is missing and where to get it (the script prints the
  sources), and continue from this skill's summary only: never fill in a table value from memory
  and present it as coming from the document.

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

Use Motorola's *M68000 Family Programmer's Reference Manual* (section "Instruction Execution Times")
or the Atari-Forum wiki / Yacht-style tables for the full table (EA modes, per-instruction detail) rather than trusting these anchors for anything tight.

## Atari ST contention (why static ≠ real)

Static instruction sums are a **lower bound**. On the ST the Shifter and MMU steal
bus cycles from the CPU during active display, and RAM/ROM/hardware regions differ:

- Reads/writes to video RAM during display can stall the CPU.
- Access to `$FFFF82xx`/`$FFFF88xx` hardware and the MFP has its own timing.
- On the ST every CPU bus access is rounded up to a 4-cycle boundary (bus slots shared with the video
  shifter), which is why all timings are rounded to multiples of 4. Treat static sums as lower bounds
  and confirm with `profile` / `profile_frame`.

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

## Reference material (not bundled - third-party text)

- *M68000 Family Programmer's Reference Manual* (Motorola/NXP), "Instruction Execution Times".
- Atari-Forum wiki 68000 timing pages. Bundled tables were removed from this repo for licence reasons.
