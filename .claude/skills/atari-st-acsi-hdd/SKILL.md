---
name: atari-st-acsi-hdd
description: Atari ST hard-disk interface and driver anatomy - the ACSI bus and DMA chip ($FF8604/$FF8606/$FF8609-$FF860D), FDC-vs-HDC selection, the ACSI command-block sequence and handshake (MFP GPIP bit 5), sector count and DMA address, status/error handling, the Atari root sector and partition table, the AHDI/XHDI conventions, the BIOS hooks hdv_init/hdv_bpb/hdv_rw/hdv_mediach/hdv_boot ($46A/$472/$476/$47E/$47A), and how a driver loads from a boot sector or AUTO folder. Use when reverse-engineering an ST hard-disk driver (AHDI, HDDRIVER, ICD, a bootable HD sector), writing a new one, or watching DMA traffic in Hatari with ShirazMCP.
---

# Atari ST ACSI hard disks and driver anatomy

## Step 0: check the reference files

This skill's tables are summaries. For anything beyond them, read the full third-party
document(s), which are NOT in this repo (licence) and live in your local `refs/` (gitignored):
  - `refs/skills/atari-st-hardware-map/memory_map.txt`

Before relying on them, run:

    python3 tools/fetch_refs.py --check --skill atari-st-acsi-hdd

