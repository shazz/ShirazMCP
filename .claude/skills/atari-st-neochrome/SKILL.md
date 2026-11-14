---
name: atari-st-neochrome
description: NeoChrome .NEO image format — the 128-byte header (flag, resolution, 16-word palette, filename, color-animation fields, geometry) followed by 32000 bytes of screen data. Use when detecting, decoding, or ripping .NEO images from Atari ST memory or files.
---

# NeoChrome (.NEO) image format

Fixed **32128 bytes** = 128-byte header + 32000-byte interleaved screen (see
`atari-st-bitplanes`). Almost always low resolution.

## Header (128 bytes)

| Offset | Size | Field |
|--------|------|-------|
| `0` | word | Flag byte (`0`) — reserved, normally `$0000` |
| `2` | word | Resolution: `0`=low, `1`=med, `2`=high |
| `4` | 16 words (32 B) | Palette registers 0–15 (ST/STE word format) |
| `36` | 12 B | Filename, space-padded (e.g. `"        .   "`) |
| `48` | word | Color-animation limits: bit 15 = enabled, bits 0–3 = first (left) color, bits 4–7 = last (right) color |
| `50` | word | Color-animation speed/direction: bit 15 = direction, bits 0–7 = delay (1/60 s), `0` = off |
| `52` | word | Color-animation step count |
| `54` | word | Image X offset on screen (usually `0`) |
| `56` | word | Image Y offset (usually `0`) |
| `58` | word | Image width in pixels (usually `320`) |
| `60` | word | Image height in pixels (usually `200`) |
| `62` | 33 words (66 B) | Reserved (zero) |
| `128` | 32000 B | Screen data, standard interleaved planes |

## Detection

Strong, cheap signal: **first word `$0000`, second word in `{0,1,2}`**, followed
by 16 plausible palette words. Optionally sanity-check width=320/height=200 at
`+58/+60`. Then the 32000-byte body at `+128` decodes with the shared bitplane
decoder.

## Reference implementations (third-party, not bundled)

Paths relative to the root of a local clone of the public Atari_ST_Sources collection (https://github.com/ggnkua/Atari_ST_Sources; clone it yourself — it is NOT bundled and each file keeps its own author's licence):

- **`C/Jean Lusetti/Vision 4.1a/TOOLS/NEO_IO.C`** — best reference: an explicit
  `NEO_HEADER` struct documenting every field (zero word, res, 16-word palette,
  12-byte anim name, anim/step, offsets, width/height, reserved) + `neo_load`.
- **`C/Lee Russell/NEO_SHOW/PICSHOW.C`** — compact GEM/VDI viewer (header-skip +
  32000-byte blit).
- **`C/Thomas Weinstein/NEO2MONO/NEO2MONO.C`** — another small header/screen decode.

## Ripper notes

- Simpler than Degas PC1 (no compression) but has a bigger header — don't confuse
  the palette offset: NEO palette is at `+4`, Degas at `+2`.
- The color-animation fields are worth surfacing as metadata (a cycling picture is
  a nice `Finding` annotation), but not required to render a still.
- See `atari-st-degas` for PI1/PC1 and `atari-st-bitplanes` for the pixel decode.
