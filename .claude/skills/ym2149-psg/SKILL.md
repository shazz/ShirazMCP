---
name: ym2149-psg
description: Yamaha YM2149 (AY-3-8910-compatible) PSG sound chip on the Atari ST — the 16 register map (tone/noise/mixer/amplitude/envelope), the $FFFF8800/$FFFF8802 access protocol, clock/period math, and how replay routines drive it. Use when analyzing ST sound, locating music replay code, or interpreting writes to the PSG.
---

# YM2149 PSG (Atari ST sound chip)

3 square-wave tone channels (A/B/C) + 1 noise generator + 1 envelope generator.
Clock on ST = **2 MHz** (2000000 Hz). 16 registers (R0–R15), each 8-bit.

## Access protocol — the universal anchor

Two hardware addresses:

| Addr | Access | Purpose |
|------|--------|---------|
| `$FFFF8800` | write | **Register select** (write reg number 0–15) |
| `$FFFF8800` | read | **Read data** from the selected register |
| `$FFFF8802` | write | **Write data** to the selected register |

Typical write: `move.b #reg,$FFFF8800` then `move.b #val,$FFFF8802`.

> 🔑 **Every** ST music player ultimately writes the PSG through `$FFFF8800` /
> `$FFFF8802`. Scanning code/RAM for these constants is the relocation-invariant
> way to locate the active replay routine — see `atari-st-ym-players`.

## Register map

| Reg | Bits | Meaning |
|-----|------|---------|
| R0 | 8 | Channel A tone period, fine |
| R1 | 4 | Channel A tone period, coarse (12-bit total) |
| R2 / R3 | 8/4 | Channel B tone period (fine / coarse) |
| R4 / R5 | 8/4 | Channel C tone period (fine / coarse) |
| R6 | 5 | Noise period (0–31) |
| R7 | 8 | **Mixer / I/O enable** (see below) |
| R8 | 5 | Channel A amplitude: bits 0–3 level, bit 4 = use envelope |
| R9 | 5 | Channel B amplitude |
| R10 | 5 | Channel C amplitude |
| R11 | 8 | Envelope period, fine |
| R12 | 8 | Envelope period, coarse (16-bit total) |
| R13 | 4 | Envelope shape (see below) |
| R14 | 8 | I/O port A (ST: floppy select, RS232 RTS/DTR, monitor) |
| R15 | 8 | I/O port B (ST: printer data) |

### R7 mixer (0 = enabled, active-low)

```
bit 0: tone A enable    bit 3: noise A enable    bit 6: port A dir (1=output)
bit 1: tone B enable    bit 4: noise B enable    bit 7: port B dir
bit 2: tone C enable    bit 5: noise C enable
```

### R13 envelope shape (bits: Continue / Attack / Alternate / Hold)

Common values: `$08` = saw down (repeat), `$0A` = triangle, `$0C` = saw up,
`$09` = decay-once, `$0B`/`$0D` = one-shot ramps then hold.

## Period → frequency math

```
tone_freq_Hz  = 2_000_000 / (16 * period12)        # period12 = R0 | (R1<<8)
noise_freq_Hz = 2_000_000 / (16 * period5)         # period5  = R6 & 31
env_freq_Hz   = 2_000_000 / (256 * period16)       # period16 = R11 | (R12<<8)
```

Volume is logarithmic over 16 levels (bits 0–3 of R8/R9/R10).

## Analysis notes

- **SID-voice / sync-buzzer / digi tricks:** advanced ST players (Hippel, Mad Max)
  reprogram the envelope every scanline or bit-bang R8/R9/R10 for sample playback.
  Rapid, timer-driven writes to R11–R13 or the amplitude regs signal these.
- STE adds a separate DMA sound system at `$FFFF8900+` (not the YM) — distinct
  from PSG chip music.
- See `atari-st-ym-players` for SNDH and player-routine fingerprinting.
