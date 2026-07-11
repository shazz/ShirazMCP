---
name: atari-st-bootsector
description: Atari ST floppy boot sector format — the 512-byte layout, the DOS BPB (BIOS Parameter Block, little-endian), the $1234 executability checksum, boot-loader vs virus code, and known bootsector-virus / antivirus context. Use when extracting or analyzing boot sectors from disk images (.ST/.MSA/.DIM) or RAM, detecting bootable/virus sectors, or building the bootsector extractor plugin.
---

# Atari ST boot sector

The first sector (logical sector 0, 512 bytes) of an ST floppy. TOS loads it at
boot and, **if it is "executable," runs it** — which is exactly why it was the
dominant virus vector and the target of every ST antivirus (Ultimate Virus Killer,
Sagrotan, VKILLER). Legitimate uses of this plugin: malware analysis / preservation
of legacy floppy viruses, antivirus signature work, and custom boot-loader study.

## Executability rule (the $1234 checksum)

A boot sector is executable **iff the sum of all 256 big-endian 16-bit words equals
`$1234` (mod 65536)**. The last word (`$1FE`) is the checksum/adjustment word chosen
to make the total come out to `$1234`.

```
sum = 0
for i in 0..256: sum = (sum + be16(sector, i*2)) & 0xFFFF   # big-endian words
executable = (sum == 0x1234)
```

If executable, TOS executes the sector from offset `$000` (a `BRA.S` there skips the
BPB to the real code). Non-executable = pure data/BPB only (normal blank disk).

## Layout (512 bytes)

| Offset | Size | Field | Notes |
|--------|------|-------|-------|
| `$000` | 2 | Branch (`BRA.S`, `$60xx`) to boot code | present on bootable sectors |
| `$002` | 6 | OEM / loader string | e.g. `"Loader"` |
| `$008` | 3 | Disk serial number | |
| `$00B` | — | **BPB begins** (DOS, little-endian!) | see below |
| `$01E` | var | Boot loader code | 68k code; runs if checksum = `$1234` |
| `$1FE` | 2 | Checksum word (big-endian) | tunes total to `$1234` |

### BPB — BIOS Parameter Block (⚠️ little-endian / Intel order)

The FAT BPB follows the MS-DOS convention and is stored **little-endian**, even
though the 68000 is big-endian and the checksum uses big-endian words. Read these
byte-swapped:

| Offset | Size | Field | Typical |
|--------|------|-------|---------|
| `$00B` | word | `bps` bytes per sector | 512 |
| `$00D` | byte | `spc` sectors per cluster | 2 |
| `$00E` | word | `res` reserved sectors | 1 |
| `$010` | byte | `nfats` number of FATs | 2 |
| `$011` | word | `ndirs` root dir entries | 112 |
| `$013` | word | `nsects` total sectors | 720 / 1440 |
| `$015` | byte | `media` media descriptor | `$F8`/`$F9`… |
| `$016` | word | `spf` sectors per FAT | 5 |
| `$018` | word | `spt` sectors per track | 9 |
| `$01A` | word | `nsides` sides/heads | 1 / 2 |
| `$01C` | word | `nhid` hidden sectors | 0 |

## Analysis / detection

For an extracted sector, the plugin should report:

1. **Executable?** ($1234 checksum) — the primary flag.
2. **Decoded BPB** → disk geometry (sanity-check against actual image size).
3. **Disassembly of the boot code** (embedded Capstone M68K from `$000`/`$01E`),
   annotated via `atari-st-hardware-map`.
4. **Suspicious-behavior heuristics** (virus tells): installs into system/reset
   vectors (`$004` resvector, `$088`/`$08C` timers), hooks BIOS/XBIOS/GEMDOS via
   `Setexc`, `Getbpb`, or `hdv_*`/`bios` vectors, copies itself, or writes to disk
   during boot. A legit loader loads/executes a program; a virus persists + spreads.
5. **Known-virus fingerprint match** — signature DB seeded from ST antivirus data
   (UVK's catalogue covered the well-known families: Signum, Ghost, BPL/"Key",
   Freeze, Mad/"C't", Milzbrand, etc.).

## Sources of boot sectors for the plugin

- **Disk image files**: `.ST` (raw), `.MSA` (Magic Shadow Archiver), `.DIM`,
  `.STX/Pasti` — sector 0 is the boot sector. `.ST` is trivial (first 512 bytes).
- **Live RAM**: TOS keeps the loaded boot sector around at boot; or read via the
  running session.

## Plugin shape

A `RipPlugin` of kind `Boot`:

```
bootsector_scan(sector: &[u8;512]) -> BootFinding {
  executable, checksum, bpb, geometry_ok,
  disasm, suspicious: Vec<Tell>, virus_match: Option<String>, confidence
}
```

Add to the depacker/format registry like any other plugin — one file, one line.

## Reference implementations (in `../Atari_ST_Sources`)

Real ST boot-sector code to model the plugin's decode/heuristics on (paths relative
to `$HOME/projects/Atari_ST_Sources`):

- **`ASM/Adrenalin U.K (ADR)/BOOT_BLK/ZR_BLK.S`** — a *"ZEN REPUBLIC Virus Free Boot
  Sector"*: real bootblock code that **checks the reset vector (`$426`) and scans
  for known virus signatures** (`cmp.l #$12123456`) — i.e. an antivirus *inside* a
  boot sector. Gold for the suspicious-behavior/virus-tell heuristics. (`Z_R_BOOT.S`
  is the installer.)
- **`ASM/Adrenalin U.K (ADR)/ADR_BOOT.1_1/INSTALL.S`** — the **checksum installer**:
  writes the `$1234` adjustment word so the sector becomes executable. Model for
  computing/patching the executability checksum.
- **`ASM/Adrenalin U.K (ADR)/ADR_BOOT.1_1/ADR_BOOT.S`** + `ADR_1_3.S` — a full custom
  boot loader (loads/launches the diskmag), with the same reset-vector virus checks.

These show both sides: a legitimate loader and the antivirus checks a defender
bootblock performs — exactly the classification the plugin automates.

### `r1pp3r` — the reference boot-cracktro ripper ⭐

**`refs/orionfuzion/r1pp3r/src/R1PP3R.S`** (68k asm, cloned) is the closest existing
tool to what this plugin + the ripper engine should do together. It rips **boot
cracktros** — cracktros stored directly on disk sectors (no filesystem) and launched
by the bootsector — and saves them as runnable `.PRG`. Technique to learn from:

1. Creates a **sandbox** that executes the bootsector safely in low user memory *as
   if at boot time* (no GEM).
2. **Hooks the XBios vector** to monitor disk access and follow the sectors the
   bootsector loads.
3. Reassembles the loaded cracktro and wraps it as a standalone GEM `.PRG`.

Built for archival (Demozoo). This is the blueprint for our "freeze → follow loads →
extract → save as PRG" flow. `prebuilt/R1PP3R.PRG` runs it directly under Hatari.

