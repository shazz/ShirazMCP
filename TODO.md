# ShirazMCP — TODO / future work

Tracked ideas and follow-ups. Core is feature-complete (P0–P20); everything still
open below is optional/next.

## ✅ Done since the core (P11–P20) — was TODO, now shipped
- **P11** `spectrogram` + `record_audio` (hound + rustfft).
- **P12 ZX0 depacker** — headerless (structural auto-detect + explicit `depack` offset).
- **P13 MPP** multi-palette picture RipPlugin + renderer.
- **P14** `watch` (value-change write-watch; reads → `xrefs`) · `xrefs` · `run_frames(n)`
  — the RE primitives the live-patch exercise wished for (memory watchpoints / data
  cross-refs / deterministic frame stepping). Read-watches aren't possible (Hatari has no
  bus-access breakpoints) → the write-watch + `xrefs` split covers it.
- **P15** `cheat_scan(new|filter|list|reset)` — Action-Replay memory-diff search (ADR-29).
- **P16** `save_prg` (repatch mode) — persist a live hack by mapping patched TEXT/DATA RAM
  back to the original `.PRG` file offsets. **(The snapshot-to-runnable-executable flavour
  is still open — see below.)**
- **P17** `ym_state` — non-intrusive live YM2149 decode ("what the chip is playing now")
  via Hatari `info ym` (ADR-33). From STEIsNotDead `tools/audio/ym2149.py`.
- **P18** STE bus-contention model (ADR-34, commit 8bb0438) — sharpens the static cycle
  estimate; splits each insn into bus vs internal cycles, scales the bus share by display
  region. Ported/refined from `ste_model.py`.
- **P19** YM player fingerprint DB (ADR-35, commit 5156c40) — 9 signatures pinned from real
  local sources; invented seeds dropped. **(Full SNDH-archive tuning still open below.)**
- **P20** `rip_sample` — STE DMA-PCM sample ripper (ADR-36, commit 549d8c7) — $FF8900 block →
  PCM16 WAV consumable by `spectrogram`. Live-verified: ripped 123s of real DHS Uhane DMA
  audio @12517Hz, 99% non-silent. Tool count 34→35.
- **Docs:** `DEVELOPMENT.md` (commit b08372f) — build + how to drive the MCP in dev (Python
  stdio harness during active dev vs `claude mcp add` once stable). Redux live-patch writeup
  in `docs/live-patching.md` (commit 63b2a45).
- **3 tool-surfaced bugs fixed:** `start` ignored overrides (07b450e); `pause()` sentinel
  echo race (021d109, ADR-11 addendum); `write_memory` >61-byte truncation (e703d98, ADR-22).

