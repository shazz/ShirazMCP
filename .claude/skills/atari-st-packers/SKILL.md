---
name: atari-st-packers
description: Atari ST executable/data packers and crunchers (Pack-Ice/ICE, Atomik, Automation, Speed Packer, and others) — magic signatures, how to detect packed regions, and depacking notes. Use when detecting compressed data in ST memory/files, building the Depacker plugin registry, or recognizing why a region looks like garbage until depacked.
---

# Atari ST packers / depackers / crunchers

Demos and games routinely ship packed. A packed region reads as high-entropy
garbage to the picture/music detectors — so the ripper must **detect → depack →
re-scan** (this is exactly the `Depacker` plugin trait + recursive engine).

## Documented magic signatures

| Packer | Magic / signature | Notes |
|--------|-------------------|-------|
| **Pack-Ice (ICE)** | `Ice!` / `ICE!` (ASCII `49 63 65 21` / `49 43 45 21`) | The most common. Versions 2.11, 2.12, 2.31. Well-documented LZ+Huffman-ish scheme; many public depackers. |
| **Atomik** | `ATOM` | Versions 3.0, 3.5. |

Prior-art depacker covering both: `Depack D` / `ADP` handle ICE (2.11/2.12/2.31)
and Atomik (3.0/3.5).

## Other packers catalogued (signatures not in the wiki source)

`Automation`, `CDC`, `DEK`, `IMPLODER`, `IDRAGON`, `GOLLUM`, `Jampacker`, `LSD`,
`MPACK`, `PACK/UNPACK`, `PFXPAK`, `SPACKER`, `THUNDER`, `UPX`.

> ⚠️ Most of these have **no documented magic** in the atari-wiki source. Identify
> them structurally (entropy + known depacker-stub code patterns) or by porting a
> known depacker and trying it. Seed the DB from ICE and Atomik first — they cover
> the vast majority of scene prods.

## Detection strategy

1. **Magic scan** — `search_memory("Ice!")`, `"ICE!"`, `"ATOM"`. Cheap, exact.
2. **Self-depacking executables** — many prods are a small depacker stub +
   compressed body. The stub relocates and unpacks at load; in a *running* Hatari
   session the memory is **already depacked**, so scan live RAM directly. Packing
   mainly matters for on-disk `.PRG`/`.TOS` files and embedded sub-blocks.
3. **Entropy heuristic** — a region that fails all format detectors but has
   near-uniform byte distribution is a depack candidate; try each `Depacker`.

## Depacker plugin contract

Each packer = one `Depacker` implementation: `try_unpack(&[u8]) -> Option<Vec<u8>>`.
On success the engine re-runs all detectors over the unpacked buffer (bounded
recursion) — so "ICE-packed screen inside a packed demo file" resolves to a PI1
`Finding` automatically.

## Reference implementations (third-party, not bundled)

Real, citeable source to port `Depacker` plugins from. Paths relative to the root of a local clone of the public Atari_ST_Sources collection (https://github.com/ggnkua/Atari_ST_Sources; clone it yourself — it is NOT bundled and each file keeps its own author's licence):

- **`C/Toyoyo/stvn/deps/pack-ice/`** — cleanest **portable C** Pack-Ice: both pack
  (`ice_crunch.c`) and depack (`ice_decrunch.c`, handles both header variants), with
  a documented API (`is_ice_data`, `ice_crunched_length`, `ice_decrunched_length`).
  **Port this first** for the `pack-ice` depacker.
- **`C/Mike Watson (Xerud)/NEW_DPAK/DEPACKER.S/`** — the canonical "one clean 68k-asm
  routine per scheme" set: `ICE1_1/ICE2_0/ICE2_2/ICE2_3_4.S` (all Pack-Ice),
  `ATM3_1/3_3/3_5.S` (Atomik), `AUTO.S` (Automation), `SPEED2/3.S`, `PFX.S` (PFXPAK),
  `JAM_LZH/JAM_LZW.S` (Jampacker), `THUNDER1/2.S`, `PP2.S` (PowerPacker),
  `RNC_*.S`, `GOL_HUFF.S` (Huffman). Cite this for the long-tail packers.
- **`C/Mike Watson (Xerud)/NEW_DPAK/DEPACK.C`** — the **magic-detection dispatcher**
  (ISDATA/ISPROG macros) → model for the registry's detect-and-route logic.
- **`C/Reservoir Gods/PURECLIB/DEPACK_S.S`** (+ `DEPACK.H`) — small unified library
  dispatching on `'ICE!'` / `'ATM5'` magics; clean API shape (`rldp_decrunch`).
- **`GFA Basic/Lonny Pursell/ice24.gfa/ice_24_o.s`** — well-commented Pack-Ice 2.4,
  documented calling convention, 68020+/ColdFire-safe.
- **`ASM/Electronic Images/routines.s/depack.s/`** — `depice.s` (ICE),
  `atom2_33.s` + `depack35.s` (Atomik 2.33 / 3.5), heavily commented.

## Notes

- Pack-Ice depacking is the priority implementation (public reference code exists;
  the ICE format is thoroughly reverse-engineered — see Toyoyo lib above).
- Distinguish a *packer* (whole-file cruncher) from a *format's own RLE* (Degas PC1,
  NEO has none) — the latter is handled inside the picture plugin, not a `Depacker`.
- Sources: https://atari-wiki.com/index.php?title=Packer/Depacker plus the archive
  references above.
