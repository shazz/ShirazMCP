# Architecture Decision Records — ShirazMCP

## 2026-07-08 — ADR-1: Rust (rmcp + tokio) over Python for the MCP server

**Status:** accepted
**Context:** The server has two very different paths: an I/O-bound
orchestration path (Hatari process, fifo, log tailing) and a compute-bound
ripping path (repeated full-RAM sweeps over ≤4 MB, rendering/scoring every
candidate screen offset, brute-force fingerprinting).
**Decision:** Rust, `rmcp` 2.2 + `tokio`. Honest framing: Rust buys nothing on
the fifo path (bounded by Hatari's round-trip), but the ripping path is
compute-bound, SIMD-friendly and embarrassingly parallel — `rayon` + `memchr`
make interactive "scrub memory as pictures" feasible. Also: single static
binary, and deliberate experimentation. `thiserror` in library layers,
`anyhow` only at the app boundary.
**Alternatives considered:** Python (FastMCP) — faster to write, team knows
it, but interpreted scans over 4 MB per sweep are an order of magnitude too
slow for the interactive ripper, and would push us to native extensions anyway.
**Consequences:** Higher up-front cost per module; libcapstone C dependency
(ADR-4) is easy to consume from Rust; nobody may later claim Rust made the
*debugging* path faster — it didn't and wasn't meant to.

## 2026-07-08 — ADR-2: Plugin-based ripping engine (RipPlugin / Depacker / Registry)

**Status:** accepted
**Context:** The ripper must not be a fixed set of hardcoded scanners; the
whole point (per the Ultimate Ripper heritage) is that new formats keep
arriving — pictures, players, packers, boot sectors.
**Decision:** Two small object-safe traits carry the entire extension
contract: `RipPlugin` (detector: `scan(&MemImage) -> Vec<Finding>`) and
`Depacker` (transformer: `try_unpack(&[u8]) -> Option<Vec<u8>>`). A `Registry`
runs detectors in parallel (`rayon`), tries depackers, and **re-runs detectors
over every successfully depacked buffer** (bounded recursion depth) — giving
"find packer → depack → find the PI1 inside" for free. Invariant: **adding a
format = one plugin file + one registry line**; a phase that forces an engine
edit is an architecture bug. The engine is pure over `MemImage` — no emulator
dependency, fully unit-testable on fixture buffers, synchronous (no async in
compute).
**Alternatives considered:** (a) Hardcoded scanner functions — simplest, but
every format touches the engine; (b) dynamic loading (`libloading`/WASM) —
real extensibility we don't need; static registration is enough for an
in-tree plugin set and keeps the build trivial.
**Consequences:** `dyn` dispatch at the registry boundary (negligible: cost is
in the scans, which monomorphize internally); recursion depth must stay
bounded to avoid depack bombs; `Finding` provenance (`via: [...]`) must be
threaded through recursion.

## 2026-07-08 — ADR-3: Type-state Session FSM

**Status:** accepted
**Context:** The Hatari session has a strict lifecycle
(Stopped→Booting→Running↔Paused) and misuse (ripping a stopped session,
double-start) was a live failure mode in the bash workflow this replaces.
**Decision:** Model the lifecycle with the type-state pattern:
`Session<Stopped>` / `Session<Running>` / `Session<Paused>`, transitions
consume `self` and return the next state. Invalid transitions are **compile
errors** — `Session<Stopped>` has no `pause()`. Because MCP tools need runtime
dispatch, the tool layer holds an `enum AnySession { Stopped(..), Running(..),
Paused(..) }` behind a `tokio` mutex; the enum gives dispatch, the type-state
guarantees each arm can only do legal things.
**Alternatives considered:** Runtime state enum + `if state != Running { err }`
checks in every method — works, but every new method is a chance to forget the
check; the compiler is a better reviewer.
**Consequences:** Slightly more ceremony (phantom types, the wrapper enum);
transitions that can fail (e.g. `start`) return `Result<Session<Running>,
(Session<Stopped>, Error)>`-style shapes so the session is never lost on error.

## 2026-07-08 — ADR-4: Embedded Capstone (M68K) for bulk disassembly

**Status:** accepted
**Context:** Hatari's debugger has a fine 68000 disassembler (`disasm`), but
it streams line-by-line through the log over the fifo — fine for one address,
hopeless for bulk work. Player fingerprinting and static cycle estimation need
to disassemble large regions of a frozen snapshot with zero round-trips.
**Decision:** Embed the `capstone` crate (M68K backend) in the `analyze`
layer for fast, offline disassembly over an acquired `MemImage`. Hatari's own
`disasm` remains the live path for single-address queries (it knows live
symbols and the real PC). We accept the libcapstone C dependency.
**Alternatives considered:** (a) Hatari-only — round-trip cost makes
fingerprint sweeps unusable; (b) pure-Rust 68k decoders — less mature/complete
than Capstone's M68K support; (c) writing our own decoder — unjustified scope.
**Consequences:** One C library in the build (well-packaged everywhere we
target); two disassembly sources whose output may differ cosmetically —
tools state which source produced a listing.

## 2026-07-08 — ADR-5: Server-side scanning + Finding handles (token-efficient surface)

**Status:** accepted
**Context:** ST RAM is ≤4 MB; naive tools would pour megabytes into the
agent's context. Anthropic's "code execution with MCP" guidance: filter and
transform in the execution environment, return references, disclose
progressively.
**Decision:** The server owns the megabytes. Scans return **compact
`Finding` handles** (plugin id, kind, offset, len, label, provenance) — never
raw memory. `extract(finding_id, as=…)` materializes an artifact **to disk**
just-in-time and returns a path (small images additionally return as MCP image
content blocks so the agent sees them inline). Every bulk-capable tool has a
default limit + pagination. The tool surface stays lean: related verbs group
into one `action`-param tool (`breakpoint`, `state`); `list_ripper_plugins`
provides discovery instead of front-loading every plugin's schema.
**Alternatives considered:** Returning bytes/base64 directly — simplest, but
burns the agent's context and makes long rip sessions non-resumable; findings
as full data objects — same problem at smaller scale.
**Consequences:** The server keeps the last scan's findings in session state
(handles must survive between calls); files under an output dir become the
inter-call intermediate state, which also makes long sessions resumable.

## 2026-07-08 — ADR-6: Deprotection — interface only, dummy implementation

**Status:** accepted
**Context:** Protections (Copylock, Sly Packer, Anti-bitos… — encryption +
anti-debug) are a distinct dimension from packers (compression). `dec0de`
exists as prior art (`refs/orionfuzion/dec0de`). But in a live session the
problem largely evaporates: protected programs self-decrypt in RAM, so
scanning live memory sidesteps the protection the same way it sidesteps
packers.
**Decision:** Specify the seam now, implement nothing real: a `Deprotect`
trait (`detect(&[u8]) -> Option<ProtectionInfo>`, `strip(&[u8]) ->
Result<Vec<u8>>`) registered in the ripper `Registry`, shipped with a single
`NoopDeprotect` that detects nothing. The engine may *report* detections as
findings but never auto-strips. No real unprotectors, no dec0de wrapping, in
this plan.
**Alternatives considered:** (a) Wrap the `dec0de` binary — external process
dependency for a capability we don't need yet; (b) port unprotectors —
significant scope, low value while live-RAM ripping works; (c) no seam at
all — would force an engine edit the day deprotection matters, violating
ADR-2's extension invariant.
**Consequences:** The extension point is compile-tested and documented from
day one; future deprotectors are "one file + one registry line" like every
other plugin; the dummy costs a few dozen lines.

## 2026-07-08 — ADR-7: Sentinel-synced fifo instead of blind sleeps

**Status:** accepted
**Context:** The manual workflow (`../MJJ/DEBUG.md`) sleeps ~1 s after every
fifo write because the fifo is async and command output only appears in
Hatari's log — slow and racy.
**Decision:** Every debugger command is followed by a marker command
(`evaluate $DEADBEEF`); the server tails the log until the marker's echo
appears, then returns the intervening output. Deterministic completion, no
sleeps, bounded timeout → typed `Error::Timeout`. This is the load-bearing
reliability primitive of the `session` layer and gates the Booting→Running
transition too.
**Alternatives considered:** Fixed sleeps (status quo — slow, flaky); parsing
Hatari prompts (no prompt is emitted on the fifo path).
**Consequences:** The marker must be relocation-proof and collision-free in
log output; all higher layers get synchronous-feeling, deterministic
`debug(cmd) -> output` semantics for free.

## 2026-07-08 — ADR-8: Test-target strategy — MJJ demo lacks symbols

**Status:** accepted (interim — made autonomously while the maintainer was away;
flagged for review, see Consequences)
**Context:** A live Hatari capture against the MJJ demo (2026-07-08, fixtures now
at `tests/fixtures/hatari_session.log` + `hatari_screenshot.png`) revealed two
facts that contradict `../MJJ/DEBUG.md`: (1) the auto-run program is
**`MJJHACK.PRG`**, not `STRGOOSE.PRG`; (2) that build carries **no DRI/GST symbol
table** — `symbols prg` returns `ERROR: symbol table missing from the program!`
(program flags PRIVATE). Symbol-dependent behaviour matters for P2 (disasm by
symbol) and P6 (labelling a fingerprinted player); P0/P1 do not need symbols.
**Decision:**
1. Use **`MJJHACK.PRG`** as the real-world integration/smoke target; treat it as a
   deliberately *no-symbols* case. Parsers must handle both the DRI/GST success
   format (per DEBUG.md) **and** the `symbols` failure line; no test asserts
   successful symbol loading against the MJJ demo.
2. **Do not modify the MJJ project** (separate, maintainer-owned; out of scope for
   this repo). For symbol-dependent phases, ShirazMCP ships its **own minimal
   symbol-bearing `.PRG` fixture** (assembled with `vasm -Ftos`, committed under
   `tests/fixtures/`) so P2/P6 have a deterministic, self-contained target with no
   dependency on any external build.
3. The captured session output (cpureg / evaluate / m / screenshot + the symbols
   failure line) is the canonical parser fixture set.
**Alternatives considered:** (a) Rebuild the MJJ demo with symbols — but that
project isn't ours to change, and coupling this repo's tests to an external build
is fragile; (b) skip symbol features in tests — leaves P2/P6 undertested;
(c) depend on symbols from `MJJHACK.PRG` — impossible, it has none.
**Consequences:** The test suite is self-contained for symbol-dependent features;
the MJJ demo stays a valuable no-symbols real-world smoke target. **Open for the
maintainer:** if you'd rather have a symbol-bearing MJJ build, tests can
*additionally* point at it — but nothing will depend on that. This ADR can be
superseded if you choose that route.

## 2026-07-08 — ADR-9: MemImage acquisition via savebin whole-RAM dump

**Status:** accepted
**Context:** The ripper (ADR-2, ADR-5) operates on a `MemImage` snapshot of
all ST RAM (≤ 4 MB). BRAINSTORM left open whether Hatari can dump RAM to a
file in one command or whether we must assemble it from ranged `m` memdumps
over the sentinel fifo. Spike procedure and both designs: docs/plan/P1.md §5.5.
**Decision:** Branch (a): Hatari 2.4.1's `savebin <file> <addr> <len>`
debugger command dumps the full range to a file in one sentinel round-trip;
`acquire()` sends it, reads the file, and validates the exact length. Spike
run against the live MJJ session (`--machine ste --memsize 1`, booted via the
DEBUG.md invocation): `savebin /tmp/shiraz-mcp-spike/spike-ram.bin 0 $100000`
completed on the first sentinel round-trip, logged `Wrote 0x100000 bytes.`,
and the resulting file was exactly `0x100000` bytes, not all-zero (nonzero
bytes present at `$400..$600`, the TOS system-variable range, and PC-local
code above that). All three acceptance criteria (§5.5 step 2) passed cleanly
on the first attempt — branch (b) was never needed.
**Alternatives considered:** Branch (b) — assembled `m` memdumps in 1024-byte
chunks (~1024 round-trips per MiB) — designed but not implemented; the cost
asymmetry against branch (a)'s single round-trip made it purely a fallback.
Hatari memory-snapshot files (`--memstate`) — rejected: snapshot format is
internal/versioned, not a raw RAM image.
**Consequences:** Acquisition is one round-trip + one file read (milliseconds
for 1 MiB); the RAM dump file under `work_dir` doubles as filesystem
intermediate state (ADR-5). `src/session/memory.rs` implements branch (a)
only — no dead `#[cfg]`-gated branch (b) code exists in the tree.

**Addendum — three further live-Hatari discoveries made while validating P1
end-to-end (docs/plan/P1.md §10.4), recorded here since they bear on the same
sentinel/fifo machinery this ADR's spike exercised:**

1. **Sentinel echo format.** P1.md's `SENTINEL_ECHO` constant
   (`"#3735928559 (dec), $deadbeef (hex)"`) was derived from the fixture's
   only `evaluate` example (`evaluate pc`, a small positive value). Live
   Hatari prints the decimal field as `unsigned/signed` when the sign bit is
   set — `evaluate $DEADBEEF` actually produces `#3735928559/-559038737
   (dec), $deadbeef (hex)`, which never matched the assumed literal. Fixed by
   keying the match on the hex field only (`"$deadbeef (hex)"`), which is
   unique in combination with the existing `"= %"` prefix check. See
   `src/session/fifo.rs`'s `SENTINEL_ECHO` doc comment.
2. **`m` command fifo interference.** Hatari's `m` (memdump) debugger command
   was found to reliably consume the very next fifo-queued line if it lands
   in the same read() batch as `m`'s own processing — silently swallowing
   the sentinel that `debug(cmd)` writes immediately afterward, with no
   error and no trace in the log. Reproduced deterministically across single-
   and multi-line `m` output, RAM and I/O addresses alike. Fixed by making
   the sentinel write retryable within the existing poll budget
   (`read_sentinel_with_retry` in `src/session/fifo.rs`) rather than adding a
   blind pre-sentinel sleep — this reuses the existing 25 ms poll cadence and
   adds no new fixed-sleep call site.
3. **`hatari-stop` is not resumable via the fifo (UNRESOLVED — flagged for
   rust-architect).** Once `hatari-stop` is sent over `--cmd-fifo`, Hatari's
   entire command-fifo servicing loop halts — confirmed by the log's
   "command FIFO read error" polling spam itself stopping — and does **not**
   recover, even after repeated `hatari-cont` writes over a 10+ second
   window. This contradicts DEBUG.md's documented workflow and P1.md's
   `pause` tool description ("Debugger, memory and screenshot tools keep
   working" while paused), and breaks the freezer-cartridge premise this
   whole session layer is built on. `session::Session<Running>::pause()` /
   `Session<Paused>::resume()` are implemented exactly per P1.md §5.3 (using
   `hatari-stop`/`hatari-cont`) and are correct against that spec, but
   `pause_freezes_pc` (P1.md §10.4) fails against real Hatari 2.4.1 as a
   result — not a code defect, a design assumption that doesn't hold. A
   `hatari-shortcut pause` alternative was tried and found unreliable in this
   headless setup (PC kept advancing across supposedly-frozen reads). No fix
   applied; needs an architectural decision, not a unilateral fifo-protocol
   patch.

## 2026-07-09 — ADR-10: Finding handles resolve against Arc'd source buffers in a single last-scan store

**Status:** accepted
**Context:** ADR-5 requires `extract(finding_id)` to work on a *later* call
than the `rip` that produced the finding — including findings located inside
depacked buffers, which have no bus address and no bytes in the live RAM
image. Something must retain the bytes between calls.
**Decision:** Each `Finding` privately holds an `Arc<MemImage>` of the exact
buffer it was found in (the root snapshot or a depacked child), attached by
the engine during `Registry::scan` (which therefore takes `Arc<MemImage>`).
The tool layer retains exactly ONE `ScanStore { scan_id, findings, out_dir }`
— the latest scan — behind a std (non-tokio) mutex; `FindingId` is
`"<scan>:<index>"`, so ids from any earlier scan are detectably stale and
are rejected with an explicit "re-run rip" error, never silently remapped.
Extraction slices the stored buffer — no depack replay, no emulator access —
so it also works after the session stopped.
**Alternatives considered:** (a) Replaying the depack chain (`via`) at
extract time — cheaper memory, but re-does the work, and couples extract
correctness to depacker determinism; (b) writing every depacked buffer to
disk at scan time — pays I/O on every scan for artifacts that are mostly
never extracted; (c) a multi-scan history — unbounded memory for no v1 use
case.
**Consequences:** Scan buffers stay in memory until the next scan replaces
them (bounded: root ≤ 4 MiB + depacked children ≤ 16 MiB each, depth ≤ 4);
the agent contract is "extract soon after rip, or re-rip"; `Registry::scan`'s
signature refines the ARCHITECTURE.md sketch from `&MemImage` to
`Arc<MemImage>` (synced in the same phase).

## 2026-07-08 — ADR-11: Pause = debugger freeze over stdin; `hatari-stop` is banned

**Status:** accepted (supersedes P1.md §5.3's `hatari-stop`/`hatari-cont`
mechanism; resolves ADR-9 addendum §3)
**Context:** Sending `hatari-stop` over `--cmd-fifo` permanently deadlocks
Hatari 2.4.1: the fifo is serviced only from the running main loop, and
`hatari-stop` halts that loop without entering the debugger — confirmed
live (5× `hatari-cont`, fifo and stdin probes all dead; ADR-9 addendum §3
and docs/plan/P1-finalize.md §0). `hatari-shortcut pause` was tried earlier
and found unreliable headless (PC advanced across "frozen" reads). The
pause/resume tools and the freezer-cartridge "freeze then rip" premise
needed a mechanism that actually works.
**Decision:** Two control planes, selected by session state:
1. **Running → cmd-fifo** (unchanged): `hatari-debug <cmd>` + sentinel.
2. **Paused → Hatari's stdin**: spawn Hatari with `Stdio::piped()` stdin,
   held open for the session's life. `pause()` = arm an always-true
   one-shot breakpoint over the fifo (`hatari-debug b pc > 0 :once` — fires
   on the next instruction, enters DebugUI, CPU halted) and confirm the
   freeze with a sentinel round-trip **over stdin** (DebugUI reads stdin;
   its echo proves the debugger is live and the CPU parked). While Paused,
   every debugger command goes to stdin as a raw CLI line (no
   `hatari-debug` prefix); the same log-tail sentinel machinery (ADR-7)
   applies verbatim — confirmed live for `evaluate`, `r`, `savebin`
   (two byte-identical 1 MiB dumps) and `screenshot`.
   `resume()` = write `c` to stdin ("Returning to emulation…"), then
   confirm liveness with a fifo sentinel round-trip.
   **`hatari-stop` must never be written to the fifo** — by anything, ever.
**Alternatives considered:** (a) `hatari-stop`/`hatari-cont` — deadlocks,
banned; (b) `hatari-shortcut pause` — unreliable headless; (c) drop the
pause/resume tools and rely on single-command atomicity — every individual
debugger command already executes with emulation suspended (ADR-9's
`savebin` is a coherent point-in-time snapshot even from Running), but
multi-command consistency (registers + RAM + screenshot at the *same*
instant) and the freezer workflow genuinely need an interactive freeze;
rejected as a needless feature cut when (2) works.
**Consequences:** The freezer premise is *preserved and made literal* —
freeze = break into the monitor (exactly what a freezer cartridge's NMI
did), inspect at leisure, `c` to thaw. Invariants the session layer must
uphold: fifo writes made while frozen queue and replay on resume (so
Paused never writes the fifo — the type-state transitions are the only
channel mutators); stdin writes after `c` queue until the *next* debugger
entry (so `c` is always the last stdin write of a pause episode); sentinel
echo lines are filtered from all collected output ($DEADBEEF is reserved),
making stale queued sentinels harmless. Known limitation: with stdin now
held open, a user-armed breakpoint (raw `debug("b …")`) that fires without
an auto-continue action freezes the machine while the session still thinks
it is Running — fifo calls then time out; calling `pause` recovers (its
stdin sentinel succeeds against the already-frozen debugger). P2's
`breakpoint` tool must own this interaction; the raw `debug` tool
description warns against arming breakpoints directly.

## 2026-07-09 — ADR-12: Picture rendering via RipPlugin::render_png; extract returns CallToolResult

**Status:** accepted
**Context:** P4 adds picture decoding (Degas, NeoChrome). `extract as=png`
must decode a finding's planar bytes and hand the agent the picture inline
(PLAN §P4), which raises two design questions: where does per-format decode
live without breaking ADR-2's "add a format = one file + one registry line"
property, and how does a tool that previously returned typed JSON also carry
an image content block?
**Decision:** (1) `RipPlugin` gains a **defaulted** `render_png(&self, bytes)
-> Option<Vec<u8>>` — a picture plugin implements scan + render in its one
file; non-picture plugins inherit `None`. `Registry::render_png(plugin_id,
bytes)` is the engine-generic lookup; the tool layer resolves the finding's
producing plugin through it. `None` doubles as "corrupt input" (with a
tracing::warn), symmetric with `Depacker::try_unpack` — render never errors.
(2) The `extract` tool returns `CallToolResult`: raw/prg produce one text
block containing the serialized `ExtractOutput` JSON; png produces an
`image/png` block plus the same JSON text block. PNG stays out of
`ExtractFormat` (it is a render, not a byte container — tool-layer
`WireFormat` splits the pipelines). (3) Palette decoding auto-detects STE
per-palette (`any word & 0x0888 != 0`) and detection requires a plausible
palette (upper nibbles clear, ≥ 4 distinct words) — the distinctness gate is
what keeps zero-filled RAM from matching everywhere. (4) Graphics test
fixtures are generated by byte-pinned builders in `tests/graphics.rs`, not
committed binaries (archive images have unclear redistribution status; a
pinned builder is a better golden reference anyway).
**Alternatives considered:** a byte-sniffing `picture::decode(bytes)`
dispatcher (rejected: a second per-format registration point, eroding the
one-file property); a separate `Renderer` trait + registry role (rejected:
more machinery for zero extra capability — every renderer is a detector);
keeping `extract` as `Json<ExtractOutput>` and adding a separate
`extract_png` tool (rejected: fatter tool surface for one format flag,
against the progressive-disclosure grouping in ADR-5); `structured_content`
on `CallToolResult` (rejected: unverified construction path in rmcp 2.2, the
text-block JSON carries the same information).
**Consequences:** `extract` no longer advertises an outputSchema — its JSON
now travels inside a text block (agents parse it fine; handshake test
updated). A future picture format is still one file + one registry line,
including its PNG rendering. Corrupt-picture renders surface to the agent as
invalid_params ("not renderable"), with detail only in server logs. ST-vs-STE
palette ambiguity is resolved per-palette, so an all-ST-range STE image
decodes with ST scaling (imperceptible: ≤ 2/255 per channel).

## 2026-07-09 — ADR-13: Degas/NeoChrome header aliasing — geometry gate + defer rule

**Status:** accepted (amends the P4.md §5.4/§5.5 pinned detectors and §8.2 fixtures)
**Context:** The P4 pinned detectors mutually alias on real-world inputs, and
the pinned golden tests caught it. (1) A low-res Degas PI whose palette color
0 is `0x000` (black — near-universal in real ST art) reads, at the same
offset, as a valid NeoChrome header: Degas res word `0x0000` satisfies NEO's
"flag must be 0", `PAL[0] = 0x0000` satisfies NEO's res word, and
`PAL[1..=15]` + first screen word passes `plausible_palette` (≥ 4 distinct).
(2) Symmetrically, every NEO image fires the Degas PI detector at its own
offset (flag word 0 = valid low-res word; palette window = NEO res word +
first 15 palette entries) and at offset +2. Both directions were verified by
byte-exact simulation of the pinned algorithms. Consequence: §8.2 tests 1, 4
and 7 assert exact finding counts (1/1/2) that the pinned detectors cannot
produce (2/2/4). The pinned constants, algorithm, and expected outcomes are
mutually inconsistent; one must yield.
**Decision:** The finding-count assertions express the intent (precise,
noise-free findings — ADR-5's compact-findings principle); the detectors are
what get amended. Two contained changes, both in the plugin layer, no engine
change:
1. **NEO geometry gate.** `image.rs` gains
   `fn neo_header_at(bytes, o) -> bool` (same visibility as its neighbor
   `plausible_palette`): flag word 0, valid res
   word at +2, `plausible_palette` at +4, **and width == 320 / height == 200
   at +58/+60**. These fields are real, documented NEO header fields
   (`NEO_IO.C` authority; the format skill explicitly suggests this sanity
   check) and are pinned to 320/200 by the §8.2 `make_neo` builder.
   `try_neo` detects via this predicate. This kills direction (1): Degas
   screen bytes at +24/+26 would have to be exactly `0x0140 0x00C8`.
2. **Degas defers to NEO.** `try_pi` rejects a candidate at `o` when
   `neo_header_at(o)` or (`o >= 2` and) `neo_header_at(o - 2)` holds. A full
   NEO header (62 constrained bytes) essentially never occurs by chance at a
   real PI's start, while a PI header (res + 4-distinct palette) trivially
   occurs at every NEO's start — the more-specific signature wins the tie.
   The predicate lives in the shared `image.rs`, so degas does not import
   neochrome (no lateral plugin→plugin dependency; ADR-2's one-file property
   holds). `try_pc` needs no guard (bit 15 of the PC res word can never
   collide with NEO's zero flag word).
3. **Test fixtures use 0xFF fill, not 0x00.** §8.2 pinned "zero buffer", but
   zero fill ahead of a planted low-res PI is itself a valid-looking PI
   header (res word 0x0000 + a palette window sliding onto the real
   palette), shifting the finding to a wrong offset (~o−24). `0xFFFF` is not
   a valid res word for any format. All planted-buffer golden tests fill
   with 0xFF; the exact-count/offset assertions stay byte-for-byte as
   pinned.
**Alternatives considered:** Relaxing the golden tests to "contains the
expected finding" (rejected: blesses a production false-positive machine —
every NEO in RAM would also report a bogus Degas finding and vice versa,
polluting the compact-findings surface ADR-5 exists for). Changing `PAL[0]`
to a non-res-word value like `0x111` (rejected: **provably insufficient** —
simulation shows Degas still fires inside a planted NEO regardless of
`PAL[0]`, because the aliasing words are NEO's flag/res words, not the
palette; it would also make the shared fixture unrepresentative of real art,
where color 0 is black). Engine-level overlap/priority resolution between
plugins' findings (rejected for now: real machinery — priority ordering,
containment rules — for exactly one known pair; **revisit trigger:** if a
third graphics format needs a pairwise defer rule, promote disambiguation to
a registry post-pass instead of accumulating guards).
**Consequences:** §8.2 tests 1/4/7 pass with their pinned exact counts.
NEO files whose width/height header fields deviate from 320/200 (rare;
NeoChrome itself writes 320/200) are no longer detected — acceptable
precision/recall trade, revisit only against a real corpus. Known residual
limitations, accepted: a PI preceded by zero-filled RAM still yields a
finding offset a few bytes early (zero fill is indistinguishable from a
low-res header prefix); a PI whose screen data happens to contain
`0x0140 0x00C8` at bytes 24..28 plus an aliasing prefix could still
double-report (vanishingly rare). P4.md §5.2/§5.4/§5.5/§8.1/§8.2 amended in
place with "(amended per ADR-13)" markers so spec and implementation agree.

## 2026-07-09 — ADR-14: Pack-Ice depacker — ported semantics, whole-buffer magic scan, pinned synthetic fixtures

**Status:** accepted
**Context:** P5 ships the first real `Depacker` (Pack-Ice, the dominant ST
scene packer) plus a cheap packed-region detector, proving the P3
depack→rescan recursion end to end. Three questions: what algorithm source
of truth; how a `try_unpack(&[u8]) -> Option<Vec<u8>>` that receives the
whole buffer locates a blob that can sit anywhere in a 4 MB RAM image; and
how to test an LZ bit-stream decoder without redistributable binaries.
**Decision:** (1) The decoder (`plugins/ice_decode.rs`) is a line-for-line
semantic port of `Atari_ST_Sources/C/Toyoyo/stvn/deps/pack-ice/
ice_decrunch.c` — backward bit stream with recomputed-sentinel byte reloads,
backward output, the three length/offset tables, and the descending
overlap-replicating match copy (memmove semantics would break RLE runs).
Every C `exit(1)` and every bound violation is `None`: the depacker never
panics and never errors. Both header magics (`ICE!` 2.3x/2.4, `Ice!` 2.1x)
are accepted over the same 12-byte header (crunched/decrunched u32 BE) and
decoded with the single 2.3x algorithm — deviant old streams fail a bounds
check and return `None` (fail-closed). (2) `try_unpack` scans the whole
buffer for both magics (memmem), sanity-checks headers (crunched ≥ 13,
1 ≤ decrunched ≤ MAX_UNPACKED, region within buffer), and returns the
first candidate that decodes cleanly. The P3 trait returns one buffer, so
with several packed regions only the first is recursed; the
`pack-ice-region` detector (kind `Packer`) still reports all of them, and
the agent can extract a region as raw bytes and rip it separately.
(3) Fixtures are synthetic and byte-pinned, not committed binaries and not
a port of `ice_crunch.c` (deviation from PLAN §P5's "commit the packed
fixture", consistent with ADR-12's fixture policy): five hand-traced byte
vectors cover the match/run/len-2 paths and both magics, and a pinned
consumption-order stream assembler (`ice_assemble`) manufactures larger
blobs. The 32 KB packed-PI1 recursion fixture deliberately splits the
payload at offset 3 and rebuilds the leading zeros with an RLE-run match:
a literal-only blob would carry the 34-byte Degas header verbatim, letting
the root-level scan find the picture without depacking and voiding the
depth-0 assertions (also, the format's main loop mandates a match between
any two literals — multi-literal streams don't exist). Unread bit-byte
positions are 1-filled so no predictable 0x00 byte abuts the literal data.
Table rows unreachable by small vectors (depack-length i=3/4, 8/9/12-bit
offsets) are unit-tested against the private decoder helpers directly.
**Alternatives considered:** porting `ice_crunch.c` for fixtures (rejected:
~an entire second codebase to review, testing our decoder against our port
of their encoder proves less than hand-traced vectors); committing a
reference-packed binary fixture (rejected: opaque golden data, unclear
redistribution, and it wouldn't pin WHY the bytes are correct); giving
`Depacker::try_unpack` an offset/multi-result signature to recurse every
region (rejected: P3 trait/engine is frozen, one-buffer covers the dominant
"one packed prod in RAM" case — revisit as an engine ADR if real targets
need it); requiring the depacker to only match at offset 0 (rejected:
packed blobs are embedded at arbitrary offsets in RAM and in files —
the whole point of the scan).
**Consequences:** "add a packer" stays one plugin file + one registry line
(the decoder helper module is private to the crate and doesn't add a
registration point). Depack cost is paid once per buffer per scan level and
is bounded by the existing MAX_UNPACKED/depth guards; the detector is
near-free (two memmem passes). Nested packing (ICE inside ICE) resolves
naturally through the depth-bounded recursion. Old pre-2.1 or exotic ICE
streams silently yield nothing rather than wrong data. One implementation
correction versus the P5 spec text: §7.1 item 6's literal `direct_length`
i=3 test vector (`[1,1,1,1,1,1,1,1,0,1]`, 10 bits) was internally
inconsistent with its own stated derivation ("i=3, value 2, bits 010",
length 10) — reaching a non-escalating i=3 row takes exactly 8 bits
(1+2+2+3, rows 0..2 escalating at their pinned all-ones values), not 10;
the corrected 8-bit vector (`[1,1,1,1,1,0,1,0]`) is used instead, verified
against the same escalation rule that the other three rows in that test
already satisfy exactly.

## 2026-07-09 — ADR-15: Managed run-to-breakpoint owns the freeze lifecycle

**Status:** accepted (closes ADR-11's known limitation)
**Context:** ADR-11 left one hole: a breakpoint armed via raw
`debug("b …")` fires outside the session FSM — the machine freezes while
the session believes it is Running, and every fifo call then times out.
P2's `breakpoint`/`profile` tools need firing breakpoints as a *feature*,
so the session must be able to run to a breakpoint deliberately and adopt
the freeze.
**Decision:** (1) A new primitive exploits ADR-11's own queueing
invariant: from Paused, write `c` then ONE sentinel probe on stdin — the
probe sits unread while the machine runs and is echoed the instant a
breakpoint re-enters DebugUI. `Session<Paused>::run_until_break(timeout)`
returns a three-state `RunToBreak` (`Fired(Paused)`, `TimedOut(Running)`,
`Broken(Running)`) so every outcome is a compile-time-known session
state. (2) All managed breakpoints are `:once` and synchronous: `set` =
freeze-if-needed → arm over stdin → run → fired ⇒ Paused-at-breakpoint /
timeout ⇒ disarm over the fifo (list+remove work in the running main
loop, verified live) and stay Running. Nothing persists armed behind the
FSM's back. (3) The one unavoidable race — fire between timeout and
disarm — returns a distinguished error whose remedy is `pause` (ADR-11's
documented recovery). (4) `profile(from,to)` composes two managed runs
around Hatari's profiler and parses `profile save`'s file (filesystem as
intermediate state, ADR-5), ending Paused at `to`. (5) Spawn adds
`--alert-level fatal`: Hatari alert dialogs are modal and would hang the
managed flows' error paths (`stateload` version mismatch); log lines are
unaffected (verified in log.c).
**Alternatives considered:** a background watcher polling for unmanaged
freezes (adds a concurrent writer to the control planes ADR-11 carefully
serialized); detached persistent breakpoints with lazy freeze-detection
(leaves the FSM lying between fire and detection — the exact hole being
closed); `:trace`-only breakpoints that never freeze (can't support
freeze-at-PC workflows like profile legs).
**Consequences:** `breakpoint(action=set)` is synchronous run-to-break —
long waits occupy one tool call (bounded by timeout_ms); the raw `debug`
tool description now points at the managed tool; `stateload` clears
Hatari's breakpoint table (verified live), documented in the `state`
tool; profiling leaves the session Paused, making freeze-inspect-resume
the explicit workflow.

## 2026-07-09 — ADR-16: Music identification — disasm-confirmed YM anchor, provisional masked-word signature DB, label round-trip channel

**Status:** accepted
**Context:** P6 must name the music driver resident in RAM. SNDH blobs are
self-describing (header tags), but raw ripped/depacked players are not; the
only relocation-invariant trace of any driver is code referencing the PSG
ports `$FFFF8800`/`$FFFF8802` (ym2149-psg skill). Fingerprinting needs code
context; findings need to reach the `identify_music` tool in structured form
without changing the engine.
**Decision:** (1) Anchor = two-phase scan: memmem byte candidates for the
four operand encodings (abs.l / abs.w × select / data port), each confirmed
by a single-instruction Capstone decode at candidate−{2,4,6} (P2
`analyze::disasm`) gated on move/lea mnemonics + the P2-pinned PSG operand
text; confirmed hits cluster within 512 bytes into one Finding per replay
routine. This introduces the (pinned-legal) `ripper → analyze` dependency —
`analyze` is a pure leaf over `types`. (2) Naming = const signature DB of
masked big-endian words: hardware constants literal, relocated address
operands mask 0x0000, opcode register fields partially masked; scopes
AnchorWindow (code, ±512) and WholeImage (format magics). Seeds for
Hippel/Mad Max, TCB Tracker, maxYMiser, Lucas/Quartet are PROVISIONAL
(documented magics + documented envelope-shape behavior, not yet
corpus-extracted); no match degrades to ("unknown", 0.25) — never an error.
(3) The ym-player Finding label is a pinned machine-parseable format
(`format_label`/`parse_label`, round-trip tested); `identify_music` rebuilds
structure from it plus a re-parse of SNDH finding bytes, instead of
extending `Finding` with a metadata field. (4) SNDH finding length is a
64 KiB cap heuristic — the format records no total length.
**Alternatives considered:** Linear-disassembling the whole image (2M
decodes on 4 MB, stream-alignment garbage; rejected). A typed
`Finding.metadata` field (engine + wire change for one plugin; revisit if a
third plugin needs structured labels). Shipping corpus-extracted
fingerprints now (needs a labelled SNDH corpus pass; deferred to backlog —
the degradation contract makes provisional data safe). Naming players from
the SNDH `CONV` tag (surfaced raw instead; fusing it into confidence is
backlog).
**Consequences:** Unknown drivers are a first-class, non-error outcome.
Provisional signatures may misname exotic rips at their stated ≤0.80
confidence; WholeImage magics label every anchor in an image (mislabels
multi-player images — rare, accepted). OS floppy-select code (PSG port A)
can surface as low-confidence anchors. The 64 KiB SNDH cap can truncate
pathological blobs or over-grab trailing RAM. Signature refinement against
frno7/sndhfix + the sndh.atari.org corpus is the pinned improvement path.

## 2026-07-09 — ADR-17: Boot-sector analysis — $1234 checksum, LE BPB, disasm-based virus tells (heuristics, never a verdict)

**Status:** accepted
**Context:** P7 (final phase) must extract and analyze Atari ST floppy boot
sectors — the historic virus vector. A sector is executable iff its 256
big-endian 16-bit words sum to $1234; it carries a little-endian DOS BPB; and
its 68k boot code is either a benign loader or a self-installing virus. The
same engine must serve two sources (a raw .ST file and a live RAM snapshot)
and must classify safely — a false "this IS a virus" is worse than a hedged
"suspicious, here are the tells".
**Decision:** (1) One `bootsector` detector (RipKind::Boot) over a 512-byte
sector: big-endian word-sum checksum vs $1234 (executability); little-endian
BPB decode with a power-of-two plausibility gate; classification into
plain-data / loader / suspicious. (2) Virus tells come from P2's embedded
disassembler (the pinned-legal ripper→analyze dependency, ADR-16): the primary
tell is a *write* to a watched low-memory vector ($426/$42A resvalid/resvector,
$472..$47E hdv_*, $4CE vbl_list) — distinguished from a *read* (antivirus
behaviour, e.g. ZR_BLK.S), so a defender bootblock classifies as loader, not
suspicious; secondary tells are a self-copy loop (post-increment move + dbra)
and BIOS/XBIOS disk traps (weak — do not force suspicious). Every result
carries tells + a confidence capped at 0.9; any decode failure degrades to no
tells / 0.0 — never an error, never a certain verdict. (3) Two sources feed the
one detector: disk mode is a 512-byte MemImage (always reported); RAM mode
dereferences _dskbufp ($4C6) in a whole-RAM snapshot and reports only a
plausible sector (no zero-buffer noise; _dskbufp persistence is // verify
live). (4) `ripper::disk::read_boot_sector` reads sector 0 of a raw .ST with
canonicalize + regular-file + extension + size validation (.MSA/.DIM rejected,
not misread). (5) The `bootsector` tool is a façade like identify_music: RAM
mode reuses rip(kind=boot); disk mode scans a file-backed MemImage and stores
it, so extract(as=raw|prg) works. Structured facts are recomputed from finding
bytes via analyze_sector; the label stays human. The `tells` field carries the
annotated disasm evidence inline, so extract.rs is untouched. (6) The $1234
checksum + BPB decode (`Bpb`, `checksum`, `decode_bpb`, `bpb_plausible`) live
in `boot_bpb.rs`, split out of `bootsector.rs` for the line budget — the same
pattern P6 used for `sndh_header.rs` and P5 for `ice_decode.rs`.
**Alternatives considered:** A known-virus signature DB (Signum/Ghost/…) —
deferred: needs a curated malware corpus; behaviour heuristics ship first and
never over-claim. A new "annotated disasm" ExtractFormat — rejected: the tells
carry the evidence and the P2 disasm tool covers full listings; keep extract
generic. Executing the sector in a sandbox to follow boot-cracktro loads
(r1pp3r) — large, emulator-coupled; backlog. Scanning all of RAM for any
executable sector — noisy and unfounded; _dskbufp is the one principled RAM
source. Flagging vector *reads* as tells — rejected outright: it would misclass
every antivirus bootblock (false positives are the failure mode we most want to
avoid).
**Consequences:** Boot-sector executability and geometry are exact; virus
detection is honest but heuristic — suspicious is a hedge with evidence and a
sub-1.0 confidence, not an accusation. RAM mode depends on unverified _dskbufp
persistence (// verify live); it degrades to a clean not-found. Only raw .ST is
supported; compressed images fail loud. Disk-source extracts land in a temp
dir when no session is running. Signature-based family naming and the
sandbox-follow rip flow are the pinned improvement paths.

## 2026-07-09 — ADR-18: Hot-patch via external vasm — argv-only subprocess, diagnostics as data, readback-verified writes

**Status:** accepted
**Context:** P8 adds the freezer cartridge's built-in assembler: freeze the
ST → assemble a 68k snippet → write it into RAM → step/observe → iterate,
with no PRG rebuild. That needs a Motorola-syntax 68000 assembler. Writing
or embedding one in Rust is a project of its own, and no maintained crate
covers the full Motorola directive/operand dialect; meanwhile the exact
assembler the target demos are built with (vasm 2.0e, `vasmm68k_mot`) is on
disk, and dialect identity between "what I build" and "what I hot-patch" is
itself a feature. Shelling out to a binary also crosses two policy lines
that needed explicit treatment: dependency hygiene (an external, unpinned
binary) and subprocess security (never a shell).
**Decision:** (1) Shell out to vasm with `tokio::process::Command` and an
explicit argv vector — `-quiet -Fbin -o <tmp.bin> [-L <tmp.lst>] <tmp.s>` —
never a shell, source passed as temp-file content (controlled dir, RAII
cleanup, no user-controlled path components). Binary path from
`SHIRAZMCP_VASM` with a machine-local default (same policy as
`hatari_path`); an absent/failed binary is a typed `Error::Assembler`
(actionable message naming the env var), never a panic. No cargo feature:
there is no compile-time dependency to gate — runtime degradation is
strictly better than a build matrix. (2) A new leaf module `assemble/`
(driver + pure text helpers): `analyze/` stays a pure synchronous leaf,
`session/` stays Hatari-only, `ripper/` stays pure; only `tools/` imports
`assemble`. (3) Assembly diagnostics are DATA, not MCP errors: `{ok: false,
errors, warnings}` (sanitized — temp names scrubbed — and bounded, 16×200
chars) so the agent's assemble→fix→retry loop costs one round-trip per
iteration; the MCP error channel carries only infrastructure failures.
(4) The server injects `org` (always = addr for `patch`); user `org`/`rorg`
is rejected whenever an org is injected, because `-Fbin` zero-pads between
sections (verified: two orgs 256 bytes apart produced a 258-byte image) — a
silent gap-zeroing patch is the worst possible failure. (5) `patch` reuses
the proven paths end to end: `debug::write_bytes` (`memwrite`) in 256-byte
chunks (the `WRITE_MAX` the memory tools already pin), then
`memory::acquire_range` readback, byte-equality (`verified`) and an
embedded-Capstone disassembly of what RAM actually holds (cycle estimates +
hardware annotations via the shared P2 mapper). Bounds: source 64 KiB,
output 64 KiB, patch 4 KiB, vasm timeout 10 s, readback listing 64 insns.
Patch is legal in Running and Paused (both ADR-11 control planes serve
`memwrite`); Running is documented as racy, Paused is the recommended flow.
**Alternatives considered:** Embedding a Rust 68k assembler — rejected for
now: large surface (Motorola directives, expressions, optimizations), no
maintained crate, and dialect drift versus the toolchain the demos use;
revisit only if the external binary becomes untenable. Hatari's debugger —
has no assembler command, not an option. Returning assembly errors as MCP
errors — rejected: it turns the agent's edit loop into error-channel noise
and hides warnings on success. Accepting user org directives alongside an
injected org — rejected outright (gap-zeroing hazard above). A cargo
feature (`asm`) — rejected: gates nothing at compile time, costs a build
matrix. Streaming source over stdin — vasm reads files, and the temp file
keeps diagnostic line numbers meaningful.
**Consequences:** The 68k edit-compile-test loop runs entirely in place —
freeze, patch, step, observe — with byte-verified writes (a ROM/unmapped
destination is reported as `verified=false`, a fact not a fault). The
system depends on an external binary at runtime only, with graceful typed
degradation and a documented override; goldens pin the exact vasm
invocation and output bytes, so a vasm upgrade that changes behavior fails
loudly in tests. Partial writes on a mid-chunk failure are possible and
documented (re-patch is idempotent); undo/journal, symbol-aware assembly,
and PRG emission are the pinned improvement paths.

## 2026-07-09 — ADR-19: Atomik/PowerPacker/RNC depackers — scope cuts, end derivation, CRC gates, pinned synthetic fixtures

**Status:** accepted
**Context:** P9 triples the depacker roster (Atomik 3.5, PowerPacker PP20,
RNC ProPack), each ported from period 68k depack sources under ADR-14's
template (whole-buffer magic scan, first-success-wins, fail-closed None,
one plugin file + one registry line, pinned synthetic fixtures). Each
scheme forced a scope decision this ADR records.
**Decision:** (1) **Atomik: `ATM5` only.** The `ATOM` magic is shared by
at least Atomik 3.1, 3.3 and Thunder datafiles — the reference dispatcher
(NEW_DPAK's DEPACK.C) resolves it by asking the user, so magic-driven
depacking of `ATOM` would guess; deferred. Atomik's picture algorithm
(trailing count byte != 0: screens stored bitplane-transformed with an
offset table in the decoded output) is refused fail-closed — a wrongly
decoded picture is worse than none. The decoder is a semantic port of
depack35.s: backward stream, recycled-carry sentinel bit register (ICE's
reader with the reload sentinel recycled instead of forced 1 — the
fixture assembler therefore ZERO-fills bit bytes), per-chunk special-
offset/frequency tables and unary-threshold length coding, descending
overlap-replicating copies. Its bit reader and static tables live in a
fourth file, `atomik_bits.rs` (`Stream`, `BASES`/`XBITS`, the odd-number
table builder), split out of `atomik_decode.rs` purely for the 200-line
budget — `pub(crate)`-consumed, not a registration point, same as
`ice_decode`/`rnc_m1`/`rnc_m2`. (2) **PowerPacker: `PP20` only, derived
end.** PP20 carries no packed-length field (the format trusts the file
size), so a blob's end inside a RAM image is derived: try the trailing-
zero-trimmed end, then the buffer end (both aligned to the header,
trailer-sanity gated); the detector's extent is labelled estimated, the
depacker validates by decoding. Consequence: a PP20 blob followed by other
non-zero data in the same buffer is missed or mis-extended — accepted and
pinned by test; whole files and blobs in zeroed RAM (the dominant cases)
work. PX20 (XOR-encrypted, needs a passkey) and PPMT are not recognized.
An efficiency-table gate (each byte 4..=14) kills "PP20" text false
positives. (3) **RNC: standard 18-byte-header layout, methods 1 and 2,
CRC-16 gated both ways.** The header's packed-payload CRC-16/ARC is
checked at candidate time (detector and depacker — near-free, rejects
false positives and, deterministically, the incompatible OLD 12-byte
RNC\x01 layout of RNC_1_1.S, which is deferred); the output CRC is
checked after decode (the method-1 reference skips CRCs; the method-2
reference and dernc check them — we check uniformly). Method 1 is the
little-endian-word LSB-first bit stream with per-chunk canonical Huffman
tables and byte-aligned literal runs; method 2 is a forward MSB-first
prefix-code LZ with raw-block and end-of-stream escapes; the XOR key is
dropped (key 0 — keyed archives fail the output CRC and yield None).
(4) **Fixtures:** as ADR-14 — no committed binaries, no encoder ports;
per-packer consumption-order assemblers plus hand-traced vectors, all
mechanically verified against the spec before implementation. The
packed-PI1 recursion fixtures are literal-only where the encoding itself
interrupts or obscures the payload (Atomik/RNC: a flag-bit byte every <=8
literals, RNC m1: 16-byte runs; PP: literals are bit-packed and reversed)
so no contiguous Degas header survives at depth 0 — no P5-style split
needed. (5) **Detectors never decode** (uniform with pack-ice-region):
Atomik and RNC report header-exact extents; PP reports the derived
estimate rather than paying a validation decode.
**Alternatives considered:** guessing among ATOM variants by trying each
decoder (rejected: three incompatible schemes sharing a magic invites
silent wrong output; revisit with structural fingerprints); implementing
Atomik picture-mode now (rejected: needs the inverse bitplane transform
plus tail offset-table semantics, and no fixture path pins it); scanning
all aligned ends for PP20 (rejected: O(n) decode attempts per magic);
a decode-validating PP detector (rejected: breaks the cheap-detector
invariant; the depacker validates anyway); supporting the old RNC layout
behind the same magic (rejected: undetectable without decoding both ways;
the CRC gate cleanly selects the standard layout); skipping RNC method 2
as the task allowed (rejected: with the asm traced, method 2 is ~150
lines and method 1 is the harder one — both fit the budget).
**Consequences:** "add a packer" remains one plugin file + one registry
line (decode-helper siblings are pub(crate), not registration points).
The registry grows to 14 plugins; recursion depth/bomb guards are
untouched and now protect four depackers. Known misses are explicit and
tested: ATOM/PX20/PPMT/old-RNC blobs and non-zero-followed PP20 regions
yield nothing rather than wrong data.

## 2026-07-09 — ADR-20: machine_state — coherent one-call snapshot via momentary freeze

**Status:** accepted
**Context:** Agents inspecting a running program needed 4-6 tool calls
(registers, debug m ×3, view_memory_as_screen) to answer "what state is the
ST in?", and the reads spanned many emulated VBLs — resolution, palette,
screen base and screen contents could come from different frames (the
verified source of view_screen_matches_live's historical flake). The
freezer-cartridge heritage (Ultimate Ripper's status screen) suggests the
shape: one call, one frozen instant, everything on screen.
**Decision:** One `machine_state` tool that composes existing primitives
only: ADR-11 freeze/thaw, `debug::registers`, three pinned video-register
reads ($FFFF8201×13 covering base hi/mid/low + sync, $FFFF8260, $FFFF8240×32),
ranged `savebin`, and P4's `render_screen`. Coherence rule: a Running
session is **momentarily frozen** (pause → gather → resume, lock held
throughout); a Paused session is read in place and never thawed; Stopped is
a wrong-state error. Failure semantics: gather errors outrank thaw errors;
a failed thaw leaves the session Paused (recoverable via `resume`) rather
than returning a result that misreports session state. Token efficiency
(ADR-5): the 32000-byte buffer is returned only as a rendered PNG image
block (default on) and/or an on-disk path handle (`include_raw_dump`,
default off) — never as bytes in context; `include_screen=false` degrades
to a 4-round-trip text-only probe.
**Alternatives considered:** (a) read live with a `coherent:false` flag —
cheaper, but hands every consumer a cross-VBL-soup caveat on exactly the
animated programs the tool exists to inspect; (b) reuse Hatari
`--saveconfig`/state snapshots — heavyweight, file-format coupling, still
needs the register decode; (c) separate video_state/cpu_state tools — more
surface, still incoherent between the two calls.
**Consequences:** A machine_state call on a Running session halts emulation
for ~2 sentinel round-trips (invisible to the emulated program: no time
advances). The reported screen base is the *programmed* register value —
for double-buffered programs it may be the back buffer, which is correct
freezer semantics but means the render is not pixel-identical to
`screenshot` (verified live on MJJ; tests assert plausibility, never pixel
equality). The tool adds a per-call pause/resume cycle that assumes no
foreign breakpoints are armed while Running (ADR-15 already guarantees
managed runs return to a known state). SrOutput gained an additive `trace`
field; `ripper::image` gained `is_ste` as the single ST/STE detection rule.

## 2026-07-09 — ADR-21: ADR-11 addendum — freeze-drain + per-call-unique sentinels

**Status:** accepted (amends ADR-11 §2.1's control-plane sequencing;
supersedes ADR-11/ADR-15's "stale probe is harmless" claim)
**Context:** A DHS-demo rip session surfaced a real bug: the first debugger
command issued after `pause()` could mis-parse and, critically,
`memory::acquire`/`savebin` snapshots could come back truncated for the
rest of the paused episode (TODO.md, "found via the DHS test"). Two
distinct mechanisms turned out to be involved, confirmed live with a
CPU-stress harness (`tests/freeze_drain_live.rs`, background `yes >
/dev/null` on every core) against a genuinely symbol-bearing fixture
(`tests/fixtures/symtest.s` — neither MJJHACK.PRG nor the DHS demo's
UHANE.PRG actually carry a DRI/GST symbol table, contrary to the original
diagnosis's premise; `symbols prg` fails "missing from the program" for
both):
1. **Leftover `pending` text.** Per `LogTail::read_until`'s own doc, a line
   arriving in the same poll as — but after — a confirm sentinel's echo is
   left queued in `pending` for the NEXT call rather than consumed
   (debugger-entry banners, breakpoint arm/fire text, symbol-load
   messages).
2. **Stale sentinel echoes (the deeper cause).** `fifo::debug`'s sentinel
   write is retried every `SENTINEL_RETRY_BUDGET` (300 ms) when Hatari is
   slow to answer. Under real contention this reliably left MULTIPLE
   `$DEADBEEF` echoes queued from ONE logical call; a LATER call's
   `read_until` could then terminate on one of those stale echoes instead
   of its own, returning before Hatari had actually processed the real
   command — the true source of the observed truncation. This also
   contradicts ADR-15's "a stale queued probe is harmless" claim: harmless
   for output (sentinel echoes are filtered), but NOT for termination —
   a stale echo can end a later round early.
**Decision:** (1) `HatariProcess::freeze` and `continue_until_break`
(ADR-15) each drain stray stdin-entry text with one extra, fully discarded
sentinel round-trip (`drain_stdin_entry_text`, `control.rs`) immediately
after their own confirm — `LogTail::pending` carries into that call, so
leftover text becomes its (discarded) output, terminated by its own fresh
echo. A drain `Timeout` is swallowed (`tracing::warn`, not propagated) —
the caller already confirmed the CPU is parked, so a slow/absent drain
echo is hygiene, not a broken freeze; a non-timeout error (dead stdin)
still propagates. (2) **Per-call-unique sentinel values** (`fifo/
sentinel.rs`, new module): `$DEADBEEF` (fixed) becomes `$dead{counter:04x}`
(a process-wide `AtomicU32` counter), so `is_this_sentinel_line` can only
match the ISSUING call's own echo — a stale echo from any earlier call's
retry can never terminate a later one. `is_sentinel_family_line` (matches
the `$dead` prefix only, any value) still strips ANY stray sentinel echo
from returned command output, unchanged in spirit from the original
design. Both fixes are complementary and both required: (1) alone
provably cannot close (2)'s hole (a stale echo can satisfy the drain round
too — confirmed live: fix (1) alone still failed 8/8 stress runs); (2)
alone would still let stray non-sentinel banner/symbol TEXT leak as a
prefix onto a later call's legitimate output.
**Alternatives considered:** a blind post-freeze sleep (rejected — the
project bans blind sleeps on principle, ADR-7, and a fixed sleep is
unbounded-load-unsound in exactly the same way item (2) closes); looping
the drain until `pending` is observed empty (rejected — text not yet
written to the log file is invisible to `pending` regardless of how many
times it's inspected; only a fresh round-trip's OWN termination signal is
trustworthy); scoping the drain to `freeze()` only (rejected — a fired
managed breakpoint, ADR-15, is exactly the same kind of DebugUI
re-entry and reproduces the same leftover-text risk).
**Consequences:** `freeze()`/a fired managed breakpoint each cost one
extra sentinel round-trip (~one poll cycle when quiet, bounded by
`sentinel_timeout_ms` otherwise) — a `pause()` latency regression judged
acceptable against silently truncated snapshots. `fifo/mod.rs`'s tests
that asserted the fixed-`$DEADBEEF` shape moved to `fifo/sentinel.rs`
(value matching) with `mod.rs` retaining only protocol-plumbing tests.
Known residual gap (tracked, not closed here): the drain's own round-trip
can — in principle, not observed live post-fix — itself race a THIRD
stale echo from a still-in-flight earlier retry; per-call uniqueness makes
this vanishingly unlikely (a collision needs a call to still be
unprocessed 65536 calls later) rather than impossible. `tests/
freeze_drain_live.rs`'s `wait_until_ready` tolerates (retries past)
transient `Err`s from the PRE-pause polling loop for the same reason —
that loop can hit the identical general sentinel-retry race on the fifo
control plane, which this ADR's fixes also cover, but proving THAT is not
this test's job.

## 2026-07-10 — ADR-22: memwrite chunk size fix — Hatari's real limit is a 61-byte token count, not P8's pinned 256

**Status:** accepted (bug fix, not a new architecture — recorded per
`documentation.md`'s "deviates from an established/pinned value" rule,
since P8.md explicitly pinned `WRITE_MAX = 256` as "the chunk size" and a
future reader needs to know that premise was wrong)
**Context:** The live-patch demo found `write_memory`/`patch` silently
truncating writes above ~16-60 bytes: a 400-byte write reported
`verified: false`, and a 125-byte write reported success with nothing
actually in RAM. P8.md's implementation note already fixed one `memwrite`
defect (missing `$`-prefixes) but pinned `WRITE_MAX = 256` as the safe
per-line chunk size without live-testing a chunk that large. Reading
Hatari's own source (`hatari/src/debug/debugui.c`,
`DebugUI_ParseCommand`) shows the real constraint: the whole command line
is tokenized with `strtok` into a fixed `char *psArgs[64]`; a `memwrite
$addr $b1 $b2 …` line spends 2 slots on `memwrite`+`$addr`, so more than
61 data-byte tokens overflows the array and Hatari rejects the WHOLE line
with `"Error: too many arguments (currently up to 64 supported)"` — not a
partial write. This is a hard, deterministic token-count ceiling (not a
buffer-size race): confirmed live against a real Hatari 2.4.1 session, N=61
always succeeds and N=62 always fails, on every attempt. The rejection
message's lowercase "rror" also slipped past `write_bytes`'s
`.contains("ERROR")` check, so a fully-rejected write still returned
`Ok(())` — the second half of the bug.
**Decision:** `debug::write_bytes` now chunks internally
(`MEMWRITE_CHUNK_BYTES = 32`, comfortably under the proven 61-byte hard
ceiling) so every caller gets the fix automatically — no caller-side
chunking left. `tools/assemble.rs`'s `write_chunks`/`WRITE_MAX`-based loop
is deleted; `patch` now calls `debug::write_bytes` directly. The
`ERROR`-line check is now case-insensitive
(`to_ascii_uppercase().contains("ERROR")`). `tools/mem/types::WRITE_MAX`
(256) stays as-is — it is a per-*call* ergonomics cap on the `write_memory`
tool, unrelated to the per-*line* Hatari limit; any value up to it is now
safe in one call since the line-chunking happens underneath.
**Alternatives considered:** keep the caller-side `WRITE_MAX`-chunked loop
in `assemble.rs` and just shrink `WRITE_MAX` (rejected — the `write_memory`
tool guard and `patch`'s internal chunk size are different concerns wearing
the same constant; conflating them is exactly how the 256 mistake reached
production); chunk to exactly 61 (rejected — zero margin against any
future Hatari change or off-by-one; 32 keeps ~2x headroom while still
roughly halving round-trips versus the empirically-tested-safe 16 from the
live demo).
**Consequences:** A single `write_memory`/`patch` call now costs
`ceil(bytes / 32)` `memwrite` round-trips instead of one — a latency
regression judged acceptable against silently losing writes. Live
regression coverage added: `tests/write_bytes_live.rs` (a single 400-byte
`write_bytes` call and an exact-boundary 62-byte call both round-trip
byte-identical) and `tests/assemble.rs::patch_large_blob_roundtrip_no_truncation`
(a 130-byte `dc.b` patch verifies true). P8.md's own "chunk size" note is
now superseded by this entry, not edited in place (historical spec, not a
living doc).

## 2026-07-10 — ADR-23: Audio capture via Hatari's native recorder + Rust-native spectrogram

**Status:** accepted
**Context:** Agents verifying sound routines (or comparing before/after a
`patch`) had no way to observe the ST's audio output — screenshots show
the screen, nothing showed the speaker. shazz already trusts a Python
spectrogram pipeline (STEIsNotDead tools/audio: matplotlib specgram
NFFT=2048/noverlap=1536/Hanning/magma, fmax 8000, channel-mean mono mix)
whose renders he can read fluently.
**Decision:** Two tools. `record_audio` drives Hatari 2.4.1's built-in
sound recorder over the control fifo: `hatari-path soundout <abs.wav>` →
`hatari-option --fast-forward off` → `hatari-shortcut recsound` → real-
time sleep → `recsound` → `--fast-forward on`. Every command is confirmed
by a live-pinned log echo (control commands have no debugger sentinel;
the echo IS the sync primitive — soundout and hatari-option echo even on
no-op, verified live). Fast-forward MUST be suspended: Hatari records
emulated time (probed: 3 wall-seconds under fast-forward yielded 38.9 s
of ~13x-compressed garbage; real-time yielded exactly 3.00 s), and 2.4.1
has no `fastforward` shortcut token, so `hatari-option` (an idempotent
set, better than a toggle) is the mechanism. The session lock is held for
the whole window — a concurrent freeze mid-recording would corrupt the
capture. `spectrogram` reimplements the trusted pipeline Rust-native
(hound 3.5.1 + rustfft 6.4.1; embedded 256-entry magma LUT): Hann 2048,
hop 512, linear axis to fmax (default 8000 Hz), 80 dB color range pinned
to the max (deviation from matplotlib's auto-range, for determinism),
normalized over displayed bins only. The DSP lives in `src/audio/`, a pure
leaf — unit-tested with synthesized sines (bin-exact assertions), zero
emulator involvement. Token efficiency (ADR-5): WAV = path handle,
spectrogram = inline PNG + compact JSON axis facts;
`spectrogram(wav_path=…)` re-analyzes an existing recording in any session
state without touching the emulator.
**Alternatives considered:** (a) host-side audio loopback capture
(PulseAudio/pipewire) — host-config-dependent, breaks headless, races the
mixer; (b) `.ym` register capture + our own YM synthesis — loses STE DMA
sound and digidrums, large new emulation surface (still open as a later
plugin via the same soundout seam); (c) shelling to the existing Python —
new runtime dependency, violates self-containment; (d) `realfft`/colormap
crates — two more deps for marginal gain over rustfft + a const LUT.
**Consequences:** Recording N seconds blocks the (single) session for N
real seconds — the one deliberate long sleep in the codebase (it is the
product, not a synchronization hack; ADR-7 unaffected). New deps: hound
3.5.1, rustfft 6.4.1 (pure Rust, lockfile-pinned). A stop-echo timeout
mid-recording can leave Hatari's recorder armed (never observed live);
remedy is stop/start — a blind recsound retry would toggle the wrong way.
The recorder captures Hatari's final mixed output (YM + STE DMA +
crossbar), sample-exact at the configured --sound rate (default 44100),
2-ch 16-bit PCM (probed). `HatariProcess` gained one narrow primitive,
`control_and_wait` (raw control command + log-echo barrier); `shutdown`
moved from `spawn/mod.rs` to `spawn/control.rs` in the same phase purely to
stay under the 200-line file budget once `control_and_wait` landed (no
behavior change).

## 2026-07-10 — ADR-24: P12 zx0_pack_pi1 fixture correction — split inside the palette, pinned darkness guard

**Status:** accepted (amends P12.md §6.1/§6.3/§6.4 in place with "ADR-24"
markers; ADR-13/ADR-14 precedent for spec-vs-implementation corrections)
**Context:** P12 §6.1's pinned `zx0_pack_pi1` (literals `payload[..1]`;
`copy_last(1)`; literals `payload[2..]`; end) claimed "no contiguous
34-byte header survives". Implementation falsified this: `payload[2..]`
is ONE verbatim ZX0 literal run (32 032 bytes), the Degas detector scans
every even offset, and the shared fixture's `PAL[0] = 0x0000` doubles as
a low-res res word — so the run start (blob offset 6) reads as res word
`PAL[0]` + palette `PAL[1..16]`+first screen word, all passing
`plausible_palette` (≥4 distinct, upper nibbles clear). A false Degas
finding fires inside the raw blob, breaking §6.4 test 6 (Picture at
depth 1 must be empty) and test 5 (exactly 1 finding — got 2, the ghost
carrying `via == ["atomik"]`). This is the exact aliasing mechanism
ADR-13 documented for NEO (black color 0 + zero res word); the original
spec text only reasoned about the *file-start* header. Independently
confirmed by a byte-exact simulation of the §6.1 assembler (cross-checked
bit-for-bit against pinned vectors Z1–Z6), the decoder, and the
`degas.rs`/`image/mod.rs` heuristics.
**Decision:** Fix the fixture, not the assertions, and convert the prose
safety claim into a pinned test. (1) Amended construction — the copy
event splits INSIDE the palette so no surviving fragment can fill a
32-byte palette window: `literals payload[..1]; copy_last(1); literals
payload[2..12]; copy_new(3, 2); literals payload[14..]; end`, with
`assert payload[0] == payload[1] && payload[9..11] == payload[12..14]`
(the PAL fixture satisfies the copy: bytes `07 07` at 9..11 and 12..14).
Fragment 1 (PAL[0..5], 10 bytes) is poisoned by the adjacent copy_new
offset byte `0xFB` in both word parities; fragment 2 (PAL[6..16],
20 bytes, word-aligned at blob offset 18) contains no res word in either
parity and runs into the screen's invalid `0xFFxx` words. Verified: zero
try_pi/try_pc hits at ALL even offsets of the raw blob (stricter than
`scan()`'s skip-past), round-trip exact, `consumed == len`. Pinned
constants are UNCHANGED — blob = 32 031 payload + 1 offset byte + 8 bit
bytes (61 stream bits) = **32 040**; outer Atomik stays **36 062**
(`atomik_pack_literal` is content-independent in length). New bit-exact
verification vector pinned in §6.1: `blob[..18] = A1 00 39 00 00 07 00 00
70 00 07 07 70 FB 54 40 44 35`, `blob[32036..] = FF FF 55 58`. (2) The
**darkness guard** (§6.4-3): `default_registry().scan(zblob-as-MemImage,
Some(Picture), 0)` → empty. Two even offsets (14/16) are safe only by
bit-byte VALUE (`0x5440`/`0x4435` are not res words), not structure; the
guard pins that residual coincidence mechanically so any drift in the
fixture palette, `bands()`, gamma widths, or detector heuristics fails
loudly here before misdiagnosing tests 5/6.
**Alternatives considered:** Shifting the big run by one more byte or one
palette word (the natural (a)-sketch; rejected: leaves `PAL[1..16]` —
30 bytes, 15 distinct valid words — contiguous, so safety hinges entirely
on the preceding lazily-allocated gamma bit bytes not forming a res word:
the same coincidence class, unprovable by structure). A fixture-generic
zero-based split `copy_new(2, 2)` over `payload[2..4]` (rejected: splits
before the palette, same 30-byte exposure). Relaxing the assertions —
drop "empty at depth 1" or bless 2 findings (rejected per ADR-13's own
reasoning: the exact counts ARE the recursion proof; a pinned false
positive would enshrine detector noise and mask real regressions).
Hardening the detector instead (rejected: a genuinely zx0-packed PI1 in
the wild can alias the same way — that is ADR-13's accepted residual
noise class, a corpus problem, not this fixture's to solve).
**Consequences:** §6.4 tests 5/6 pass with their pinned exact counts; no
pinned length churn anywhere (§6.1/§6.3/§6.4-3/5/7 and the live test's
`consumed == 32_040` all stand). The packer is explicitly coupled to the
shared PAL fixture via its new assert — acceptable for a test-only
builder, and self-documenting. Pack-Ice/Atomik analogues (`ice_pack_pi1`
offset-3 split) never had a depth-boundary assertion pinning
intermediate-layer darkness; empirically dark today (P5/P9 exact-count
tests pass), but a follow-up may add the same one-line darkness guard to
their fixture tests. Numbering note: P12.md's internal references to
"ADR-23" (the whole-buffer-exact gate) predate decisions.md's landed
ADR-23 (audio capture); the gate decision should land as ADR-25 with
P12.md's references updated at phase close.

## 2026-07-10 — ADR-25: ZX0 depacker — headerless schemes get an exact-gated auto path plus an explicit depack tool

**Status:** accepted
**Context:** The DHS "Uhane" demo packs data with ZX0 (Einar Saukas's
format, Emmanuel Marty's decompressors) — the dominant modern
cross-platform compressor, and the first scheme in the roster with NO
magic header, no stored lengths, nothing to scan for. The ADR-2/ADR-14
recursion engine is magic-driven (scan → depack → rescan), so ZX0 cannot
be auto-located in a RAM image; and its typical payload is raw
screen/music/code data that no detector matches, so even a successful
depack is invisible unless the unpacked bytes themselves become a handle.
**Decision:** Two complementary paths, engine untouched. (1) **Auto
engine: whole-buffer-exact gate.** `Zx0::try_unpack` decodes at offset 0
only and succeeds only if the stream consumed the entire buffer and
produced non-empty output. This makes recursion work for the real nested
case (a depacked inner buffer that IS a bare .zx0 file — proven by the
atomik⊃zx0⊃PI1 fixture, via == ["atomik","zx0"]) at essentially zero
false-positive risk (0/20000 random buffers passed the gate at spec time)
and O(buffer) cost. (2) **Explicit path: the `depack` tool**
(addr, scheme, len?, depth?). The agent locates the stream (disasm,
search_memory), the server slices a window at that address, runs the named
depacker PREFIX-tolerantly via a new `Depacker::try_unpack_prefix` default
method (additive, object-safe; default = try_unpack over the whole window,
overridden by headerless schemes to return (unpacked, consumed)), then
stores results in the standard last-scan store: findings[0] is a synthetic
whole-buffer Raw finding (`Finding::whole_buffer`) carrying via=[scheme] —
so `extract` materializes the raw depacked blob, detector findings inside
it arrive with honest provenance and no fake bus address
(`Registry::scan_via` exposes the engine's existing via parameter), and
paging reuses rip's scan_id. Consumed-length is reported — ZX0 stores it
nowhere, the decoder is its only source. (3) **Version scope: ZX0 v2
forward standard only** (the format of zx0 v2.x and salvador), pinned from
einar-saukas/ZX0 v2.2 dzx0.c with fixtures generated/verified by the
reference binaries themselves. v1 "classic" (offset gamma not inverted)
and the backwards variant are deferred: headerless means variants are
indistinguishable by inspection; when a real target needs one it becomes
its own plugin id selectable through the tool's `scheme` parameter.
**Alternatives considered:** a validating heuristic detector attempting
decodes at candidate offsets (rejected: bit-granular starts make candidate
generation explosive, and any plausibility gate loose enough to find real
streams admits false positives — the explicit call is exact and costs one
round trip); windowed `rip` (addr/len arguments) instead of a new tool
(rejected: muddles rip's whole-RAM contract and cannot return the depacked
blob when no detector matches — the dominant ZX0 payload is headerless raw
data); requiring the agent to supply the unpacked length (unnecessary —
the stream self-terminates); registering nothing in the auto engine
(rejected: packer-in-packer with a bare .zx0 inner file is real and the
gate is free); a zx0 region detector (impossible without decoding —
violates the ADR-19 cheap-detector invariant).
**Consequences:** The P3-frozen `Depacker` trait gains one default method
— still object-safe, zero impact on existing impls; the registry gains two
capability lookups (unpack_prefix, scan_via — the ADR-12 render_png
precedent), not engine changes; tool count +1. "Add a headerless scheme"
is now one plugin file + one registry line, same as magic schemes; future
LZSA/Shrinkler/zx0-b ports reuse the seam unchanged. Known limits are
explicit and tested: a ZX0 stream the agent cannot locate is not found
(auto rip stays silent about arbitrary in-RAM streams by design), and
wrong-variant streams fail closed rather than mis-decode.

## 2026-07-10 — ADR-26: MPP multi-palette pictures — RECOIL-pinned change engine; unknown modes surface header-only

**Status:** accepted
**Context:** The DHS "He'e nalu Uhane" endpart displays MPP (Multi-Palette
Picture, Zerkman/Sector One) images — >16 colours via palette rewrites at
fixed x positions inside every scanline — which the single-palette P4
decoders cannot represent. The credits say "MPP mode 5", but the canonical
container (zerkman/mpp README) and its only independent decoder (RECOIL)
define modes 0–3 only; no public mode-4/5 layout exists. Two references
also disagree on stored height (README 200/276 display lines vs RECOIL
199/273 stored lines).
**Decision:** (1) One detector+renderer plugin (`mpp` + `mpp_decode`,
one registry line, ADR-2) implementing the complete public format: modes
0–3, ST 9-bit / STE 12-bit / STE 15-bit ("extra") palette entries, with the
per-line entry counts (52/46/54/48), change-position tables, and
forced-black border semantics pinned from RECOIL 6.4.5 (cited, not ported).
Heights are RECOIL's 199/273 — it length-validates real files; the README
describes displayed geometry. (2) The renderer keeps ONE 16-slot palette
that persists across the frame, reloads slots 1..=15 (mode 3: 6..=15) per
line from the continuous MSB-first bitstream, and applies in-line rewrites
while walking x, so each pixel is looked up in the palette as of its x.
(3) An otherwise-valid header with mode 4–7 becomes a header-only finding
labeled "unsupported mode" — a genuinely extended "mode 5" file still
surfaces (with its MPPH metadata extractable) instead of silently vanishing;
mode > 7 is rejected. (4) Detection is magic-anchored ("MPP" + flag/reserved
shape + MPPH-iff-extra + whole-artifact-fits) with no alignment assumption —
≥ 6 fixed bytes make Degas-style plausibility heuristics unnecessary and
false positives negligible. (5) Colour math: 12/15-bit identical to RECOIL;
9-bit uses the house v*255/7 ST scaling (RECOIL bit-replicates, ≤ 1/255
apart) so MPP and Degas render identical colours for identical palettes.
(6) Double-image files are sized fully but render frame 1 only; 15-bit
entries render as their blended colour, single frame.
**Alternatives considered:** mode 2 only (the blitter mode Uhane most
plausibly uses — rejected: the other modes share 90% of the machinery and
RECOIL parity is cheap); guessing a mode-5 layout (rejected: nothing to pin
against); rejecting unknown modes outright (rejected: hides exactly the
artifact that motivated the phase); porting RECOIL's decode verbatim
(rejected: license and style — semantics were re-derived and re-tested);
extending `decode_planar` with a palette callback (rejected: smears MPP's
change engine into the shared image layer for one consumer).
**Consequences:** `extract as=png` works for MPP with zero tool-layer
changes (ADR-12 registry lookup). A future mode-4/5 spec is a follow-up
phase touching only the plugin pair. Double-image blending and MPPH tag
parsing are recorded backlog. Fixtures are byte-pinned synthetic builders
(ADR-12 policy) whose goldens prove a mid-line palette change on the exact
pixel and cross-line palette persistence — the two behaviors that define
the format.

## 2026-07-10 — ADR-27: Interactive RE primitives — change-watchpoints, static xrefs, VBL frame-stepping

**Status:** accepted
**Context:** The live-patch exercise (docs/live-patching.md §"What was
missing") showed the tool surface forces "disassemble and reason" where a
freezer cartridge should let you "ask the machine": which of N copies of
a table is live (chased by hand through an indirect pointer), what code
touches an address (hand-scanned disassembly), and catching animated
state (poll-and-race `pause`). Reading Hatari's `breakcond.c` established
the load-bearing constraint: Hatari has **no bus-access breakpoints** —
its condition engine compares *values* at instruction boundaries
(`DebugCpu_Check` in `do_specialties`), so a "write watch" is really a
value-change watch (`(addr).w ! (addr).w` + tracking), a read watch is
impossible, and the hit PC is the instruction *after* the writer.
**Decision:** (1) `watch(addr, len)` = a managed `:once` change-tracking
breakpoint riding ADR-15's run-to-breakpoint unchanged: ensure-paused →
read old value frozen → arm → run → freeze reports hit-PC, new value, and
a best-effort backward decode of the writing instruction (`writer_at`:
the instruction ending at PC, preferring one whose operand resolves to
the watched address). Only `on="write"` exists (schema-level); the
description routes read questions to xrefs. (2) `xrefs(addr)` = pure
analysis in `analyze/xrefs.rs` (+ `xrefs_ops.rs` for op_str resolution and
`xrefs_scan.rs` for the byte-level candidate walk, split for the line
ceiling): a linear extension-word candidate scan (abs.l, abs.w, d16(pc),
d8(pc,ix) arithmetic) confirmed by nearest-first single-instruction
Capstone decodes (the ADR-16 pattern), classified read/write/ea/exec —
static "who touches this", register-indirect consumers documented as
invisible. (3) `run_frames(n)` = a managed `:once` breakpoint on Hatari's
`VBL` frame counter (`b VBL > cur+n-1`), read via `evaluate VBL` while
frozen — exactly n frames then Paused, frame-deterministic. (4) Surface:
three separate tools (the `profile` precedent — typed params/results,
`breakpoint` stays the raw-condition surface); `managed::Fire` gains
`ours: bool`, and an `ensure_paused` helper freezes-then-reseats for
pre-arm frozen reads, accepting one documented drift: an arm failure
after it leaves the session Paused rather than restored-to-Running.
**Live verification (DISPLAY=:0, real Hatari 2.4.1, MJJ demo):** all
three `// verify live` wire-format blocks in P14.md §3 confirmed correct
as pinned, with one correction and one test-fixture fix:
- §3.2's fire-block ordering (`BreakCond_UpdateTracked`'s tracking-value
  line `  $<addr> = $<new>` printing BEFORE the match-report header) and
  the dot-spaced normalized-expression tokenizer format are both exactly
  as pinned — confirmed on a real watch of `$470E8` (the MJJ scroller
  cursor documented in docs/live-patching.md §5): fired, `pc=$00AD7A`,
  writer `move.l a0,$470e8.l` decoded exactly, `exact: true`.
- §3.2's "-> Track value changes, show value(s) when matched." line is
  **arm-echo text** (a separate exchange `bp::Armed` never retains), not
  fire-block text — the spec's phrasing reads ambiguously on this point;
  `tests/re_live.rs` asserts on the fire block's actual contents instead
  (match-report header, removal line, dot-spaced `:once` expression).
- §9.4 item 4's planted-source assumption ("vasm... encodes the first as
  abs.l") was **wrong**: vasm auto-optimizes a bare numeric destination
  within pc-relative range to a d16(pc) move; the fixture now forces
  absolute-long addressing with an explicit `.l` suffix
  (`move.l $78900.l,d0`) — a test-fixture correction, not a production
  code change.
- One real bug found and fixed (not a wire-format issue; see ADR-28):
  `watch`/`run_frames`'s `output` field went through
  `breakpoint_wire::cap_output` as spec'd, which truncates to the FIRST
  2 KiB — correct for `breakpoint(set)`'s near-instant conditions, but
  wrong here, where a real-time wait can accumulate more than 2 KiB of
  harmless FIFO-poll noise before the informative match-report block
  (which always arrives at the END of the captured text).
**Alternatives considered:** patching Hatari for true access hooks
(upstream-scale project; change-tracking answers the actual questions);
emulating a read watch by single-stepping every instruction server-side
(orders of magnitude too slow over the fifo); watch as a `breakpoint`
action (schema bloat on the raw tool, against the profile precedent);
xrefs by whole-image linear disassembly (stream-desync garbage — the
candidate+confirm scan is ADR-16's proven shape); frame-stepping by
polling `evaluate VBL` from Running (the exact poll-and-race the feature
exists to kill); multi-word watch ranges (Hatari has no OR conditions —
N-breakpoint choreography deferred).
**Consequences:** "Which copy is live?" becomes one `watch` call instead
of disassembly archaeology; same-value writes are invisible and DMA/
blitter writes coalesce (documented); emulation slows while a watch is
armed (per-instruction check + debugger memory read, bounded by
timeout); watching read-side-effect hardware registers perturbs the
machine (documented warning). xrefs misses register-indirect flow by
design and can mis-address a hit by a few bytes on pathological suffix
decodes (accepted, ADR-16 class). run_frames is frame-deterministic but
not PC-deterministic. Nothing persists armed behind the FSM's back —
every exit state is compile-time-known, preserving ADR-15's invariant.

## 2026-07-10 — ADR-28: Tail-keeping output cap for watch/run_frames

**Status:** accepted
**Context:** P14.md §7.1 step 9 said to reuse `breakpoint_wire::cap_output`
(keeps the first 2 KiB) for `watch`'s (and, by the same shape, `run_frames`'s)
fired-output text. Live testing (DISPLAY=:0, real Hatari, MJJ demo, watch on
`$470E8`) showed slow-firing watches accumulate control-plane polling noise
(`"command FIFO read error: Resource temporarily unavailable"`, logged once
per idle poll of the other control-plane fifo while a wait is in progress)
*before* the informative match-report block, which always arrives at the END
of the captured debugger-entry text (confirmed: `managed::handle_fired`'s
internal `ours` detection, run on the *uncapped* string, correctly found the
match line — proving the content existed but was being discarded by the cap).
Head-capping returned ~2 KiB of pure noise with the report cut off entirely.
**Decision:** A new `cap_output_tail` (keeps the LAST ~2 KiB, cutting at the
first newline at-or-after the window start, char-boundary safe, falling back
to the raw boundary-adjusted tail rather than empty when no newline exists in
the window) lives in `src/tools/watch_wire.rs` and is shared by
`src/tools/frames.rs`. `breakpoint_wire::cap_output` is untouched:
`breakpoint(set)`'s conditions fire near-instantly in practice, so its report
sits at the head of a short string, and head-capping remains correct there —
the two tools have genuinely different capture shapes (report-at-head vs.
report-at-tail), so two small helpers with opposite semantics is the honest
model, not duplication. Ruling made by rust-architect consultation mid-phase
(three options weighed: ship as spec'd and accept a noise-only field on slow
fires; fix in place with the new tail helper; change the shared `cap_output`
itself and its pinned P2 test).
**Alternatives considered:** shipping as spec'd (rejected: wastes agent
context on a field that is deterministically zero-signal on exactly the
tools whose defining trait is "waits a while"); changing the shared
`breakpoint_wire::cap_output` to keep the tail everywhere (rejected: a
behavior change to already-shipped P2 code outside P14's declared file
tree, fixing a problem `breakpoint(set)` doesn't have, and would require
rewriting that function's own pinned unit test).
**Consequences:** Two output-cap helpers with opposite truncation direction
coexist in `tools/`, each matching its tool's real capture shape; a doc
comment on each cross-references the other so a future reader isn't left
guessing why. Follow-up (explicitly out of scope here, P2/session-layer):
consider suppressing the FIFO-poll noise line at its source in
`src/session/spawn/control.rs`, which would shrink every long capture
regardless of which cap direction is used — not needed for P14 since the
tail cap already makes `watch`/`run_frames` correct without it.

## 2026-07-10 — ADR-29: cheat_scan splits wire-in/wire-out — `cheat_out.rs` joins the P15 file tree (insn_out precedent)

**Status:** accepted
**Context:** P15.md §2 pinned `cheat.rs` ≤180 / `cheat_wire.rs` ≤190 non-test
lines and noted no further split was expected. Measured implementation landed
at 286 / 200: the spec-pinned 11-field `CheatScanOutput` (§7.3) constructed
per-action, the §7.1 per-field validation matrix, a ~14-line tool description,
and this project's default-`use_small_heuristics` rustfmt (which explodes any
3+-field struct literal and 4+-arg call to one item per line) make the
intrinsic content of the tool ~420+ formatted lines — over the 2×200 budget of
a two-file layout, and irreducible below it without breaking the harder
≤30-line-function rule or duplicating logic. Ruling by rust-architect
consultation mid-phase.
**Decision:** (1) Collapse the three per-action output builders into one
`page_output(action, &CheatStore, offset, sample, has_more)` — they shared 8
identical field assignments; filter mutates `prev_count`/`predicate` after,
`new` builds its `CheatStore` *before* storing so the builder can read it,
`reset` keeps its inline struct-update literal. §7.3's per-action field
population is unchanged on the wire (`skip_serializing_if`). (2) Replace the
16 scattered `forbid()` calls with one pure `validate_params(&CheatScanArgs,
Action)` matrix validator in `cheat_wire.rs` (allowed-set per action, also
enforces filter's required predicate, names the offending field per §7.1) —
unit-tested per §9.3's contemplated param-matrix test. (3) Add
`src/tools/cheat_out.rs` — a straight extraction of the output side
(`CheatScanOutput`, `CandidateOut`, `page_output`, `page_candidates`,
`format_value`, `predicate_str`, sample-cap consts, and their unit tests),
leaving `cheat_wire.rs` the input side (args schema, `CheatStore`, parsers,
validator, pure geometry/region helpers). This is the established pattern:
`insn_out.rs` was split out of `analyze_wire.rs` for exactly this reason
(plan-pinned two-file layout measured over the ceiling), so P15 §2's "the
permitted split is the established sibling pattern" is satisfied, not
deviated from. The `cheat_filter`/`apply_filter` dispatcher/worker split is
retained on structural (not line-count) grounds: the sync worker makes
holding the std `cheat` mutex across an await unrepresentable (§6).
**Alternatives considered:** shipping `cheat.rs` at 286 with a rationale
comment (rejected: the ≤200 gate is the blocking global rule and a
precedented clean split exists); shifting validation into the already-full
`cheat_wire.rs` alone (rejected: relocates the overage); trimming the wire
schema's remaining doc comments (rejected: they flow into the MCP JSON
schema the agent reads — self-description is load-bearing); redesigning the
§7.3 output schema (rejected: spec-pinned).
**Consequences:** All three files land under 200 non-test lines with
headroom (~185 / ~160 / ~110); every function stays ≤30 lines; §7.1's matrix
becomes one declarative, unit-testable function instead of 16 call sites.
P15.md §2's file tree and §7.2's re-export line are amended in the same
commit; `mod.rs` registers `cheat_out`; `cheat_out.rs` carries an
insn_out-style module doc stating the extraction rationale. Future
action-param tools with wide pinned output schemas should budget three files
(tool / wire-in / wire-out) from the start rather than rediscovering this.

## 2026-07-10 — ADR-30: save_prg — persist a live hack by re-emitting the patched .PRG (repatch flavour; snapshot deferred)

**Status:** accepted
**Context:** The live-patch exercise (docs/live-patching.md) ends with
nothing to keep: patches live in volatile RAM and vanish on `stop` — the
one freezer-cartridge power the surface lacked was "save". Two classic
flavours exist: map patched TEXT/DATA RAM back onto the original .PRG's
file offsets (persists static-data/code hacks — exactly what the exercise
patched), or dump full RAM+regs+hardware behind a 68k restore stub (the
r1pp3r/Ultimate-Ripper model — persists anything, costs a code generator
and hardware-state semantics).
**Decision:** Ship the re-emit flavour as `save_prg(out_path, source_prg,
mode="repatch")`; reserve `mode="snapshot"` (rejected with
invalid_params) for a follow-on phase. Mechanics: (1) the live load
address comes from the GEMDOS basepage via OSHEADER — `$4F2` → real OS
base (+8) → version gate (≥ 1.02) → `p_run` (+$28) → basepage →
p_tbase/p_tlen/p_dbase/p_dlen/p_bbase/p_blen (layout authority: r1pp3r's
FIX_CURR_BPAGE; TOS < 1.02 font-heuristic out of scope); (2) the original
.PRG is parsed (28-byte header per the shipped prg_wrap table — a
parse↔prg_wrap round-trip test pins them together — plus the standard
delta-encoded relocation table), the expected RAM image is reconstructed
by replaying relocation at the live load address, and actual RAM (one
ranged savebin of TEXT+DATA) is diffed against it; (3) diffs OUTSIDE
relocated longwords are written to a byte-identical copy of the source
file at offset 28+seg_off; diffs INSIDE relocated longwords and everything
in BSS are NOT file-representable — reported (values + offsets), never
persisted, with BSS declared statically unreachable; (4) the tool is
Paused-only (a persistence op must snapshot a frozen machine; pause is one
call away), validates both paths (canonicalize, regular-file, extension
family, no `..`, refuse out==source), cross-checks header
tlen/dlen/blen against the live basepage (catches wrong-file and
packed/self-relocating programs), and warns when divergence exceeds 10%
(the program's own DATA writes also diff — an optional regions allow-list
restricts what is applied); (5) output is a compact summary — capped
run/skip lists, counts, paths; never bytes (ADR-5).
**Alternatives considered:** Snapshot-to-runnable first — general but a
full phase of restore-stub engineering; it must not gate the simple save.
Searching RAM for the PRG's byte prefix instead of reading the basepage —
ambiguous (multiple copies; the live-patching exercise itself hit 3 copies
of a palette). Momentary-freeze-on-Running (ADR-20 style) — rejected:
saving is deliberate, and Paused-only keeps the tool thin. Rewriting the
reloc table to re-point patched pointers — deferred improvement;
report-only cannot corrupt. Emitting via prg_wrap re-serialization —
rejected: copy+patch preserves symbols/fixups/flags byte-for-byte by
construction.
**Consequences:** Static TEXT/DATA hacks (scroll text, palettes, tables,
in-place code) survive `stop` as a clean, still-relocatable .PRG identical
in size to the original. Runtime state does not — BSS and patched
relocated pointers are reported honestly instead of being silently baked
in or silently dropped. The diff cannot distinguish agent patches from the
program's own DATA-segment writes; the divergence warning + regions
filter put that judgement where it belongs (the agent). Basepage
discovery trusts p_run (current process) — a Pexec'd child fails loud via
the header cross-check. The reserved snapshot mode and reloc re-pointing
are the improvement paths. Two live-behavior corrections surfaced during
implementation and are recorded as their own ADRs rather than edited into
this one: ADR-31 (os_beg's bounds check) and ADR-32 (the live acceptance
test's target, since the sanctioned smoke PRG turned out to be packed).

## 2026-07-10 — ADR-31: basepage discovery — os_beg is bus-bounded, not RAM-bounded (live correction of P16 §4.3 row 2)

**Status:** accepted (live-behavior correction of a `// verify live`-flagged
spec value — same precedent as ADR-22's memwrite ceiling and ADR-9's live
addenda; P16.md §4.3 row 2 and §5.2's `hop` signature are superseded by
this entry, not edited in place)
**Context:** P16 pinned one uniform pointer-hop bound for basepage
discovery: zero or `>= ram_len` ⇒ `Error::Parse`, applied identically to
sysbase, os_beg, and p_run — with `// verify live` on the os_beg row. Live
verification (Hatari, `--machine ste --memsize 1`, TOS 1.62 FR,
MJJHACK.PRG) disproved it: `$4F2` → `$940` (a RAM-resident shadow of the
OS header; raw dump `60f8 0162 00e0 0030 00e0 0000` is a well-formed
OSHEADER), and `8(sysbase)` → `$E00000` — the real OSHEADER in ROM, whose
version word reads `$0162`, exactly matching the booted image. On every
ROM-resident TOS the definitive OSHEADER lives in ROM ($FC0000 for TOS
1.00–1.04, $E00000 for 1.06+/EmuTOS ROM); the `8(a1)` dereference (r1pp3r
`FIX_CURR_BPAGE`) exists precisely to escape the RAM shadow to that ROM
header. RAM-bounding os_beg therefore rejects the normal case on real TOS
images and broke discovery entirely against the MJJ demo.
**Decision:** os_beg is validated against the 24-bit ST bus — nonzero and
`< Addr::BUS_LEN` (`0x0100_0000`, a new shared constant that also replaces
the literal ceilings in `Addr::parse` and `tools/watch.rs`). sysbase,
p_run, bp_addr, and all basepage geometry checks stay `ram_len`-bounded
(they are genuinely RAM-resident; confirmed live). `hop` stays the single
shared validator but takes `(ceiling, region)` so the error message stays
truthful: `"{name} = ${v:x} outside {region}"`, context
`"basepage discovery"` unchanged.
**Alternatives considered:** ROM-window allowlist ($E00000/$FC0000/cart)
— rejected: over-fits known layouts, breaks RAM-loaded TOS / EmuTOS PRG
builds (where os_beg legitimately IS in RAM), and adds no safety since
the chain re-anchors to RAM immediately after (p_run and bp_addr bounds,
version gate at os_beg+2). Dropping os_beg validation entirely — rejected:
loses the loud-failure property. Treating $E00000 as a Hatari quirk to
special-case — rejected: this is documented real-TOS behavior, not
emulation.
**Consequences:** Discovery works against ROM TOS (the common case) and
RAM TOS alike; garbage os_beg still fails loud as `Error::Parse`. A new
pinned test world mirrors the live machine (sysbase in RAM, os_beg
$E00000, version/p_run reads at $E00002/$E00028) alongside the existing
os_beg-in-RAM world; a new `os_beg_beyond_bus_rejected` test pins the
ceiling. Three files touched (`debug/basepage.rs`, `types/addr.rs`,
`tools/watch.rs`) — the `watch.rs` change is literal-to-constant only, no
behavior change. Confirmed live 2026-07-10 against Hatari 2.4.1 + the MJJ
demo (`MJJHACK.PRG`, TOS 1.62): the corrected six-read sequence, the
TOS-version gate, and the basepage→text_base relationship all held exactly
as specified — the p_run-semantics `// verify live` residue is discharged.

## 2026-07-10 — ADR-32: P16 live target — MJJHACK.PRG is UPX-packed; happy path runs against a staged STRGOOSE copy

**Status:** accepted
**Context:** P16 §9.6 pins the live `save_prg` round trip against
`AUTO/MJJHACK.PRG` (the ADR-8 smoke target). Live evidence (2026-07-10)
shows MJJHACK.PRG is **UPX-packed** (UPX 4.22 banner in the file; header
35744/0/265564 vs live basepage 161502/0/139806 — the header describes the
stub, not the unpacked program). `check_match` (§7.1 step 5.2) correctly
rejects it, and packed programs are explicitly out of scope (§12). The
spec's pinned target therefore cannot execute the spec's pinned success
scenario — a spec defect, recorded here per the ADR-22/ADR-24 convention
(plan docs stay as written; the ADR carries the fix). The pure unit tests
prove parser↔repatch *internal* consistency only; the claim "file bytes +
relocation at the live load address == what real TOS put in RAM" has no
oracle except a live run of an unpacked program, so dropping the live
happy path outright was rejected.
**Decision:** `tests/save_prg_live.rs` carries two `#[ignore]`d tests.
(1) `save_prg_persists_a_text_patch` (§9.6 steps 2–8 unchanged) boots a
**staged read-only copy** of the MJJ build tree (`stage_strgoose_harddrive`:
copy `$HOME/projects/MJJ/build/` into the test work_dir — root data
files included, the demo GEMDOS-loads them — then delete the copy's
`AUTO/MJJHACK.PRG` so `STRGOOSE.PRG`, confirmed unpacked with an
~8.5 KB reloc table, is the sole auto-run `*.PRG`); `source_prg` is the
staged copy. The MJJ tree itself is never written (ADR-8 upheld). (2)
`save_prg_rejects_packed_program` boots the untouched MJJ tree (MJJHACK)
and asserts the Paused gate, live basepage discovery and RAM acquire all
succeed, and that `save_prg` fails with a message containing "does not
match" — a permanent live regression test of the §12 packed-program safety
net against a real packer. (3) Pinned fact: **MJJHACK.PRG is UPX-packed** —
any future phase needing file↔RAM correspondence must not use it as the
file-side source.
**Alternatives considered:** Narrowing the live test to rejection-only —
rejected: leaves the TOS-loader correspondence assumption (file bytes +
reloc == live RAM) unverified live, which is the one thing pure unit tests
structurally cannot prove (a shared bug in both parse and repatch would
stay invisible). Shipping a vasm-built minimal PRG fixture (ADR-8 §2
precedent; vasm already a dev-adjacent tool per ADR-18) — retained as the
documented fallback if the staged STRGOOSE copy ever fails to boot
standalone; any such fixture must carry ≥1 absolute self-reference so its
reloc table is non-empty. Rebuilding MJJHACK unpacked — modifies the MJJ
project, banned by ADR-8.
**Consequences:** The happy path is proven live against a real, complex,
relocated binary loaded by real TOS (not a synthetic fixture); the
packed-rejection path is proven live against a real UPX-packed program.
Per-run cost is a ~1 MB tree copy (staged under the test's own work_dir,
cleaned per run). Confirmed live 2026-07-10: both tests pass end-to-end
(`save_prg_persists_a_text_patch`: patch byte-verified at its exact file
offset in the re-emitted PRG, source untouched, output size == source
size; `save_prg_rejects_packed_program`: Paused gate + basepage discovery
succeed, save fails with the documented cross-check message). If STRGOOSE
ever drifts from the MJJ tree, the staging helper fails loudly at copy
time, not mid-emulation.

## 2026-07-10 — ADR-33: ym_state — non-intrusive YM2149 readout via `info ym`, no freeze

**Status:** accepted
**Context:** "What is the sound chip playing?" previously took raw `debug`
calls plus manual math. Reading the YM through the bus is destructive:
register N is read by writing N to the $FF8800 select port — a side effect
on the running player. And machine_state's precedent (ADR-20) suggested a
momentary freeze for coherence.
**Decision:** Read Hatari's emulated register file with the debugger command
`info ym` (psg.c PSG_Info: a pure fprintf of PSGRegisters[16]; registered in
debugInfo.c as a non-interactive info subcommand). One sentinel-synced
round-trip; $FF8800 is never written; the session is never paused, resumed,
or otherwise mutated — verified live 2026-07-10 (Hatari 2.4.1, MJJ demo):
registers kept advancing between two probes and audio continued normally.
NO momentary freeze, deliberately deviating from ADR-20's pattern: a single
remote debugger command executes with the emulated CPU parked between
main-loop slices, so all 16 registers come from one instant by construction;
freezing would add two sentinel round-trips and pause/resume failure
choreography for zero coherence gain (pause parks the CPU at an arbitrary
instruction — possibly mid-way through a player's register-write burst —
which is no better an instant). Decode is pure (analyze::ym, the ym2149.py
math: freq = 2e6/(16·period), the 16-level log volume table, envelope
freq = 2e6/(256·period), active-low mixer); the tool reports R0–R13 only
(R14/R15 are I/O ports, not sound).
**Alternatives considered:** (a) poke $FF8800 select + read + restore while
frozen — 3+ round-trips, still a hardware poke, and the current selection is
not recoverable through the bus (reading $FF8800 returns data, not the
select value); (b) read the player's RAM shadow — player-specific,
unreliable; (c) momentary freeze à la ADR-20 — rejected as above.
**Consequences:** ym_state works identically on running and paused sessions
and cannot corrupt program state (nothing is written, nothing is paused).
Known accepted limitation: a timer-driven player could in principle be
mid-register-burst at the instant the command executes; a freeze would not
fix that either. The debug layer gains ym_registers/parse_info_ym (the
first `info` subcommand the codebase consumes — the same parse-family
contract as `m`/`cpureg`); analyze gains its first audio-domain decoder.
Confirmed live 2026-07-10 against Hatari 2.4.1 + the MJJ demo
(`tests/ym_state_live.rs`): a running-session poll landed a snapshot with
channel B audible via noise-only (mixer $2A, tone off/noise on) — the exact
live-probe shape pinned in P17.md §2.2 — registers drifted across
subsequent reads (no self-corruption), `record_audio` captured non-silent
output after N register reads, and two paused reads returned byte-identical
`registers_raw` (a frozen chip is stable, and the read itself has no
side effect). Tool count: 33 → 34.

## 2026-07-10 — ADR-34: STE bus-contention model sharpens the static cycle estimate

**Status:** accepted
**Context:** P2's static 68000 estimate is contention-blind; on the ST(E)
the Shifter steals bus slots during the 200 visible scanlines (~12
cyc/word vs ~8 in the 113-line VBlank — STEIsNotDead's proven
`ste_model.py`, via the m68000-cycles skill), so blind numbers understate
visible-region cost by up to 50%. The measured Hatari-profiler path
already includes real contention; only the estimate needed sharpening.
**Decision:** Split each instruction's ST-rounded cost into bus cycles
(4·instruction-words + operand data accesses, clamped to the total;
classifier mirrors the decode dispatch) and internal cycles; scale only
the bus share by a display-region factor — visible ×3/2 (the physical
12/8), vblank ×1, mixed ×413/313 (scanline-weighted). A static range
cannot know its display region, so `profile` takes `region` (default
`visible` = worst case, ste_model.py's own UNKNOWN policy) and returns
blind + contended side by side — a bracket, not a single adjusted number.
Classifier gaps fall back to fully-contended (exactly ste_model.py's flat
behavior), never to no-contention. Per-instruction `cycles_est` in
`disasm` stays blind (per-insn contended values would be false precision).
**Alternatives considered:** ste_model.py's flat ×1.45-on-total (simplest,
but adds ~45% to mul/div/shift internal cycles that never touch the bus;
1.45 is demonstrably just 1.5-on-bus diluted by a ~10% internal share);
extending `decode*.rs` to return (total, bus) tuples (more invasive across
three at-ceiling files, same result); inferring the region from the VBL
counter at profile time (measured path already answers that; a static
estimate is a what-if tool).
**Consequences:** `profile` output gains `static_cycles_contended`,
`static_bus_cycles`, `region` (and `vbl_fraction`'s no-measurement
fallback becomes the contended value); blind semantics and the measured
path are unchanged. The bus/total ratio doubles as a "how bus-bound is
this routine" signal. Prefetch refill at branch targets is deliberately
unmodelled (≤8 cycles, inside the model's ±10% honesty band). The §5.2
classification table omitted a row for shift/rotate instructions (0xE);
ruled by the architect during implementation: register-form shifts never
touch the bus (Dn only), memory-form shifts (`size==3`) are a single word
RMW — 8(1/1) per the M68000UM, matching the Scc/tas/ALU-mem RMW idiom
used everywhere else in the classification. `tools/profile_wire.rs` is a
new file (`ProfileArgs`/`ProfileOutput`/`build_output`/region plumbing
moved out of `analyze_wire.rs`/`analyze_profile.rs`, insn_out.rs
precedent) purely to keep both split files under the line ceiling.

## 2026-07-11 — ADR-35: Corpus-derived YM player fingerprints; invented seeds dropped

**Status:** accepted
**Context:** P6's signature DB (ADR-16) shipped provisional seeds pending
extraction from the labelled SNDH corpus. That corpus is unavailable (no
SNDH-archive download; both local sndhfix checkouts are empty submodules),
but Atari_ST_Sources contains labelled replay binaries/sources for several
mainstream drivers. Two seed entries could not be justified from any real
source, and one ("QUARTET" ASCII) was shown to collide with demo scrolltext.
**Decision:** (1) A pinned extraction methodology (P19.md §4.1: scan
labelled samples for PSG operand bytes, decode the recurring gadget, mask
relocated/varying words, cite file@offset, test against the pinned excerpt)
— future drivers are added the same way. (2) The DB moves to a data-only
`ym_sig_db.rs` (matcher untouched; add a driver = one entry + one test);
nine entries, each corpus-cited: Hippel/Mad Max movem-blast family (x3) +
TFMX data magic, maxYMiser init gadget + gwEm banner string, TCB module
magic (x2, "!" variant still provisional), Reservoir Gods timer chain
(hand-assembled from source, verify live). (3) The invented hippel-code
pattern and the quartet magic are deleted — honesty over coverage; the
unknown -> ("unknown", 0.25) degradation contract is unchanged, so a dead
or missing signature can never produce a wrong name.
**Alternatives considered:** keep the invented seeds alongside (rejected:
confident wrong guesses); ship an extraction helper binary (rejected: a
build target for an offline, few-times-a-year task — the recipe + a 15-line
script in P19.md suffice); fetch the SNDH archive (out of reach per brief).
**Consequences:** Lucas/Quartet, David Whittaker/Delta/Music Mon and the
Mug UK SNDIMAGE rips currently degrade to "unknown" (documented gaps).
Discovered engine gap, out of scope here: those SNDIMAGE rips reach the PSG
via 24-bit `lea $FF8800.l` operands (bytes `00 FF 88 00`), which
`ym_player::is_psg` does not recognize — backlog follow-up on the anchor
scan. Hippel confidence re-pins from 0.70 to 0.82 in test expectations.

## 2026-07-11 — ADR-36: rip_sample — STE DMA-PCM ripping is register-driven, momentary-freeze, WAV-out

**Status:** accepted
**Context:** DHS "Uhane"-class demos play digi-music through the STE DMA
sound engine ($FFFF8900+, signed 8-bit PCM streamed from RAM) — invisible
to identify_music (YM replay anchors) and ym_state (PSG registers). The
frame start/end registers point directly at the playing buffer, so
extraction can be deterministic.
**Decision:** A dedicated `rip_sample` tool: one `m $ff8900 34` block read
(debug::read_bytes — debugger reads bypass the IoMem handlers, so the read
is side-effect-free raw-IoMem; start/end and $FF8921 are authoritative
there, $FF8901's play bit can read stale-1 after a finished one-shot, the
frame counter is informational only — pinned from hatari dmaSnd.c/
stMemory.c/ioMemTabSTE.c and verified live), decoded pure in
analyze::dmasnd, then a ranged savebin of [frame_start, frame_end) —
both under a momentary freeze (ADR-20 pattern; deliberately NOT ADR-33's
no-freeze, because this is two dependent round-trips and loop-mode players
re-point the frame registers every end-of-frame interrupt). Output is a
16-bit signed WAV (i16 = i8 × 256, stereo = STE even-left interleave)
written by audio::wav::write_pcm8 so wav::read_mono/spectrogram consume it
directly; the tool returns a path handle + rate/length/stereo/peak facts,
never bytes (ADR-5). DMA-off/no-buffer/wrong-machine are results
(status "none"/"stopped" + reason), never errors; force=true rips a
valid-but-stopped buffer.
**Alternatives considered:** (a) `info dmasnd` — exists in 2.4.1 but reads
the same raw IoMem and needs a new parser: rejected; (b) a RipKind::Sample
RAM-scan plugin — headerless PCM has no signature, a false-positive
machine: deferred honestly, and rip_sample is not a RipPlugin at all
(the plugin contract is pure over MemImage; this needs live registers);
(c) 8-bit WAV output — WAV's 8-bit is unsigned, a second convention and
unreadable by our own PCM16-only reader: rejected; (d) no freeze (ADR-33
style) — wrong, multi-round-trip coherence is the point here.
**Consequences:** the last DHS-"Uhane" gap closes: DMA digi-music is
rippable in one call and composes with spectrogram/record_audio.
analyze gains its second audio-domain decoder (dmasnd); audio::wav gains
its first writer. The freeze choreography is duplicated from machine_state
a second time — a third consumer should extract it. Falcon crossbar
(16-bit, record path) is out of scope; frame-counter readback is
best-effort by emulator design. Live facts (2026-07-11, staged
`AUTO/UHANE.PRG` + `music1`/`music2`, machine=ste, memsize=4): the §12.1
scratch probe against the YM-only MJJ demo confirmed the $FF8900 block
reads cleanly (three 16-byte dump lines, no bus-error warning, byte-
identical across two reads) and decodes to `BufferCheck::Empty` →
status "none" while no DMA sound is programmed. Against Uhane, the DMA
engine reports control `$03` (play+loop), mode `$81` (mono, rate index 1
→ 12517 Hz), frame window `$208D72..$382070` (1,544,958 bytes,
duration ≈123.4 s) — ripped WAV independently verified via Python's
`wave` module: mono/16-bit/12517 Hz, peak sample at full scale (±32768,
from an STE i8 extreme × 256), 99.2% of samples nonzero (audibly non-
silent), and consumed successfully by `spectrogram(wav_path=…)`. Tool
count: 34 → 35.

## 2026-07-11 — ADR-37: Picture findings are structure-scored, ranked, and capped — the Degas gate stops trusting the header

**Status:** accepted
**Context:** The DHS "Uhane" run returned 45 "Degas PI1/PI2" false positives
from `rip(kind=picture)`. The Degas header is two weak facts (res word 0-2 +
`plausible_palette`'s 32 bytes), which any table of small BE words in RAM
satisfies; the 32000-byte "screen" behind it then decodes as planar noise.
Reference decoders (RECOIL, Atari_ST_Sources DEGAS.C) identify Degas by
filename extension + exact length — signals a RAM scan does not have, so
precision had to come from the pixel data itself.
**Decision:** A pure scorer (`image::score::score_screen`) computes, from 25
sampled scanlines and strided byte samples: used-color count, horizontal
index-coherence above the per-resolution random baseline (1/2^planes),
vertical byte self-similarity at 1- and 2-line strides (the 2-line arm keeps
ordered dither alive), sampled byte entropy, and dominant-color share. Three
gates reject blank bodies, structure-free noise, and high-entropy packed
data; a weighted composite (0-100) lands in a new `Finding.confidence:
Option<u8>` (machine-rankable, unlike the ym-player label precedent) and on
the wire as `FindingOut.confidence`. Degas gates AND scores; NeoChrome only
scores (its six-word geometry gate already carries the precision); MPP is
untouched (magic + exact size math). `rank_and_cap` (finding.rs) orders each
plugin's survivors confidence-descending and caps Degas at 8 per buffer —
ranking stays *inside* the plugin so the engine's registration-order
guarantee and `FindingId` stability are untouched; cross-plugin comparison
happens agent-side via the confidence field. Thresholds (v 0.04/0.10,
h_excess 0.05, entropy 7.5, cap 8) are source-derived estimates marked
`// verify live` — to be tuned against the Uhane RAM once the emulator is
free.
**Alternatives considered:** cross-bitplane correlation (stronger signal,
~4x cost, unpinnable without live data — deferred); global confidence
re-sort in the rip tool (breaks the documented per-registration ordering and
risks tie-flips in existing tests for no agent-visible gain); content-hash
dedupe (redundant — skip-past-hit already prevents overlaps, the cap absorbs
the rest); a minimum-entropy gate (would reject legitimately blank/flat art).
**Consequences:** Blank-screen-behind-a-palette — previously an accepted
finding — is now rejected by design (G1), and the degas unit fixtures grew
structured bodies to match. Real fixtures (bands PI1/PC1, NEO, packed-PI1
recursion across all five depackers) are pinned unregressed by
tests/picture_precision.rs and the existing suites. A pathological RAM full
of palette-like words costs ~20k ops per gate-passing candidate (measured
envelope: 45 candidates on the real run — sub-millisecond); no staged gate
until live data says otherwise. degas.rs split (degas_rle.rs) was a
prerequisite pure move for the 200-line budget. `tests/picture_precision.rs`
uncovered a fixture-arithmetic erratum during implementation: the spec's
first-draft Uhane fixture (512 KiB, sites spaced 24576 bytes apart) placed
candidates closer together than one candidate's real footprint (`PI_TOTAL`
= 32034 bytes), so adjacent sites' scored windows bled into each other and
a planted real picture landed inside a neighboring accepted finding's
skip-past-hit span; corrected to 768 KiB / 32768-byte spacing (non-
overlapping) — a fixture-only fix, not a scorer or gate change.

## 2026-07-11 — ADR-38: SNAP1 terminal-gap deficit — 0x03 redefined as implicit zero-fill to the input cursor, then raw tail

**Status:** accepted
**Context:** P22 implementation proved (symbolically and empirically) that the
SNAP1 codec as first specified (P22.md §4.3/§4.4) cannot pack its own
textbook case. The per-run in-place condition for a zero run rewrites to
"stream bytes remaining after the run ≤ output bytes remaining", and each
downstream run contributes literal +4, zero/skip 4−n, old `0x03` tail +1 to
the deficit. Corollary: the *last* zero run in any stream always fails —
by exactly 1 when tail-merged — because nothing downstream can bank against
the terminator's marker byte. The deficit is invariant under budget slack
(candidate fix "budget−1" is ineffective; the inequality has no budget term)
and under splitting the run (moving k zeros into the tail grows P by k and
shrinks the LHS by k). Relaxing the check to `o+n ≤ i+1` is unsound: the
last write lands on the unread next opcode byte, turning `0x03` into `0x00`.
The only in-spec recourse, demoting the whole terminal gap to literal,
cascades to `Dense` for the canonical half-zero image — and real ST RAM ends
in exactly this shape (screen memory at top of RAM, free-store gap below).
**Decision:** Redefine `0x03` normatively: *zero-fill output until the
output cursor reaches the input cursor, then copy remaining input to region
end*. Strict generalization (fill is empty when o == i), no new opcode, no
length field. Safety is unconditional: the fill writes only `[o, i)`,
strictly below unread input; the copy then proceeds lockstep from o == i.
`verify_inplace`'s per-run conditions are unchanged (`o ≤ i` for the tail
already implies fill ≥ 0); only `unpack` and stub.s `.tail` gain the fill
loop (+4 instructions / +8 bytes; §10.2's 576-byte hole pin re-verified
after reassembly). Packer: the terminal `0x03` absorbs an immediately
preceding Zero run into the implicit fill (regardless of ZERO_MIN — the
fill costs 0 stream bytes); the demote-to-literal retry loop stays as the
backstop for interior zero runs. Fill length is self-determined by payload
staging: `(R − L_tail) − o`, exactly the absorbed gap.
**Alternatives considered:** reserve 1 byte of budget (ineffective — deficit
is budget-invariant); relax the zero-run inequality by 1 (unsound, clobbers
the unread terminator); partial run split at the failure point (deficit
provably split-invariant); a new combined opcode `0x04 len:u24 + tail`
(sound and equivalent, but costs an opcode, a length field, and a bigger
stub delta for no extra generality); document demote-whole-run as a known
limitation (rejected: the failure mode is not 1 byte but the whole terminal
gap, which makes typical half-zero RAM — the phase's core promise —
unpackable).
**Consequences:** The terminal gap becomes free (0 stream bytes instead of
4), images ending in zeros — which also failed by 1 under the old rule —
now pack, and every earlier zero run gains `fill − 1` bytes of slack.
`stub.bin` must be reassembled and recommitted; §4.3's opcode table, §4.4's
safety argument, and §5.2's stub source in P22.md are amended to match. The
codec remains packer-proved: no decoder-side checks were weakened.

## 2026-07-11 — ADR-39: save_prg mode=snapshot — freeze the machine into a self-restoring .PRG (SNAP1 + $1C0 hole)

**Status:** accepted
**Context:** ADR-30 shipped save_prg as repatch-only and reserved
mode="snapshot": repatch cannot capture BSS/runtime/hardware state and
needs the original file. The freezer-cartridge save (Ultimate Ripper /
Action Replay; in-repo blueprint refs/orionfuzion/r1pp3r) needs the whole
frozen machine in one runnable artifact. The structural obstacle: a .PRG
that restores all of RAM overlaps the RAM it loads into and the code that
does the restoring.
**Decision:** Extend save_prg with mode="snapshot" (ADR-30's reserved
vocabulary; grouped-verb precedent) rather than a sibling tool. Capture
under one Paused-session lock: full RAM (ADR-9 savebin), cpureg, info ym
(ADR-33), NEW info mfp (internal-struct dump — timer reload values, which
IoMem cannot provide), and IoMem blocks for video/memcfg/STE-DMA-sound
(debugger IO reads return last-written values — precisely what a restore
stub must rewrite). Emit = position-independent two-stage 68k stub
(committed stub.bin, vasm source + drift-guard test) + SNAP1-packed RAM
image. SNAP1 is a purpose-built zero-run/literal/skip/raw-tail codec whose
Rust packer PROVES in-place decodability at pack time (`verify_inplace`);
the terminal-run `0x03` opcode is an implicit zero-fill-then-copy (ADR-38),
so a trailing compressible gap costs zero stream bytes instead of forcing
a whole-region literal. An interior zero run that would overflow its local
safety margin is demoted to a literal (ADR-38: partial splitting cannot
help — the margin deficit is invariant under the split point — only
dropping the run's stricter `o + len ≤ i` check to the literal's `o ≤ i`
fixes it). Restore order: Super → mask → hole block to $1C0 → payload
staged high (r1pp3r backward copy) → decode 0→R (zero-filling/copying per
ADR-38) → memcfg/video/palette → YM 0-13 → MFP
(stop→clear→static→reloads→control→IER/IMR) → STE DMA sound → USP → RTE
frame below frozen SSP → movem → RTE at the frozen PC. The wire output is
an untagged `SavePrgOutput` enum (`Repatch`/`Snapshot`, discriminated by
shape); rmcp's `#[tool]` macro requires every output schema's JSON-Schema
root to carry `"type": "object"` (MCP spec, checked at router-build time),
which schemars' derive does not emit for an untagged enum (it emits a bare
`oneOf`) — `SavePrgOutput` hand-implements `JsonSchema`, generating from a
schema-only mirror type and adding `"type": "object"` alongside the
derived `oneOf` (both variants are objects, so this is exact, not a lie).
**Alternatives considered:** sibling tool (breaks the ADR-30 contract);
uncompressed used-extent capture (cannot restore dense regions, no
integrity story); ZX0 for the payload (in-repo ZX0 is a depacker; writing
an optimal packer is a phase of its own and its in-place delta is subtler
than SNAP1's constructive rule); stub residue below frozen SSP instead of
$1C0 (SSP position is untrusted input; the vector hole is fixed,
r1pp3r-precedented — crash page $380 — and never hardware-raised on ST(E));
a derived `#[serde(tag = "mode")]` internally-tagged output enum instead of
untagged (schemars *does* emit `"type":"object"` for that shape, sidestepping
the rmcp issue entirely, but the wire JSON would carry a synthetic `mode`
discriminator field the two flavours' own shapes already make redundant —
`changed_bytes` vs `entry_pc` distinguish them for free).
**Consequences:** Residue is bounded and ALWAYS reported: ≤576 bytes at
$1C0..$400 (566 bytes in the committed stub) + 6 bytes below frozen SSP.
Honest non-restores are a fixed list in every output (beam position, MFP
timer phase + pending interrupts, IKBD mode, FDC/DMA in flight, blitter,
DMA-sound position, microwire). Programs pointing MFP VR at the hole, or
with live data in the crash-page vectors, resume corrupted — detected
where possible and warned. A too-dense RAM image is an honest error, not a
broken file (ADR-38's fix substantially shrinks how often "too dense"
actually triggers for realistic half-zero RAM). The stub is a committed
binary artifact: reviewable assembly source + byte-equality drift test
replaces build-time assembly (no vasm needed to build the crate). The
`SavePrgOutput`/`RepatchOutput` split (moved out of the pre-P22 flat
`save_prg_wire.rs`) and the `ripper::snapshot::pack` module's `mod.rs`
(run decomposition + search) / `codec.rs` (stream encode/decode) split are
both prerequisite 200-line-budget moves, not scope changes.

## 2026-07-11 — ADR-40: snapshot decode sweep must never write $0-$7 — live bus error, live-verified fix

**Status:** accepted
**Context:** ADR-39's first live boot of a real `SNAP.PRG` bus-errored
mid-resume: "Bus Error writing at address $0, PC=$29e" inside the hole
code's decode sweep, before it ever reached the hardware-reinstall block.
Direct probing (a hand-assembled `AUTO/PROBE0.PRG` doing nothing but
`Super(0)`, `SR=$2700`, `move.b #$42,$0.w`, spin) reproduced the identical
bus error in isolation, ruling out stub instruction ordering, supervisor
setup, and memcfg/phystop sequencing (all three were already correct).
Reading Hatari's own `src/cpu/memory.c` (`SysMem_bput`/`wput`/`lput`)
confirms the true cause: `addr < 0x8` bus-errors *unconditionally*, checked
before and independent of the supervisor-gated `addr < 0x800` branch — real
ST(E) hardware (and this emulation of it) refuses any write to the 68000
reset SSP/PC vectors regardless of CPU mode. The decode sweep's `[0,R)`
walk had no reason to special-case `[0,8)` and so hit this on every
snapshot whose frozen image held anything in the first two longwords
(effectively always, since TOS's own reset vectors live there).
**Decision:** `[0, LOW_MEM_RESERVED)` (`LOW_MEM_RESERVED = 8`) is now a
fixed, unconditional `Run::Skip` prepended by `decompose_all`
(`ripper/snapshot/pack/mod.rs`) ahead of the greedy decomposition of
`[8, hole.start)` — reusing the existing `Skip`/`OP_SKIP` mechanism (a
pure output-cursor advance, no write) already required for the $1C0 hole,
so `codec.rs` and `stub.s` needed no changes. `snapshot_wire.rs`'s fixed
`not_restored` list (§4.2) grows from ten items to eleven, disclosing this
unconditionally rather than only when the frozen image's `[0,8)` was
non-zero. Live-verified after the fix via a race-free technique (arm a
one-shot `pc = <frozen_pc>` breakpoint on the resumed session — valid only
when `frozen_pc` sits in the frozen program's own RAM, not shared TOS ROM
that legitimately re-enters the same addresses during a fresh boot; the
breakpoint tool pauses the CPU exactly there, before Hatari's VBL-driven
redraw or any timer/palette-cycle effect can run even once): CPU registers
and the full 32-byte `$FF8240` palette block matched the frozen capture
byte-for-byte, and a screenshot taken a further 200ms into real resumed
execution showed the same scene (starfield, logo, gradient) with only the
demo's own scroller having advanced — the expected effect of continued
execution, not corruption. `tests/snapshot_prg_live.rs`'s
`snapshot_resumes_frozen_scene` is rewritten to use this same technique
(`wait_for_resume_at` + a ROM-retry guard in `freeze_and_capture`); the
prior version polled `registers()` at 100ms granularity and then raced a
separate `pause()` call, which left enough of a window for the demo's own
VBL/timer-driven palette cycling to legitimately move the palette before
the read — an intermittent false-failure on a *correct* resume, not a
resume defect, caught while investigating this ADR.
**Alternatives considered:** restoring `[0,8)` via a different privileged
path (there isn't one — the check in `SysMem_bput` et al. is unconditional
at the hardware level, not a mode gate to route around); skipping the
low-mem write only in the assembled stub instead of the Rust packer (would
still spend stream bytes encoding those 8 bytes for nothing; the packer-side
`Skip` costs zero payload bytes, matching how the $1C0 hole is already
handled); relaxing the live test's palette assertion instead of making the
resume-detection race-free (would hide a real class of flake rather than
fix the measurement).
**Consequences:** Every snapshot now permanently leaves `[0,8)` as
whatever TOS booted with, regardless of frozen content — always disclosed,
never a silent gap. The fix is a pure `pack/mod.rs` + `snapshot_wire.rs`
change (16/16 `ripper::snapshot` tests, 602/602 full lib suite, both live
`snapshot_prg_live` tests green). The live-test hardening
(`wait_for_resume_at`, ROM-retry in `freeze_and_capture`) is now the
project's reusable pattern for any future "prove exact hardware-state
resume" live test — poll-then-pause is not race-free against
interrupt-driven state.

## 2026-07-11 — ADR-41: SNDH-archive-derived YM driver fingerprints; 24-bit PSG anchor fix

**Status:** accepted (extends ADR-16/ADR-35)
**Context:** ADR-35 derived 9 signatures from local sources and named the
SNDH archive (sndh.atari.org) as out of reach; it also flagged that
`ym_player::is_psg` misses 24-bit `lea/move $FF8800.l` operands. The
archive is now fetched locally (refs/sndh_lf, gitignored: 5,897 .sndh /
678 composers / 269 MB, yearly zip, all files plain — no ICE depack
needed). A whole-corpus scan grounded three facts: (1) the archive's most
frequent PSG gadgets are shared rip-wrapper shims (placeholder operands
DEAD/BA55CAFE), so frequency alone would fingerprint the wrapper, not the
driver; (2) 274 files reach the PSG only via 24-bit operands and today
produce no anchor at all; (3) the P19 module-magic TCB entries never occur
in SNDH rips (0/5,897) — driver code and module files need separate
signatures.
**Decision:** (1) Methodology extended (P24.md §4) with a
group-discrimination step: a gadget qualifies only with >=50% in-group file
coverage and <=25 out-group files (deep mode 20%/60 for multi-version
drivers), which mechanically rejects wrapper shims. (2) Eleven new
corpus-cited entries (Quartet, David Whittaker x2, TCB replay code,
Hippel x2, Tim Follin, Jess, Tao, Big Alec, Music Mon), each with
file@offset provenance and a byte-pinned excerpt test; rg-triplex's
hand-assembled encoding is now corpus-verified (Damo/YM_Rucka.sndh) but
stays 0.70 (single binary). Best-match coverage on sndh2026: 1,796/5,897
files (30.5%), per-player counts pinned. (3) `is_psg` gains the 24-bit
spellings ($ff8800/$ff8802 tokens); BACKS unchanged (all attested 24-bit
encodings sit within back-6). (4) The discovery pipeline is NOT shipped
(re-affirming ADR-35: offline, few-times-a-year, ends in human
decode/labeling); the validation half ships as the #[ignore]d
tests/ym_sig_corpus.rs (env SHIRAZMCP_SNDH_CORPUS): provenance pins,
exact best-match histogram, per-driver-dir exclusivity matrix — the
false-positive grep, mechanized and re-runnable.
**Alternatives considered:** committing a miner binary/xtask (rejected:
dead weight between yearly archive drops); frequency-ranked signatures
(rejected: names the Grazey/PHF wrapper across every driver); masking the
hippel-runl $1C displacement and bigalec's 4CD0 register field (rejected:
never observed varying — over-masking buys false positives); raising
rg-triplex to 0.82 (rejected: one binary is not two).
**Consequences:** identify_music names 11 engines instead of 5; Whittaker/
Follin/late-Hippel rips in live RAM become findable at all (the 24-bit
fix). Honest gaps remain and are documented in P24.md §10: ~69% of the
archive (per-composer long tail), Rob Hubbard ST (6 heterogeneous files),
Scavenger (gadgets too generic), Big Alec's other driver builds, Lucas/
ProTracker-STk still absent. Corpus counts are pinned to sndh2026_lf; the
yearly refresh re-pins the histogram. A wrong or dead entry still can only
produce "unknown", never a wrong name — the ("unknown", 0.25) contract
is unchanged and its tests pass unmodified.

## 2026-07-11 — ADR-42: profile_frame — the measurement window is TIME (VBL frames); the PC range is demoted to an address filter

**Status:** accepted
**Context:** The `profile` tool bounds its measurement window with a PC
pair (`run to from` → `run to to`), which cannot express "one whole frame"
(natural start/end is the same loop PC and `to > from` is required) or
"routine including bsr'd callees" (the natural exit PC sits below the
entry). The effect-dev request (STEIsNotDead MCP_TODOS §1) asked for
`profile_frame`, `profile_routine`, and/or `to < from` support. Reading the
Hatari source settled the design — **against the binary ShirazMCP actually
spawns**, `/usr/bin/hatari` (the distro package, stock v2.4.1;
`src/session/spawn/mod.rs`, env `SHIRAZMCP_HATARI`), not the
`tattlemuss/hrdb` fork checked out at `/home/matt/projects/hatari`'s HEAD.
The spec was drafted against the fork's line numbers and re-verified
against the `v2.4.1` tag in that same repo (`git show v2.4.1:src/debug/*.c`
— the tag is present even though HEAD is forked) before this ADR was
written: `profile on/off` only toggle a flag (`debug/profile.c`,
`Profile_Command`); the per-address counters are freed and freshly zeroed
by `Profile_CpuStart` (`debug/profilecpu.c:852`) on every debugger exit via
`DebugCpu_SetDebugging` (`debug/debugcpu.c:945`, called from
`debug/debugui.c:1217` on `c`/continue exit and from `DebugUI_ParseLine`
at `debugui.c:1347` per remote-fifo line), and finalized by
`Profile_CpuStop` (`debugcpu.c:1079`) on every entry — i.e. each
continue→break window yields exactly that window's data, and the buffer
spans all of ST RAM + cartridge + TOS (every executed address, interrupts
included). The fork's `hrdb`-only prev-cycle carry-over (which preserves
the first post-resume instruction's attribution across the reset) is
**absent from stock v2.4.1** (`grep savePrevCycles` = 0 hits there); stock
simply frees and zeroes, so the shipped binary loses that one
instruction's cycle count per window — a handful of cycles against a
160256-cycle frame, immaterial, and not depended on by this design.
**Decision:** One new tool, `profile_frame {frames, from?, to?}`: N managed
single-VBL legs (reusing frames.rs's vbl_condition/current_vbl primitives
and the ADR-15 lifecycle) with one `profile save` per paused frame
boundary — the reset-on-continue semantics make each dump a clean
per-frame sample with no reset command needed. Every measurement command
stays on the frozen ADR-11/15 stdin path inside the `DebugUI()` loop
(never the remote fifo `DebugUI_ParseLine` path), because that path resets
the buffer on every parsed line too — the single-session server never
sends fifo traffic mid-window, so this invariant holds by construction.
`from`/`to` become a pure address FILTER (an extent, naturally to > from):
range_* = "your code", total_* = the whole frame, both always reported
because a vsync-locked program burns ~160256 total cycles/frame by
construction (total validates the measurement; range is the cost).
fits_50hz/headroom_pct are defined on the WORST frame (a dropped frame is
a visible glitch). No static estimate (single-pass straight-line is
meaningless across a frame), hence no `PROFILE_MAX_RANGE` cap. Per-frame
dumps go to `work_dir/profile-frame-{seq:04}/frame-{i:04}.txt`; the output
returns the directory plus the worst frame's path (ADR-5).
**Alternatives considered:** (a) allow `to < from` in `profile` — patches
the symptom, still can't express whole-frame or same-PC windows;
(b) `profile_routine` with rts-depth tracking — a different mechanism
(stack tracking) whose main use ("routine incl. callees") the extent
filter already covers; deferred; (c) cumulative deltas between successive
saves (frame_i = total_i − total_{i−1}) — WRONG against this source: data
does not survive a continue (`Profile_CpuStart` free+zero), which the live
test pins by asserting per-frame totals stay ≈160256 rather than climbing
as i×160256.
**Consequences:** Frame-cost tuning becomes one call ("0.4 VBL, fits with
58% headroom"). The tool inherits the managed-lifecycle guarantees (ends
Paused, or Running on timeout with the breakpoint disarmed). Headroom is
only meaningful with an extent that excludes the vsync wait-spin — stated
in the tool description rather than hidden. The reset semantics were
verified against stock v2.4.1 (the binary actually spawned), not just the
hrdb-modified checkout the spec first drafted against; the live test's
±frame-total assertion detects any divergent binary immediately, whichever
build a future `SHIRAZMCP_HATARI` points at. 50 Hz is assumed
(`VBL_CYCLES = 160256`); 60 Hz/mono machines measure correctly but
misstate the budget — future work if ever needed.

## 2026-07-11 — ADR-43: Control-plane robustness — Hatari noise filtered centrally; EPIPE classified as process death

**Status:** accepted
**Context:** (1) Hatari's main loop `perror()`s "command FIFO read error:
<errno>" on every idle poll of its non-blocking control fifo
(`Control_CheckUpdates`, v2.4.1 `control.c:398-411`, polled from
`Main_EventHandler` at `main.c:521`); with stderr redirected to our log,
every long-running tool call returned dozens of noise lines. This turned out
to be more than cosmetic: the noise line's own text contains "error"
case-insensitively, so it could reach `debug::write_memwrite_line`'s
case-insensitive `"ERROR"` scan and be mistaken for a real Hatari write
rejection — the field-reported `write_memory` failure on a Running session
(STEIsNotDead `MCP_TODOS.md` §6: `parse error (memwrite): unexpected input
"command FIFO read error…"`), confirmed by tracing both paths through the
same `debug()` → `strip_protocol_lines` chokepoint and pinned with a
dedicated regression test. (2) A field report showed `fifo I/O … Broken
pipe` while `status` claimed `paused`, and recurred "several times in one
session" (`MCP_TODOS.md` §3/§7). Source analysis pins EPIPE on either
control plane to an exited/exiting Hatari: `Control_RemoveFifo`'s only
runtime callers are `Control_SetFifo`'s own re-open (never triggered at
runtime by this server), a startup option-parse failure (`main.c:929-932`),
and `main.c:971` — the line immediately after `M68000_Start()` returns, the
normal emulator exit path. Live-verified: an unreaped `<defunct>` Hatari
zombie sat behind a session the FSM still reported `paused`.
**Decision:** (1) A single documented noise predicate
(`fifo/noise.rs::is_hatari_noise`, prefix-match only — `perror` suffixes are
locale text) applied in `strip_protocol_lines`, the existing central
output-hygiene point, so every tool inherits the fix and noise is stripped
before ADR-28's tail cap AND before any caller's own error-line scan
(closing `MCP_TODOS.md` §6 as a side effect). (2) A `ProcessExited` error
variant; on control-plane EPIPE, `HatariProcess` probes `try_wait`: dead ⇒
`ProcessExited` (never retried), alive+fifo-plane ⇒ reopen the write end
once (1 s bound) and retry the whole operation exactly once (safe: sentinels
are per-call unique, so a half-finished attempt's stale echo can never
satisfy the retry's fresh value), alive+stdin-plane ⇒ classify only after a
brief grace + re-probe (no reopen is possible on stdin — the kernel only
closes a child's stdin read end at process exit). The error message
explicitly names `start` as the required recovery, not a retry: repeated
field reports show Hatari is genuinely dying under real workloads, not
merely dropping a pipe, so the message must not suggest anything less than a
fresh boot. (3) `status` probes liveness and reaps: a dead child under a
live-state FSM is reset to `Stopped` with an explanatory `note` carrying the
exit status (`ExitStatus`'s `Display`, e.g. `signal: 11 (SIGSEGV)`) — the
one cheap "why" datum available without redesigning the error shape to also
carry a captured log tail.
**Alternatives considered:** Filtering in `LogTail` ingestion (rejected:
couples semantics-free tailing to Hatari line meanings and silently affects
needle-matching callers — today no needle collides with a noise line, but
the coupling would have to be re-proven for every future needle). Patching
Hatari (rejected: we drive the user's binary). Retrying EPIPE blindly at the
write site (rejected: `write_line` has no `Child` access, mid-protocol
retries corrupt sentinel round-trips, and EPIPE is almost never transient).
Auto-transitioning the FSM to `Stopped` from every tool on `ProcessExited`
(rejected: smears guard-transition logic across the tool layer; the error
message directs to `status`/`stop`, both of which recover). Attaching a
captured log tail to `ProcessExited` for a richer "why" (considered per a
field request — deferred: the pinned error shape carries `status` only; a
log tail needs its own sizing/formatting/API-surface design this phase did
not scope).
**Consequences:** `run_frames`/`watch` output is clean; `write_memory` on a
Running session no longer false-fails on Hatari's own noise; a crashed
Hatari is detected at the next control-plane write and `status` self-heals
to `stopped` with the exit status preserved for diagnosis; the noise list is
one grep away from extension; the paused-plane EPIPE remains
non-recoverable by design (stdin cannot be reopened) — only honest. Why
Hatari keeps dying under the reporting workload is still unknown; this fix
makes the death visible and diagnosable (exit status in `note`) rather than
closing the root cause — a follow-up investigation, not a P27 deliverable.
**Implementation note:** every Hatari file:line citation in this ADR and its
code comments was verified against the real, stock `v2.4.1` tag (`cd
/home/matt/projects/hatari && git show v2.4.1:src/<file>.c` — that checkout
is the tattlemuss/hrdb fork, but its `v2.4.1` tag is the genuine upstream
release commit, confirmed via `git log -1 v2.4.1`). `docs/plan/P27.md`'s own
citations were drawn from a different working tree and were consistently
off by tens to hundreds of lines (the semantics they described were
correct; the exact line numbers were not) — corrected throughout rather than
left as a trap for the next reader.

## 2026-07-11 — ADR-44: send_key — IKBD injection over the existing control fifo, composed with the managed frame-runner; KEYS_MAX derived from Hatari's real fifo read window

**Status:** accepted
**Context:** Headless capture of interactive modes required rebuilding
programs with a `-D` default flag because nothing could press a key
(STEIsNotDead `MCP_TODOS.md` §4). Hatari's control fifo already accepts
`hatari-event keypress/keydown/keyup <char|scancode>` plus
`doubleclick`/`rightdown`/`rightup` (v2.4.1 `control.c:66-161`); but
`hatari-event` is serviced by `Control_CheckUpdates` in the MAIN loop
(`main.c:521`) — a Hatari frozen in DebugUI (ADR-11) queues fifo writes
until resume, and `Control_ProcessBuffer` aborts a whole read batch at the
first invalid command (`control.c:371`).
**Decision:** One `send_key` tool (`keys`: 1..=14 entries — chars, scancodes
0..=255 emitted canonically as decimal, named aliases pinned from
`keymap.c`'s `Keymap_PcToStScanCode`, plus the three mouse events; `action`
press/down/up). Validation is 100% client-side (batch-abort fact). Events
always go to the fifo: Running ⇒ serviced within one main-loop iteration,
confirmed by one sentinel round-trip whose output is scanned for Hatari
`ERROR:` lines; Paused ⇒ queued (reported `queued: true`). An optional
`frames: N` arg composes with the ADR-15/27 machinery (`ensure_paused` →
queue events → `run_to_breakpoint` on VBL+N) into the atomic headless-
capture primitive: freeze → inject → run exactly N frames → freeze →
screenshot.

**Deviation from the phase spec:** `docs/plan/P27.md` §3.2 pinned
`KEYS_MAX = 32`, justified by "Hatari's 4096-byte read buffer
(`control.c:428`)". Verified against the real v2.4.1 tag (the binary this
server actually spawns — see ADR-43's `Implementation note`), that citation
is wrong: `Control_CheckUpdates()` reads at most **399 bytes** per poll
(`char buffer[400]`, `read(ControlFifo, buffer, sizeof(buffer)-1)`,
`control.c:401,409`) and, by its own doc comment, assumes a whole command
arrives in one read — a split command is not reassembled, and the orphaned
tail additionally aborts the *next* read's batch
(`Control_ProcessBuffer`'s `while (ok && cmdend && *cmd)`, `control.c:371`).
At 32 entries the worst-case batch (832 B) exceeds the real window on every
`send_key` path, including the `frames` path, where lines are queued
against a frozen emulator and coalesce into one contiguous unread blob read
as a single ≤399-byte chunk on resume — not a rare edge case, but the
feature's primary composed flow. `KEYS_MAX` is therefore derived from the
real window instead of the spec's pinned number:
`(399 − 35 B sentinel headroom) / 26 B worst-case line
("hatari-event keypress 255") = 14`, enforced by a compile-time `const`
assertion (`input_wire.rs`) so a future token-table change cannot silently
reopen the gap. This mirrors the codebase's existing `write_line` precedent
(`fifo/mod.rs`): pin the per-write invariant that is provably safe, do not
engineer a timing-based mitigation (ADR-7's anti-blind-sleep stance would
reject one anyway) — ruling obtained from rust-architect before
implementation rather than silently guessed.
**Alternatives considered:** Shipping `KEYS_MAX = 32` as spec'd and
documenting the corruption risk as accepted (rejected by rust-architect
ruling: not rare, not confined to the no-confirm Paused path, and directly
contradicts the spec's own stated safety rationale for the number).
Byte-precise runtime validation (keys ≤ 32 *and* total bytes ≤ budget)
instead of a single lower entry cap (rejected: buys at most two extra
entries in the char-only case for a second validation surface and a
harder-to-explain rejection). Erroring when paused (rejected: discards the
free queue-on-freeze behavior and worsens the agent flow). Auto
thaw-send-refreeze (rejected: uncontrolled frame count — the `frames` arg
does it deterministically with existing machinery). A separate `send_text`
tool (rejected: alnum-only subset of the sequence path, second validation
surface, no new capability). Exposing mouse events as a separate tool
(rejected: same wire path, three parse-table lines). Cross-call queued-byte
tracking to close the residual multi-call-while-paused risk (rejected:
couples session state to every resume path for a corner only the caller
controls — documented instead, see Consequences).
**Consequences:** Interactive prototypes are drivable headless with
deterministic capture; press+release lands in one main-loop slice, so
state-polling programs need down/frames/up (documented on the tool); the
single-char path rides the host SDL keymap (uppercase auto-shifts,
layout-odd chars should use scancodes). Sequences longer than 14 need
multiple calls — acceptable, since the tool is cheap and stateless, and 14
still covers an 8.3 GEMDOS filename plus `.` and enter. Accepted residual
risk: stacking several *queued* `send_key` calls while Paused (no `frames`)
can still accumulate more than 399 unread bytes in the pipe — per-call
bounding cannot close this without cross-call queue accounting; the tool's
`queued: true` semantics document that a caller should resume/run_frames
between batches rather than stacking many queued calls.

## 2026-07-11 — ADR-45: Symbol-aware addressing — Hatari resolves, the server lists

**Status:** accepted
**Context:** TOS relocates GEMDOS-launched programs, so demo developers
hand-computed label addresses (MCP_TODOS §2). Hatari v2.4.1 already loads
DRI/GST symbol tables (autoload on debugger entry, `symbols prg`/`symbols
<file> [T [D B]]`) and resolves bare names in `evaluate` expressions and
`b` conditions (evaluate.c getValue; breakcond.c ParseValue — both verified
at the v2.4.1 tag). What was missing: an MCP surface, auto-detected section
bases, and label names in tools.
**Decision:** (1) One grouped `symbols` tool (load/list/free). (2) Hatari
is the ONLY name→address authority: `resolve_addr` = `Addr::parse` first
(numbers are never reinterpreted — strict superset of the old behavior),
then `evaluate <name>` on parse failure. (3) Listing is served from a
Rust-side mirror parsed from the same file with the same offset rules —
Hatari's own `symbols name|code|data` is FORBIDDEN on the wire because
v2.4.1's Symbols_Show pages via fgets(stdin) every ~20 rows, which
deadlocks the main loop (Running) or corrupts the stdin control plane
(Paused); new grep-gate (`tests/symbols_live.rs`'s
`symbols_listing_pager_grep_gate`). (4) Auto base = existing
`debug::basepage` (ADR-31) feeding explicit `$hex` T/D/B offsets; PRG
sources are relocated by Hatari itself against the same basepage. (5)
Blast radius: exactly profile/disasm/breakpoint take names (breakpoint
needed zero plumbing — Hatari's breakcond resolves at arm time, only an
arm-failure hint was added); widening the other ~12 address-taking tools
is a mechanical `parse_addr`→`resolve_addr` swap left for demand. (6) vasm
`-L` listings are NOT ingestible (Hatari accepts nm-ASCII or in-PRG tables
only); vasm `-Ftos` embeds a DRI table by default, so the primary workflow
needs no extra files.
**Alternatives considered:** resolving from the server-side store (rejected:
dual authority drifts from autoload/free/program-exit); generating a Hatari
config file to raise nSymbolLines and use native listing (rejected: touches
global spawn behavior to work around a listing we can serve better
ourselves); symbol support in all ~15 address-taking tools (rejected:
blast radius, and the seam makes later widening one line per tool).
**Consequences:** label workflows cost one extra `evaluate` round-trip per
name; a name that is also a register/variable (a0, VBL, TEXT) resolves to
the register — documented, matches native debugger behavior; names outside
[A-Za-z0-9_] load but cannot be referenced (Hatari's tokenizer, not ours);
the store must be cleared on stop/free or `list` lies. One implementation
note versus the phase spec: the ripper-side DRI/GST parser
(`ripper::prg_symbols::parse_dri`) checks only the RELOCATED symbol's
upper section bound (`addr < section_end`), not a lower bound — a symbol
whose raw value happens to be small enough to land before its own
section's base (observable only when `use_bssdata_offset`'s TEXT-relative
branch fires for a DATA/BSS symbol) is still accepted rather than skipped;
the spec's §2.5 wording ("past its section end") only pins the upper
check, so this is a literal reading, not a deviation — flagged here for
visibility since it's a corner the live test (§10.2) doesn't exercise.

## 2026-11-14 — ADR-46: `start` machine options and the session-dependent address bus (CT60-like Falcon)

**Status:** accepted
**Context:** The "Rift" demo's Falcon CT60 build (68060, 32-bit addressing, TT-RAM) ran in Hatari but `start` had no CPU/TT-RAM/DSP options, and every address parser rejected anything >= `$1000000` (ADR-31's 24-bit `BUS_LEN`), while `annotate`/two operand parsers masked addresses to 24 bits.
**Decision:** (1) `start` gains optional, enum-typed options; `MachineOptions` (`session/spawn/options.rs`) validates the combination (TT-RAM needs tt/falcon + `addr24=false`, cpu_level 6 needs `addr24=false`, DSP Falcon-only, ...) on the FINAL overlaid config and is the only place that builds the Hatari flags; unset options pass no flag so existing calls are byte-identical. ST-RAM stays capped at 14 MB; TT-RAM is a separate 0-1024 (x4) option. (2) `Addr::parse` is now pure 32-bit; the "fits the bus" check moved to where the session is known: `HatariConfig::check_addr` / `DebugChannel::bus_limit` (24-bit unless `addr24=false`), so a default session still REFUSES `$1000000` rather than letting Hatari silently mask it to `$0`. (3) The 24-bit masks became `Addr::bus24`: `$FFFF8240` aliases `$FF8240`, but `$01FF8240` (TT-RAM) is no longer mis-annotated as a palette register. (4) `auto_run` is a path ON the GEMDOS drive (no `..`, no `:` etc.); `tos`/`harddrive` named in a call must exist.
**Alternatives considered:** leaving parse strict and adding `parse32` (every tool then needs the config; two parsers drift); trusting Hatari to reject bad combos (it silently ignores some).
**Consequences:** `rip`, `search_memory`, snapshots and basepage discovery still cover ST-RAM only (`ram_len`); a program loaded into TT-RAM is not reachable by `save_prg`/basepage discovery. `symbols` still mirrors Hatari's 24-bit symbol bound.

## 2026-11-16 — ADR-47: profile_raw, count_hits, set_registers — first-class tools for what agents were scraping from hatari.log

**Status:** accepted
**Context:** Long reverse-engineering sessions kept hand-scraping `hatari.log`. They ran `profile save` and summed the dump by hand, and they counted breakpoint hits with `grep -c`, which over-counts because Hatari also echoes breakpoint definitions and listings into the log. They also set registers with raw `r` commands and lost stack pointers to the USP/ISP banking.
**Decision:** (1) `profile_raw` runs one profiler window over N whole frames, aligned to a VBL boundary as in ADR-42. It reuses `profile_frame`'s leg, generalised to `frames_leg(n)`, and returns totals against `N × 160256`, a region split, named ranges with `entry_hits`, and the top N addresses. An empty dump is an error. (2) `count_hits` arms `b pc = $X :trace` per PC and runs one managed N-frame leg. The count is Hatari's cumulative per-breakpoint `matched H times` counter (`breakcond.c`), not a line count: the `b` listing carries no hit counts, and the counter is immune to echoes, listings and lost lines. The count is matched on the armed breakpoint's index as well as its expression, so an identical user breakpoint's counter is never read as ours. Its breakpoints are removed on every exit, highest index first. A pre-existing identical breakpoint keeps its lower index and stays. `managed::handle_fired` now accepts its `:once` expression anywhere among the matched blocks, because trace hits print the same header first. (3) `set_registers` requires a paused session. In Hatari's UAE core, A7 is the live SP; `regs.usp`/`regs.isp` are banked copies, and `cpureg` re-syncs the active one from A7. So USP/ISP are never written directly. The inactive pointer goes in through A7 while SR has S flipped, then the final SR is written, then the active pointer through A7, then D/A registers, PC last. Everything is read back, and a failed write rolls back to the original snapshot. (4) The `cpureg` parser also accepts Hatari 2.6's `IM=` flag key, alongside 2.4's `IMASK=`.
**Alternatives considered:** for `count_hits`, grepping the per-hit lines (fragile, and it was the original bug) or `:quiet` with a later readout (Hatari never reports hit counts outside a match). For `set_registers`, writing `r usp=`/`r isp=` directly (silently lost for the active mode).
**Consequences:** each trace hit is two log lines buffered in memory until the window closes, so `count_hits` caps `frames` at 500 and very hot PCs slow emulation. `profile_raw` reports TOS 1.0x (`$FC0000`) as "other". `set_registers` takes JSON numbers, not `$hex` strings.
