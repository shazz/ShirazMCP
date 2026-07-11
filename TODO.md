# ShirazMCP — roadmap

**Open work is tracked in [GitHub Issues](https://github.com/shazz/ShirazMCP/issues).**
This file is just a pointer + a record of what's shipped.

## Open (see Issues)

- [#1 Windows support via a platform transport adapter](https://github.com/shazz/ShirazMCP/issues/1) — the `ControlTransport` refactor + hrdb-fork RDB transport (spec: `docs/plan/P23.md`)
- [#2 Tune find_picture confidence thresholds against a live rip](https://github.com/shazz/ShirazMCP/issues/2)
- [#3 Player recognition: expand the YM driver fingerprint DB](https://github.com/shazz/ShirazMCP/issues/3) — name the *engine*
- [#12 Tune identification: content-fingerprint RAM against the SNDH archive (Shazam-for-SNDH)](https://github.com/shazz/ShirazMCP/issues/12) — name the *tune*
- [#4 Depacker long tail (Cranker + others)](https://github.com/shazz/ShirazMCP/issues/4)
- [#5 VS Code frontend — visual debugger over the MCP](https://github.com/shazz/ShirazMCP/issues/5)
- [#6 Headless CI for the live suites (Xvfb)](https://github.com/shazz/ShirazMCP/issues/6)
- [#7 Real deprotectors](https://github.com/shazz/ShirazMCP/issues/7)
- [#8 Disk-image containers: .MSA / .DIM](https://github.com/shazz/ShirazMCP/issues/8)
- [#9 Sprite / font ripper](https://github.com/shazz/ShirazMCP/issues/9)
- [#10 Audio structure analysis](https://github.com/shazz/ShirazMCP/issues/10)
- [#11 Power-ripper code-API mode](https://github.com/shazz/ShirazMCP/issues/11)

## Shipped

Core (P0–P10): session/FIFO/memory/debug, Capstone disasm + 68000 cycles + Hatari
profiler + managed breakpoints, ripper engine, graphics (Degas/NeoChrome→PNG),
Pack-Ice depacker, music/SNDH+YM, boot sector, assemble/patch (vasm), `machine_state`.

- **P11** spectrogram + record_audio · **P12** ZX0 depacker · **P13** MPP picture
- **P14** watch / xrefs / run_frames · **P15** cheat_scan · **P16** save_prg (repatch)
- **P17** ym_state · **P18** STE bus-contention cycle model · **P19** YM fingerprint DB
- **P20** rip_sample (STE DMA-PCM → WAV) · **P21** find_picture structure-scoring
- **P22** save_prg mode=snapshot (freeze the machine to a self-restoring .PRG)

Per-feature specs in `docs/plan/*.md`; decisions in `decisions.md`.