- `ok`: read the file(s) above.
- `missing` or `mismatch`: run `python3 tools/fetch_refs.py --fetch --skill atari-st-acsi-hdd` (restores
  from this repo's git history or the listed source, verified by sha256). If it still reports
  missing, tell the user which document is missing and where to get it (the script prints the
  sources), and continue from this skill's summary only: never fill in a table value from memory
  and present it as coming from the document.

**Provenance and trust.** Written from public documentation and the author's recollection of it, not
from a licensed copy of the Atari source. Items marked **[UNVERIFIED]** are from memory or conflict
between sources: confirm them against Hatari's source and a real disk trace (see "Verifying with
ShirazMCP") before relying on them in a driver. Nothing here is copied from copyrighted manuals.

## Sources (read these; none are bundled)

- *Atari ACSI/DMA Integration Guide* (Atari Corp., 1991) - the authority on the bus, the DMA chip
  and the command handshake. Hosted on archive sites (search the title; Atari-Forum, atariforge).
- *Atari ST/STe/MSTe/TT/F030 Hardware Register Listing* (Dan Hollis, 1994) - register and bit list
  for `$FF8604/$FF8606/$FF8609..0D` (the copy embedded at `src/analyze/memory_map.txt`).
- TOS.HYP system-variable and BIOS docs (FreeMiNT project, freemint.github.io/tos.hyp): `hdv_*`,
  `_drvbits`, `pun_ptr`, Rwabs flags, BIOS error codes.
- *XHDI specification* (Uwe Seimet, v1.10 and later) - the cross-driver API; mirrored in the
  FreeMiNT sources. **[UNVERIFIED]** exact function numbering below - check the spec.
- Hatari source: `src/hdc.c` (ACSI emulation: which commands it implements), `src/fdc.c` (DMA chip
  registers), `src/ide.c`, `src/scsi.c`. Authoritative for what the emulator accepts.
- SCSI-2 / SCSI-1 command descriptions (SCSI Primary/Block Commands) for opcodes and sense data.
- AHDI 3.x / HDX source is Atari-copyrighted: study it only where you have the right to; this skill
  does not reproduce it.

## 1. The hardware

ACSI (Atari Computer Systems Interface) is a cut-down SCSI-1-like bus: 8-bit data, a command phase
the host drives one byte at a time, up to 8 targets (IDs 0-7, bits 7-5 of the first command byte),
**no arbitration or parity, no reselect**. The ST DMA chip (custom, between the CPU bus and the
bus connector) is shared with the floppy controller (WD1772) and moves data in 16-byte FIFO bursts
at a 24-bit address (DMA reaches ST-RAM only, below 4 MB on STs; TT/Falcon can't DMA into Fast RAM
- bounce through ST-RAM **[UNVERIFIED limits per model]**).

| Address | Size | Name | Use |
|---|---|---|---|
| `$FF8604` | word | DMA data | read/write the register selected by the mode word: FDC reg, HDC byte, or the sector count |
| `$FF8606` | word | DMA mode (W) / status (R) | what `$FF8604` addresses, direction, DMA on/off |
| `$FF8609/0B/0D` | byte | DMA base/counter hi/mid/lo | 24-bit buffer address; reads return the live pointer |
| `$FFFA01` bit 5 | byte | MFP GPIP | FDC/HDC interrupt line, **active low** (0 = device asserting) |
| `$FFFA05` bit 5 | | GPIP direction | input (0) |
| `$43E` `flock` | word | OS | non-zero = DMA owned by someone: the OS VBL floppy code stays off the chip |
| `$4BA` `_hz_200` | long | OS | 200 Hz counter for timeouts |

Mode word (`$FF8606` write) - the constants real code uses (**the bit-7/bit-3 names in the Hollis list
read "0 = HDC / 1 = FDC" for bit 7, which does not match how the constants below are used; treat the
constants as ground truth and the bit labels as [UNVERIFIED]**):

| Value | Meaning |
|---|---|
| `$0080 $0082 $0084 $0086` | FDC command/status, track, sector, data (A1/A0 = bits 2/1) |
| `$0088` | HDC access, A0 = 0 (**start of a command byte sequence / first byte**) |
| `$008A` | HDC access, A0 = 1 (further command bytes; reading the status byte) |
| `$0090` / `$0190` | select the **sector count** register, DMA read / write |
| bit 8 (`$100`) | direction: 0 = DMA reads from the device into RAM, 1 = RAM to the device |
| bit 6 (`$40`) | 1 = DMA off (status-only access) |
| `$0000`-style | DMA on for the data phase, direction by bit 8 |

Status read (`$FF8606`): bit 0 = DMA error line (**the polarity is [UNVERIFIED]: Atari's guide says 0 =
error, the Hollis list says 1 = error - test it**), bit 1 = sector count non-zero, bit 2 = data
request. Toggling bit 8 (write one value, then the opposite, then the wanted one) **resets the DMA
FIFO and status** - always do it before programming a transfer.

## 2. One ACSI read/write, step by step

Group-0 (6-byte) commands. Interrupts that touch the DMA chip or `flock` must be quiet.

1. `flock` <- non-zero (`st.w $43E`) so the OS floppy VBL code leaves the chip alone; restore after.
2. **DMA address:** write the three bytes **low byte first** (`$FF860D`, then `$FF860B`, then
   `$FF8609`). Address even; buffer in ST-RAM. **[UNVERIFIED: low-first ordering requirement]**
3. **FIFO reset + sector count:** toggle bit 8 so the final state is the direction you want: for a
   read write `$0190` then `$0090`; for a write `$0090` then `$0190`. Then, with the sector-count
   register selected (bit 4), write the count (number of 512-byte blocks, 1-255) to `$FF8604`.
4. **Command bytes** (6): mode `$0088` (or `$0188` for a write) is the starting mode.
   - First byte = `(target << 5) | opcode`, written with A0 = 0.
   - The idiom `move.l #((byte << 16) | $008A),$FF8604` writes the data word to `$FF8604` and then
     switches the mode to A0 = 1 for the next byte in one instruction (a long write hits both
     registers: high word to `$FF8604`, low word to `$FF8606`).
   - After **each** byte except the last, wait for the device to pulse its interrupt (GPIP bit 5
     low) before the next. Poll `btst #5,$FFFA01` with a `_hz_200` timeout (a few ms for command
     bytes **[UNVERIFIED value]**; seconds for a spin-up). The last byte is written with the mode
     already set for the data phase (DMA on, direction bit 8), and the DMA starts when the device
     raises DRQ.
5. **Wait for completion:** the device asserts the interrupt line again when it has finished
   (typically up to a few hundred ms for reads, seconds with spin-up; AHDI-like drivers use ~1-3 s
   **[UNVERIFIED]**). Time out via `_hz_200` and fail with E_READF/E_WRITF.
6. **Status byte:** write `$008A` (A0 = 1, HDC), read the word at `$FF8604`, take the low byte:
   `$00` = good, `$02` = check condition (follow with REQUEST SENSE, opcode `$03`, 4 bytes, no
   sector count). Then restore `flock`.
7. Optionally verify the DMA counter (`$FF8609..0D`) advanced by `count * 512`.

Command block (group 0): byte 0 = `(target<<5)|opcode`; byte 1 = `(LUN<<5)` | LBA bits 20-16;
bytes 2-3 = LBA bits 15-0; byte 4 = block count (0 means 256 for SCSI-1 read/write);
byte 5 = control/vendor byte (0).

| Opcode | Command | Note |
|---|---|---|
| `$00` | TEST UNIT READY | no data, status only |
| `$01` | REZERO/RESTORE | |
| `$03` | REQUEST SENSE | 4+ byte data phase |
| `$08` / `$0A` | READ(6) / WRITE(6) | 21-bit LBA = max 1 GiB |
| `$12` | INQUIRY | identifies the device (vendor/product) |
| `$15` / `$1A` | MODE SELECT / SENSE | |
| `$1B` | START/STOP UNIT | |
| `$1F` | **ICD extension** | after byte 0, the *real* SCSI opcode follows (10-byte commands: `$25` READ CAPACITY, `$28`/`$2A` READ/WRITE(10)); used by ICD adapters for > 1 GiB / SCSI devices. **[UNVERIFIED details]** |

Capacity: READ CAPACITY(10) (via `$1F`) returns last LBA and block size, big-endian. ICD/SCSI
bridge hardware varies: some ACSI-to-SCSI bridges implement only a subset.

## 3. Partitions: the Atari root sector (AHDI convention)

Physical sector 0 of a disk holds boot code (optional) and the partition table:

| Offset | Size | Field |
|---|---|---|
| `$000` | up to `$1C1` | boot code area (optional) **[UNVERIFIED exact extent]** |
| `$1C2` | long | `hd_siz` - disk size in sectors |
| `$1C6` | 4 x 12 | partition entries: byte flag (bit 0 = exists, bit 7 = bootable **[UNVERIFIED]**), 3-char ID, long start sector, long size |
| `$1F6` | long x 2 | `bsl_st`, `bsl_cnt` bad-sector list |
| `$1FE` | word | checksum: 256 words sum to `$1234` if the sector is bootable (`atari-st-bootsector`) |

Partition IDs: `GEM` (<= 16 MB, FAT16 with 512-byte logical sectors), `BGM` (big, up to 512 MB
via larger logical sectors - needs TOS 1.04+ or AHDI >= 3), `XGM` (extended partition: another root-
sector-format table at the partition start, chained), `RAW`, `F32`/`LNX`/`MIX` etc. from other
drivers **[UNVERIFIED list]**. Numbers are big-endian (68000), unlike the little-endian FAT BPB
inside each partition's boot sector.

GEMDOS drive letters: floppies are A:/B:; hard-disk partitions take C: upward, one bit each in
`_drvbits` (`$4C2`, bit 0 = A:). The driver sets these bits after reading the root sectors.

## 4. How a driver plugs into TOS

TOS does not talk to a hard disk itself (TOS 1.00 has no ACSI boot at all; later ROMs scan ACSI
targets 0-7 for a bootable root sector **[UNVERIFIED which TOS versions and in what order versus the
floppy]**). Instead it calls through low-memory vectors that a driver replaces. Floppy devices are
BIOS devices 0 and 1; devices >= 2 go to the hard-disk vectors:

| Address | Vector | Called as (C calling convention, args on stack, result in d0) |
|---|---|---|
| `$46A` | `hdv_init` | once at boot; the ROM's own routine is a stub for a missing disk |
| `$472` | `hdv_bpb` | `bpb *(WORD dev)` - return a pointer to the 9-word BPB (`recsiz, clsiz, clsizb, rdlen, fsiz, fatrec, datrec, numcl, bflags`), or 0 |
| `$476` | `hdv_rw` | `LONG (WORD rwflag, void *buf, WORD count, WORD recno, WORD dev[, LONG lrecno])` - `rwflag` bit 0 = write, bit 1 = do not touch media-change state, bit 2 = no retries, bit 3 = physical (ignore partition) **[UNVERIFIED bit 1-3 meaning]**; if `recno == -1` use the long `lrecno`. Returns 0 or a negative BIOS error |
| `$47A` | `hdv_boot` | called to boot from the hard disk; loads sector 0 to `_dskbufp`, returns 0 on success **[UNVERIFIED]** |
| `$47E` | `hdv_mediach` | `LONG (WORD dev)` - 0 unchanged, 1 maybe, 2 changed |
| `$516` | `pun_ptr` | `PUN_INFO` for AHDI: `puns` word, `pun[16]` (target number, bit 7 = absent), `partition_start[16]` longs, `P_cookie` `'AHDI'`, `P_cookptr`, `P_version`, `P_max_sector`, reserved **[UNVERIFIED layout]** |
| `$5A0` | `_p_cookies` | cookie jar; XHDI-capable drivers add an `XHDI` cookie pointing at a dispatcher |
| `$4C2` `$4C6` | `_drvbits` `_dskbufp` | drive bit map; 1 KB scratch sector buffer |

Rwabs/BIOS errors: `0` OK, `-1` general, `-2` drive not ready, `-4` CRC, `-5` bad request, `-7`
unknown media, `-8` sector not found, `-10` write fault, `-11` read fault, `-13` write-protect,
`-14` media changed, `-15` unknown device, `-16` bad sectors on format.

A driver chains the old vector (save the old long, call it for devices you do not own, in the
same style as VBL/trap hooks; `st-reverse-engineering` section 5) and returns the BIOS error
code, not an ACSI status. Typical logical flow of `hdv_rw` for a partitioned disk: map
`dev` -> (target, partition start) from `pun`; `lsector = partition_start + recno`; split into <=
255-sector ACSI transfers; per chunk run section 2; map failures to `-11/-10`.

XHDI (cookie `XHDI`) is the portable replacement for poking `pun_ptr`: one dispatcher, function
word on the stack - `0` GetVersion, `1` InqTarget, `2` Reserve, `3` Lock, `4` Stop, `5` Eject,
`6` DrvMap, `7` InqDev, `8` InqDriver, `9` NewCookie, `10` ReadWrite, `11` InqTarget2, `12` InqDev2,
`13` DriverSpecial, `14` GetCapacity, `15` MediumChanged **[UNVERIFIED numbering and the magic
`$27011992` before the dispatch address]**. Implement at least GetVersion/InqTarget/ReadWrite/
GetCapacity if you want MiNT or HDX-style tools to see your disk.

## 5. Getting the driver running

Options, from simplest to most intrusive:

- **AUTO folder (floppy):** a PRG in `AUTO\` that installs hooks, reads the root sector, fills
  `_drvbits`, and stays resident (`Ptermres`). This is how AHDI/HDDRIVER start; easiest to debug.
- **Hard-disk boot sector:** the ROM loads root sector 0, checks the `$1234` sum, jumps in. The
  code must be tiny (the root sector minus the table, under 450 bytes) and loads the real driver from reserved sectors (a driver
  area after the root sector), because it can use ACSI before any hook exists. It then relocates
  the driver, lowers `phystop`/`_memtop` or `Malloc`s it (so GEMDOS does not reuse the memory),
  installs hooks, builds `_drvbits`, and returns so TOS finishes its boot (AUTO folder, desktop).
- **Reset-proof:** keep a resvector (`$426` = `$31415926`, `$42A`) to reinstall after warm reset.

Fixed rules for any driver: save/restore SR and registers, use `flock`, never leave the DMA chip
in a state that confuses the floppy driver (set mode back to FDC, e.g. write `$0080`), keep the
hook idempotent across warm resets, and handle media-change (`hdv_mediach`) for removable media.

## 6. Verifying with ShirazMCP

Hatari emulates ACSI hard disk images (its `--acsi` option, and IDE/SCSI on other models) and
`src/hdc.c` implements the commands above, so a driver can be tested without hardware.
ShirazMCP's `start` exposes a GEMDOS `harddrive` directory and floppy images; for ACSI/IDE images
use Hatari's own config (`hatari.cfg` / `--acsi`) **[UNVERIFIED that `start` can pass it - use
`debug` and the config for now]**. Then:

- `watch` `$FF8606`/`$FF8604` (value-change writes report the writing PC) to find the driver's
  command routine in a program you are reverse-engineering; `xrefs` on `$FF8606` lists readers
  and writers (the hardware map annotates these addresses).
- Breakpoint on `$476`/`$472`: `read_memory $476 4` to see whether a driver replaced the ROM
  vector, `disasm` the new target, then follow the stack arguments (`registers`).
- `bootsector(source=ram)` on the root sector in `_dskbufp` (`$4C6`): `$1234` checksum and the
  loader/virus heuristics.
- `machine_state` / `registers` after a `run_frames` to see whether the poll loop is spinning on
  `btst #5,$FFFA01` (timeout path) or progressing; `profile` the poll loop to see its cost.
- A DMA that "hangs" is usually one of: missing FIFO toggle, wrong sector-count mode, GPIP bit
  polarity inverted (it is **active low**), timeout too short, or `flock` not set so a VBL floppy
  routine touched the chip.

## Open questions to settle before shipping a driver

Mark each with a real trace: address-write order, the exact status polarity, command-byte timeouts,
the root-sector boot-code size, TOS versions that boot ACSI, XHDI numbering, `hdv_rw` flag bits,
`pun_info` layout, the `$1F` escape details, and Fast-RAM bounce rules on TT/Falcon.