## ✅ Demo delivered — "Blade Runners" redux hot-patch (2026-07-11)
All three patches verified live AND persisted in a standalone `.PRG` (booted with the
original absent): logo **row-swap** RUNNERS-over-BLADE (`$00E08E`, bitmap-band exchange),
**scroller** → "SHIRAZMCP…" (`$02D640`, 1632-byte buffer), **gradient** → green→magenta
(`raster_table` at `$00031EFC`, found via the demo's own symbol table). Run-ready artifact:
`$HOME/projects/MJJ/hacked/STRGOOSE_hacked.PRG` + auto-boot HD `$HOME/projects/MJJ/hacked/hd/`.

---

## 🔎 Still open — RE primitives
- **⭐ Snapshot-to-runnable executable** — the second `save_prg` flavour: dump full RAM +
  regs + hardware and wrap with a restore stub → a self-contained `.PRG` that resumes the
  exact frozen+patched instant (the Ultimate Ripper / `refs/orionfuzion/r1pp3r` model).
  Captures BSS/runtime state that repatch-mode can't. We already have `savebin` (RAM), the
  `.PRG` wrapper in `extract(as=prg)`, and the r1pp3r blueprint — very buildable.
- **P21 threshold verification** — re-run the Uhane rip live (single-Hatari constraint: wait
  for P20) and tune the four `// verify live` thresholds in `image/score.rs`; acceptance:
  ≤8 ranked picture findings, real screens on top, whole-RAM rip < 1 s.

## 🎵 YM fingerprint DB — tune against the full SNDH archive
P19 pinned what's confidently derivable from LOCAL sources and stated the gaps honestly
(Lucas/Quartet, Whittaker/Delta/Music Mon, unlabelled rips). The [SNDH archive](https://sndh.atari.org)
is out of reach locally; when available, expand the DB (one `ym_sig_db.rs` entry + one
corpus test each). Also: real rips use 24-bit `lea $FF8800.l`, which `is_psg` currently
misses (flagged in P19) — small engine fix worth doing.

## 🔌 Depacker long tail
Repeat the P9 shape (one `Depacker` file + one registry line each). **Cranker** (BIFAT/TEK,
DHS's packer) is the notable miss. Others in `../Atari_ST_Sources/C/Mike Watson (Xerud)/
NEW_DPAK/DEPACKER.S/`: Automation (`AUTO.S`), Speed Packer (`SPEED2/3.S`), PFXPAK (`PFX.S`),
Jampacker (`JAM_LZH/LZW.S`), Thunder (`THUNDER1/2.S`).

## 🖥️ VS Code extension — a visual debugger over the MCP
**Vision:** a shared human + AI debugging surface — the agent rips/patches/debugs via MCP
while a human watches (and drives) in VS Code.

- **Option A — Webview panel (quick v1):** connect to `shiraz-mcp`, poll `machine_state` on a
  timer, render registers/palette/screen-PNG + `pause`/`resume`/`disasm`/`patch` buttons.
  ~Weekend project; `machine_state` was practically designed for this.
- **Option B — DAP adapter (idiomatic, more payoff):** a Debug Adapter Protocol adapter →
  VS Code's native debugger UI for free (breakpoints, registers, disassembly view, memory
  inspector, step). Our tools map ~1:1 onto DAP.

Both are thin frontends over the same Rust core. Consider extracting a small `core` crate
(session/debug/analyze) if the split gets real. **Next step:** pick A vs B, draft
`docs/plan/vscode.md`. (Gotcha: single Hatari session — human panel + agent share one machine.)

## 🤖 CI / headless
`Xvfb` isn't configured — live tests need a real X display (`DISPLAY`). Set up a headless X
server so the `#[ignore]`d live suites (incl. the new `rip_sample_live`) can run in CI.

## 🛡️ Real deprotectors
The `Deprotect` seam exists (interface-only `NoopDeprotect`, ADR-6). Options: wrap the
prebuilt `refs/orionfuzion/dec0de` binary, or port specific unprotectors (Rob Northen
Copylock, Sly Packer, Anti-bitos, Toxic Packer…). Often moot in a live session (programs
self-decrypt in RAM). See `atari-st-protections`.

## 💾 Disk-image containers
Only raw `.ST` is supported (bootsector `source=disk`). Add `.MSA` (Magic Shadow Archiver)
and `.DIM` decoders.

## 🎨 Sprite / font ripper
`RipKind::Sprite` is reserved but has no plugin — a sprite/font ripper (N planes × 16px word
columns) is a natural addition.

## 🎼 Audio structure analysis (optional)
From `STEIsNotDead/tools/audio/analyze.py`: energy/band-energy envelope, Foote novelty
(section boundaries), self-similarity matrix — "see a track's structure." Bigger DSP effort,
more music-production than debugging. Optional/later.

## 🧰 Power-ripper "code API" mode
Anthropic's code-execution-with-MCP pattern: expose the ripper as a scriptable API so an
agent writes one "scan every offset, score, return top-N" call instead of many round-trips.
Noted in `docs/BRAINSTORM.md`.

## 🧹 Housekeeping / known debt
- `src/tools/lifecycle.rs` at 207 non-test lines and `src/tools/mod.rs` at 209 — both over
  the 200-line rule (pre-existing; P18 nudged `mod.rs` +2 via a spec-mandated one-liner).
  Split when next touched.
- Not pushed to any remote yet (shazz to create the GitHub repo).
