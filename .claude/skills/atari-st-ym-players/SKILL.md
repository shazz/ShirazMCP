---
name: atari-st-ym-players
description: SNDH file format and Atari ST YM2149 music-player (replay routine) identification — the SNDH header/tags, the init/exit/play entry points, and how to fingerprint known drivers (Jochen Hippel/Mad Max, TCB Tracker, maxYMiser, Lucas/Quartet, etc.) in memory. Use when detecting SNDH data, naming a replay routine, or building the music-player signature DB.
---

# SNDH & YM player-routine identification

Goal: recognize *which* music driver is running/embedded — the ripper-cartridge
"oh, this is a Mad Max player" feature. Three layers, easy → clever.

## 1. SNDH header (easiest, highest value)

SNDH = the original song data + replay code with a header bolted on top, exposing a
**uniform calling interface** regardless of the underlying driver.

Layout:

```
offset 0 : 3 × 68k branch instructions (the entry points), each reachable via:
             +0  init   (d0 = subtune number)
             +4  exit / deinit
             +8  play   (call once per VBL / timer tick)
offset 12: ASCII magic 'SNDH'
then     : null-terminated TAG blocks, then 'HDNS' terminator
```

Common tags (4-char id + value):

| Tag | Meaning |
|-----|---------|
| `TITL` | Title |
| `COMM` | Composer |
| `RIPP` | Ripper (who ripped it) |
| `CONV` | Converter |
| `YEAR` | Year |
| `##` | Subtune count (e.g. `##04`) |
| `TIME` | Per-subtune durations (words, seconds) |
| `TA`/`TB`/`TC`/`TD` | Timer used + frequency (MFP timer A/B/C/D) |
| `!V` | VBL replay frequency |
| `FREQ`/`TIME` | Replay-rate metadata |
| `HDNS` | End of header |

Detection: `search_memory("SNDH")`, then parse tags backward/forward from +12.
Gives title/composer/subtunes with zero fingerprinting.

## 2. Universal anchor: the YM write loop

If there's no SNDH wrapper (raw ripped player relocated in RAM, possibly Pack-Ice
depacked), locate the driver by its unavoidable behavior: **it writes the PSG via
`$FFFF8800` / `$FFFF8802`** (see `ym2149-psg`). Scan code for these constants →
locate the `play` routine's register-write block. This is relocation-invariant.

## 3. Signature DB → name the driver

Around the write loop, fingerprint with **relocation-tolerant** signatures: match
on opcode sequences + immediate constants + the shape of the register-write table,
**ignoring absolute address operands** (they differ per relocation/depack).

Well-known drivers to seed the DB:

| Driver | Notes |
|--------|-------|
| Jochen Hippel (Mad Max) | TCB/Hubbard-style; SID-voice & sync-buzzer; many `.SID`/`.HIP` |
| Mad Max original | The Exceptions demos, B.I.G. Demo lineage |
| TCB Tracker | Tracker format, popular in the scene |
| maxYMiser | Modern (gwEm) tracker, rich effects |
| Lucas / Quartet | 4-voice via software mixing |
| ProTracker STk / Music Mon | Amiga-style / monitor players |
| David Whittaker, Delta Music | Game-music drivers |

Seed sources: [`frno7/sndhfix`](https://github.com/frno7/sndhfix) (already encodes
SNDH/player knowledge) and the labelled [SNDH archive](https://sndh.atari.org/fileformat.php)
corpus. Fingerprints should be extracted from these, not hand-guessed.

## `identify_music()` output shape

```
{ sndh_header?: { title, composer, subtunes, timer, freq },
  replay_addr: $xxxxx,        # via $FF8800 anchor
  player: "Jochen Hippel (Mad Max)" | "unknown",
  confidence: 0.0..1.0 }
```

## Extraction

To rip as a runnable `.PRG` (the Ultimate Ripper trick): wrap the located player +
song data with the SNDH entry-point stubs and a tiny VBL/timer install shim.

## Reference implementations (third-party, not bundled)

Paths relative to the root of a local clone of the public Atari_ST_Sources collection (https://github.com/ggnkua/Atari_ST_Sources; clone it yourself — it is NOT bundled and each file keeps its own author's licence):

- **`ASM/Mcoder/unreleased/MADMAX/DISK/MEGARIP.S`** — 68k "MegaRip": freezes via
  VBL/MFP and captures YM2149 (`$FFFF88xx`) + MFP register writes — a soundchip
  ripper/replay harness. Closest thing to a live music grabber.
- **`ASM/The Nerve Center (TNC)/MUSICRIP.TXT`** — tutorial + inlined replay source
  ("Misfit Music Rip"); documents ripping chip music from memory with an Ultimate
  Ripper / MonST, and gives the replay wrapper.
- **`ASM/The Light Team (TLT)/MUSICRIP.ING/PLAY_IT.S`** — 3-channel replay routine
  (init/play convention) for ripped tunes.
- **`ASM/Various/Mug UK/JOCHEN.HIP/…/OOH_RIPS.S`** and the `ASM/Various/Mug UK/
  NEW_RIPS.MUG`, `SNDIMAGE/` trees — ripped **Mad Max / Jochen Hippel** tunes with
  their extracted replay wrappers (fingerprint fodder for the driver DB).

> Note: these locate-then-rip via a human/cartridge; a pure `$FF8800`-scanning
> *auto-detector* wasn't found in the archive — that part is ours to build (the
> anchor scan above), but these give the replay/extract half for free.

### SNDH header parsing & player skeletons

- **`C/Toyoyo/stvn/src/sndh.c` + `sndhisr.s` (+ `sndh.h`)** — cleanest **SNDH header/
  tag parser** (derived from GODLIB) + a Timer-C ISR calling init/exit/play. Start
  here for the `sndh` detector plugin.
- **`ASM/T.O.Y.S/tpmtool/source/src/sndh_parser.cpp`** (+ `sndh.hpp`, `sndh_writer.cpp`)
  — full **C++** SNDH tag parser *and writer* (exact `SNDH`/`TITL`/`COMM`/`TC` bytes).
- **`ASM/Reservoir Gods (RG)/TRIPLEX.2K4/SNDH/DRIVER.S`** — canonical replay skeleton:
  `bra.w init / bra.w exit / bra.w vbl` header + full sound-hardware save/restore
  (`WRAP.S` wraps it into an SNDH). Model for the entry-point layout.
- **`ASM/Preromanbritain/SNDH_CPU/SNDH_CPU.S`** (+ `SOURCE/PLAY_TC.S`, `PLAY_VBL.S`) —
  standalone SNDH player showing Timer-C vs VBL replay drivers.
- **`ASM/Adrenalin U.K (ADR)/ADR_PLAY/ADR_PLAY.S`, `MOD_PLAY.S`, `PLAY_TUN.S`** —
  diskmag music/MOD players (another replay-wrapper style).
