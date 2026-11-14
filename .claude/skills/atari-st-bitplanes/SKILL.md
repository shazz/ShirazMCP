---
name: atari-st-bitplanes
description: Atari ST screen memory layout — interleaved bitplanes, the three ST resolutions (low/medium/high), pixel-to-color-index decoding, palette word formats (ST 3-bit vs STE 4-bit), and Shifter/screen-base hardware registers. Use when decoding, rendering, or ripping ST screen data, converting planar↔chunky, or interpreting $FF82xx video registers.
---

# Atari ST bitplane / screen format

Every ST screen is **exactly 32000 bytes**, regardless of resolution. Only the
plane count and geometry change.

| Res | Value | Geometry | Colors | Planes | Bytes/line |
|-----|-------|----------|--------|--------|------------|
| Low | 0 | 320×200 | 16 | 4 | 160 |
| Medium | 1 | 640×200 | 4 | 2 | 160 |
| High | 2 | 640×400 | mono | 1 | 80 |

32000 = bytes/line × lines in every mode (160×200, 160×200, 80×400).

## Interleaved planes (the key idea)

Pixels are stored **planar and word-interleaved**. Horizontal pixels are grouped
in blocks of 16 (one word per plane). For each 16-pixel group the plane words are
stored consecutively:

```
Low res, one 16-pixel group = 4 words (8 bytes):
  word0 = plane 0 bits   (LSB of color index)
  word1 = plane 1 bits
  word2 = plane 2 bits
  word3 = plane 3 bits   (MSB of color index)
then the next 16-pixel group follows immediately.
```

Pixel `p` (0 = leftmost) within a group uses bit `15-p` of each plane word. Its
color index is:

```
index = plane0[15-p]<<0 | plane1[15-p]<<1 | plane2[15-p]<<2 | plane3[15-p]<<3
```

- **Medium res:** 2 words/group (plane0, plane1), 40 groups/line.
- **High res:** 1 word/group, mono, index 0/1.

## Decode loop (chunky output)

```
for line in 0..H:
    base = line * bytes_per_line
    for group in 0..(W/16):
        words = read `planes` words at base + group*planes*2   # big-endian
        for p in 0..16:
            idx = 0
            for pl in 0..planes: idx |= ((words[pl] >> (15-p)) & 1) << pl
            emit pixel (group*16 + p, line) = palette[idx]
```

All ST words are **big-endian**. Screen data conventionally starts on a 256-byte
boundary (screen base low byte is often ignored on plain ST).

## Palette word format

16 palette entries (low res). Each is one 16-bit word.

- **ST (3-bit per channel, 0–7):** `%0000 0RRR 0GGG 0BBB`
  - Red = bits 8–10, Green = bits 4–6, Blue = bits 0–2.
- **STE (4-bit per channel, 0–15):** same nibble positions but each channel is 4
  bits, and **the least-significant bit is stored in the high (bit-3) position of
  the nibble**. A 4-bit value `%d3 d2 d1 d0` is stored as `%d0 d3 d2 d1`. This
  keeps the top 3 bits ST-compatible. To read an STE channel nibble `n`:
  `val4 = ((n & 7) << 1) | ((n >> 3) & 1)`.

Scale to 8-bit for PNG: ST `v*255/7`, STE `v*255/15`.

## Relevant hardware registers (Shifter)

| Addr | Size | Meaning |
|------|------|---------|
| `$FFFF8201` | byte | Screen base, high byte |
| `$FFFF8203` | byte | Screen base, mid byte |
| `$FFFF820D` | byte | Screen base, low byte (STE) |
| `$FFFF8205/07/09` | byte | Live video address pointer (hi/mid/lo, read) |
| `$FFFF820F` | byte | STE line width (extra words per line) |
| `$FFFF820E` | word | STE horizontal fine scroll / line offset |
| `$FFFF8240…$FF825E` | word×16 | Palette registers 0–15 |
| `$FFFF8260` | byte | Resolution (bits 1–0: 0 low, 1 med, 2 high) |
| `$FFFF820A` | byte | Sync mode (bit 1: 0 = 60 Hz, 1 = 50 Hz) |

## Notes for ripping

- A candidate screen is any 32000-byte region; score by palette sanity (16 valid
  words nearby) and plane entropy. Low res is the common demo/game case.
- Sprites/fonts reuse the same interleaved layout at smaller widths — a "sprite"
  is usually N planes × 16px-wide word columns.
- See also: `atari-st-degas` (PI1/PC1 wrap this data), `atari-st-neochrome` (NEO).

## Reference implementations (third-party, not bundled)

Paths relative to the root of a local clone of the public Atari_ST_Sources collection (https://github.com/ggnkua/Atari_ST_Sources; clone it yourself — it is NOT bundled and each file keeps its own author's licence):

- **`ASM/Lineout/Stone Tower/src/STLOWC2P.S`** — textbook **chunky→planar (c2p)** for
  ST-low via lookup table; clearest plane-interleave reference (reverse for decode).
- **`C/Warwick Grigg/C_series.5/DEGAS.C`** — doubles as a **planar→chunky** decode
  reference (per-plane bit extraction, all 3 resolutions).
- **`GFA Basic/James Collett (Professor)/GFA/BITPLANE.TXT`** — prose explainer of the
  interleaved-bitplane layout.
- **`C/Dominique Béréziat/VIEWER/SRC/COLOR.C`** — ST/STE palette/color handling.

### Other ST picture formats (references, not yet skills)

- **Spectrum 512** (.SPU/.SPC, per-scanline palette, 19200-byte pal):
  `GFA Basic/Damien M. Jones/SPECTRUM.GFA/DMJ.GFA/SPC_DCOM.S` (annotated 68k),
  `ASM/Mcoder/10-20-2007/SPCUNPK.C` (compact C `Spc_unpack`).
- **IFF/ILBM** (`cmpByteRun1` PackBits, FORM/BMHD): `C/Tom Hudson/IFF.STQ/IFFRTNS.C`
  (by the Degas Elite author).
- **Tiny (.TNY):** no clean decoder found in the archive — gap to fill from spec.
