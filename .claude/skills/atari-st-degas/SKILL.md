---
name: atari-st-degas
description: Degas and Degas Elite image formats — PI1/PI2/PI3 (uncompressed) and PC1/PC2/PC3 (RLE-compressed) byte layouts, resolution header word, palette block, and the PackBits-style compression scheme. Use when detecting, decoding, or ripping .PI?/.PC? images from Atari ST memory or files.
---

# Degas / Degas Elite image formats

Both wrap the standard ST interleaved-bitplane screen (see `atari-st-bitplanes`).
The resolution of the file must match its data layout (low=4 planes, etc.).

## PI1 / PI2 / PI3 — uncompressed (32034 bytes)

| Offset | Size | Field |
|--------|------|-------|
| `0` | word | Resolution: `0`=low (PI1), `1`=med (PI2), `2`=high (PI3) |
| `2` | 16 words (32 B) | Palette registers 0–15 (ST/STE word format) |
| `34` | 32000 B | Screen data, standard interleaved planes |

Total = 2 + 32 + 32000 = **32034 bytes**. Fixed size → trivial to detect: a
region whose first word is 0/1/2 followed by 16 plausible palette words and 32000
bytes of plane data.

## PC1 / PC2 / PC3 — Degas Elite compressed

| Offset | Size | Field |
|--------|------|-------|
| `0` | word | Resolution **with bit 15 set**: `$8000`=low (PC1), `$8001`=med (PC2), `$8002`=high (PC3) |
| `2` | 16 words | Palette |
| `34` | var | RLE-compressed screen data (decodes to 32000 bytes) |

Detection anchor: **first word has the high bit set** (`(w & 0x8000) != 0`) and
low bits are 0/1/2.

### Compression: PackBits-style RLE

Read a signed control byte `c`, then:

- `c` in `0x00..0x7F` (0…127): **literal** — copy the next `c + 1` bytes verbatim.
- `c` in `0x80..0xFF` (as signed −128…−1): **run** — repeat the next single byte
  `257 - c` times (i.e. `1 - signed(c)`... equivalently `(256 - c) + 1`). Skip
  `c == 0x80` (no-op / boundary in some encoders).

Decode until 32000 output bytes are produced.

### Important: plane/scanline ordering after decompression

Degas Elite does **not** compress the screen as one flat interleaved buffer. The
compressed stream is organized **per scanline, one bitplane's worth of a line at a
time** (all plane-0 words of the line, then plane-1, …), and the decompressed
bytes must be **re-interleaved** back into the standard word-interleaved screen
layout before the bitplane decoder (`atari-st-bitplanes`) will read it correctly.

> ⚠️ Verify the exact column/plane interleave against a known reference decoder
> (e.g. the `recoil`/`libgd` Degas readers) before trusting a PC1 rip — the header
> and RLE control bytes above are solid, but the post-decompress reshuffle is the
> easy place to get a garbled image.

## Companion palette-animation file (.PI?/.PC? + Degas Elite)

Degas Elite may store color-cycling metadata after the image in Elite variants;
plain Degas does not. Not needed for a static rip.

## Reference implementations (in `../Atari_ST_Sources`)

Paths relative to `$HOME/projects/Atari_ST_Sources`:

- **`C/Warwick Grigg/C_series.5/DEGAS.C`** — cleanest self-contained **C**: `degashdr_s`
  (res + 16 palette), a put-pixel unpacker handling the **compressed PC formats** with
  plane-by-plane de-interleaving. Port the PC1 decode + reshuffle from here.
- **`GFA Basic/DBA/DEGASPC1.TXT`** — plain-language "DEGAS.PC1 UNPACKER" walkthrough of
  the PC1 RLE scheme with a working listing (also `DEGASPC2.TXT`, `DEG_READ.TXT`).
  Good prose to verify the control-byte semantics + ordering.
- **`C/Bill Rosenkranz/pi1tomon/PI1TOMON.C`** — compact uncompressed **PI1** reader
  (16000-word screen buffer + palette).

## Ripper notes

- PI1 is the highest-value, lowest-risk target: fixed size, no decompression.
- For `find_picture`, scan for the resolution word + palette-plausibility, then
  hand the 32000-byte body to the shared bitplane decoder → PNG.
- See `atari-st-neochrome` for the other common still-image format (NEO).
