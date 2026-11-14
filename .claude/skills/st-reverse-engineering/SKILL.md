---
name: st-reverse-engineering
description: General method for reverse-engineering existing Atari ST/STE software (a .PRG/.TOS/.ACC, a .ST/.MSA/.DIM disk image, a boot-sector demo) with ShirazMCP - identify and inspect statically first, then escalate to disassembly and finally the emulator; ripping music/graphics/tables; TOS conventions that explain odd code; ShirazMCP gotchas. Use at the start of any "what does this program do / get this asset out / patch this" task.
---

# Reverse-engineering Atari ST software

The cheapest evidence first. Escalate only when the previous rung stops paying:

1. **Header, strings, signatures** - free, always do it.
2. **Static rip** of music / pictures / tables - a file read and a few lines of Python.
3. **Disassembly** of the interesting routine - slow, but it is where the algorithm lives.
4. **Emulator (ShirazMCP)** - for packed/encrypted code (let it unpack itself, then freeze and
   read RAM), self-modifying or timing-dependent code, and to watch behaviour happen.

Booting the emulator to find something `grep` would have found wastes an hour.

## 1. Identify what you have

- `tools/prg_inspect.py FILE` (this repo) parses the GEMDOS header, sweeps packer / protection /
  SNDH signatures and prints disk-image boot-sector facts. Dependency-free Python 3.
- A GEMDOS executable starts with `$601A`; header = magic, TEXT, DATA, BSS, SYMBOL sizes, 3 reserved
  longs/flags, then TEXT at file offset 28. All offsets you later compute are relative to TEXT
  (symbol table and relocation table follow DATA). Getting this wrong makes every finding garbage.
- `.ST` is raw sectors, `.MSA` is RLE-compressed tracks, `.DIM`/`.STX` need their own parsers.
  Sector 0 is the boot sector (`atari-st-bootsector`). Payload may sit in files on the FAT or in
  custom-format tracks with no filesystem at all.

## 2. Packed or protected? Check BEFORE trusting a static read

A crunched file shows no strings and no signatures, and you will wrongly conclude there is nothing.
See `atari-st-packers` (compression: Pack-Ice, Atomik, Automation, ...) and `atari-st-protections`
(encryption / anti-debug: Copylock, Sly Packer, ...). Compression is reversible offline or with the
`depack` tool; protection is usually easiest to defeat by running it and reading RAM after it decrypts.
UPX-style or relocating packers leave nothing useful on disk - scan live RAM (`rip`, `search_memory`).

## 3. Rip the cheap assets

- **Music:** an SNDH has `SNDH` at file offset 12 (after three `bra` entry points init/exit/play) -
  the tune starts 12 bytes before the tag, ends at `HDNS` when present (it often is not: bisect the
  length by playing candidates). Tags (`TITL COMM RIPP CONV YEAR TC ##`) are the truth, filenames are
  not. In a running program use `identify_music`; to name a tune use `tools/sndh_index.py` (name ->
  file) and `db/` (bytes -> name). See `atari-st-ym-players`, `ym2149-psg`.
- **Pictures:** planar, not chunky (`atari-st-bitplanes`). A low-res screen is 32000 bytes plus a
  16-word palette; Degas/NEO have headers (`atari-st-degas`, `atari-st-neochrome`). `find_picture`
  scores candidates in RAM; `view_memory_as_screen` renders any address as a screen.
- **Tables are gold:** sine tables, motion paths, raster lists. Rip the bytes instead of re-deriving
  the curve - the original's table *is* the effect.

## 4. Recover the algorithm

- Find the VBL (`$70`), HBL (`$68`) and Timer handlers, then follow what they write to `$FFFF82xx`
  (`atari-st-hardware-map`). `watch` reports the PC that changed a value; `xrefs` finds readers;
  `cheat_scan` finds a variable with no disassembly at all (snapshot, play, filter changed/eq).
- `m68000-cycles` + `profile_frame`: "this must fit in one scanline / one VBL (160256 cycles at
  50 Hz)" is often the strongest clue to what a routine can possibly be doing.
- `atari-st-blitter` when it touches `$FFFF8A00`; `ym2149-psg` for `$FFFF8800/8802` writes.
- Disk / hard-disk code: `atari-st-acsi-hdd`.

## 5. TOS conventions that explain odd-looking code

- Programs install VBL work through the vbl queue (`_vblqueue` `$456` -> array of `_nvbls` `$454`
  slots, called with JSR so they end in RTS) rather than overwriting `$70`, which would drop TOS's
  keyboard/`_hz_200`/floppy housekeeping. Overwriting `$70` without chaining is a tell of a demo
  that owns the machine.
- Hooks (`hdv_*`, `etv_critic`, trap vectors) are installed by saving the old long and chaining to it.
  A hook that does not chain is either a takeover or a bug.
- Byte-wide registers on odd addresses (`$FF8201/03/0D`, `$FF820F`, `$FF8265`) must be accessed with
  `.b`; a word access there is an Address Error (3 bombs) - useful when diffing a crash.
- `AUTO\` folder PRGs run before GEM, in directory order; a PRG that returns with Pterm lets the next
  one run. That is how to start a PRG under ShirazMCP: put it in `<harddrive>/AUTO/`.
- The resident-program idiom: `Ptermres`, or lowering `phystop`/`memtop`, or keeping a resvector
  (`$426` magic `$31415926`, `$42A`) to survive reset. Virus/protection code abuses the same ones.

## 6. ShirazMCP gotchas

- Pass `work_dir` explicitly if `start` times out on "fifo creation"; pass `tos_path` rather than
  relying on a Hatari config.
- Pass `borders: true` for screens that open the borders: the video registers still say 320x200
  and a cropped screenshot hides the effect.
- `read_memory` returns <= 1024 bytes per call (page with `next_addr`); whole-RAM tools (`rip`,
  `find_picture`) use the full dump.
- `watch` fires on value-change writes only; use `xrefs` for reads. Use the managed `breakpoint`
  and `watch` tools, never raw `debug("b ...")`.
- Disassembler trap: `movem` with a `(d16,PC)` operand can be off by 2 - cross-check the target.
- `save_prg mode=repatch` re-emits the program with your live patches (reloc/BSS patches are not
  persisted); `mode=snapshot` freezes the whole machine.
- See the `shirazmcp` skill for the tool catalogue and `docs/live-patching.md` for a walk-through.

## 7. Write down what you found

Keep scripts, offsets and the packer/protection verdict beside the work (not in a temp dir).
A short analysis note - file source and hash, load addresses, parts and their code addresses, the
tables and their sizes, the cycle budget, the music format/player - saves the next session from
re-deriving it. Record the best-match score when you identify a tune.
