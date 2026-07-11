# P1 finalize — pause/freeze redesign (ADR-11) + file-size cleanup

Resolves ADR-9 addendum §3 (`hatari-stop` deadlock). Everything below was
**verified live** against Hatari 2.4.1 (`DISPLAY=:0`, MJJ demo
`MJJHACK.PRG`, `--machine ste --memsize 1 --fast-forward on`, the exact
`boot.rs` invocation) on 2026-07-08. The rust-engineer appends §1 to
`decisions.md` verbatim, then implements §2–§4.

---

## 0. Live evidence (spike log: session transcript, 2026-07-08)

| # | Probe (verbatim wire string → channel) | Observed |
|---|---|---|
| 1 | `hatari-stop` → fifo | **Permanent deadlock.** 5× `hatari-cont`, a fifo `hatari-debug evaluate pc`, AND a stdin probe all produced zero log output over 5+ s; process alive, both control planes dead. Matches ADR-9 addendum §3. |
| 2 | `hatari-debug b pc > 0 :once` → fifo, stdin = `/dev/null` (current `boot.rs`) | Breakpoint fires instantly, "You have entered debug mode" logged, but DebugUI reads EOF on stdin and **immediately continues** — PC advanced across 3 reads ($b998 → $aaf0 → $d37a). Freeze does not hold. |
| 3 | Same breakpoint, stdin = **held-open pipe** | **Freeze holds.** CPU parked (`CPU=$e007ec` banner); cmd-fifo NOT serviced while frozen (writes queue in the pipe). |
| 4 | Raw debugger lines → **stdin** while frozen: `evaluate pc` ×2, `r`, `savebin <f> 0 $100000` ×2, `screenshot <f>`, `evaluate $DEADBEEF` | All serviced, output in the same log. PC identical across reads ($e007ec, $e007ec). Two 1 MiB `savebin` dumps **byte-identical** (memory frozen). Screenshot PNG written. Sentinel echo `…$deadbeef (hex)` appears in the standard dual signed/unsigned form — the existing `SENTINEL_ECHO` matcher works verbatim on this channel. |
| 5 | `c` → stdin | "Returning to emulation…" logged; PC advances again; **fifo serviced again**; fifo lines queued during the freeze drain and execute at this point. |
| 6 | Repeat freeze→inspect→resume a second time | Works identically (repeatability confirmed). |

Conclusion: Hatari services the cmd-fifo only from the running main loop.
`hatari-stop` halts that loop without entering the debugger → nothing can
ever read the fifo again. A **breakpoint** halts the CPU *inside DebugUI*,
which reads **stdin** — so stdin is the frozen-state control plane, and the
existing sentinel/log-tail machinery works unchanged over it.

---

## 1. ADR-11 — append to `decisions.md` verbatim

```markdown
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
```

---

## 2. Minimal patch spec (rust-engineer)

No MCP tool names/schemas change — no router work; `rmcp` usage stays as
per `docs/plan/rmcp-2.2-notes.md` (only two `description` strings change,
§2.6).

### 2.1 Verbatim wire strings

| Purpose | Channel | Line written |
|---|---|---|
| Arm freeze | fifo | `hatari-debug b pc > 0 :once` (raw write, **no sentinel** — a fifo sentinel would queue behind the freeze and replay on resume) |
| Confirm freeze / any Paused command | stdin | `<cmd>` then `evaluate $DEADBEEF` (raw CLI lines, no `hatari-debug` prefix; same `SENTINEL_ECHO` match) |
| Thaw | stdin | `c` (raw write, no sentinel — **nothing may be written to stdin after `c`**) |
| Confirm resume | fifo | existing `debug("evaluate pc")` sentinel round-trip |
| Banned forever | fifo | `hatari-stop` (grep-gate: no occurrence outside tests/docs) |

### 2.2 `src/session/spawn/boot.rs` — `build_command`
`.stdin(Stdio::null())` → `.stdin(Stdio::piped())`.

### 2.3 `src/session/spawn/mod.rs` + new `src/session/spawn/control.rs`
- `HatariProcess` gains two private fields: `stdin: tokio::process::ChildStdin`
  (taken via `child.stdin.take()` in `spawn()`, held for the session's life)
  and `control: ControlChannel` (starts `Fifo`).
- New file `control.rs` (precedent: `boot.rs` split) owns:
  ```rust
  /// Which control plane services debugger commands. Runtime mirror of the
  /// type-state (Running=Fifo, Paused=DebugStdin); private, mutated ONLY by
  /// freeze()/thaw() so it cannot drift from the Session<S> transitions.
  pub(super) enum ControlChannel { Fifo, DebugStdin }

  impl HatariProcess {
      /// ADR-11 freeze: arm `b pc > 0 :once` on the fifo, then sentinel-
      /// confirm over stdin (echo ⇒ DebugUI entered ⇒ CPU parked).
      pub(crate) async fn freeze(&mut self) -> Result<()>;
      /// ADR-11 thaw: `c` on stdin (last stdin write), flip to Fifo, then
      /// fifo sentinel round-trip confirms the main loop is servicing again.
      pub(crate) async fn thaw(&mut self) -> Result<()>;
  }
  ```
- `HatariProcess::debug_with_timeout` routes on `self.control`:
  `Fifo` → write `hatari-debug {cmd}` to the fifo; `DebugStdin` → write
  `{cmd}` raw to stdin. Same `LogTail`, same sentinel wait either way (make
  `fifo::debug`/`write_line` generic over `W: AsyncWrite + Unpin` — both
  `tokio::fs::File` and `ChildStdin` qualify — and pass the prefix).
  `debug/mod.rs`'s `DebugChannel` seam and every caller above it are
  **unchanged**.
- `shutdown()`: if `control == DebugStdin`, best-effort write `c` to stdin
  before the existing `hatari-shortcut quit` → 2 s → SIGKILL ladder (a
  frozen Hatari never reads the fifo quit).

### 2.4 `src/session/mod.rs` — transitions
- `Session<Running>::pause()`: delete the `write_wrapper("hatari-stop")` +
  fifo sentinel body; call `self.data.freeze().await` (same
  `Result<Session<Paused>, (Session<Running>, Error)>` shape).
- `Session<Paused>::resume()`: delete `hatari-cont` body; call
  `self.data.thaw().await`.
- Timeout `what` strings: `"freeze (debugger entry after b pc > 0 :once)"`
  / `"resume (fifo liveness after c)"` — errors must explain themselves.

### 2.5 `src/session/fifo/` — sentinel hardening (one behavior change)
In `debug()`'s output filter, additionally drop any line matching
`is_sentinel_line` (not just `"> "` echoes/blanks). `$DEADBEEF` is reserved
as the marker, so stray echoes from a retried/queued sentinel can never
pollute a later command's parsed output (`parse_evaluate` takes the first
`= %` line — without this filter a stale `$deadbeef` echo could be
returned as a real value). Unit test: collected output containing a second
sentinel line yields output without it.

### 2.6 `src/tools/lifecycle.rs` + `src/tools/debugger.rs` — text only
- `pause` description (now true): "Freeze emulation via a debugger break
  (freezer-cartridge 'freeze then rip'). Registers, memory, screenshot and
  rip all work while frozen; the machine state does not advance."
- `debug` tool description gains: "Do not arm breakpoints (`b …`) via this
  tool — a firing breakpoint freezes the machine outside session control;
  use pause (P2 adds a managed breakpoint tool)."
- No handler/logic changes (per `docs/plan/rmcp-2.2-notes.md`, description
  is a `#[tool]` attribute string — no schema impact).

### 2.7 `tests/integration.rs` — `pause_freezes_pc` rewrite (P1.md §10.4)
1. `start` → `pause()` → Ok (internally sentinel-gated; no sleeps for sync).
2. `debug::registers` twice, 50 ms apart → identical `pc` (the 50 ms sleep
   asserts frozenness, not synchronization — allowed).
3. `session::memory::acquire` twice → **byte-identical** `MemImage`s
   (freeze-then-rip, proven at the memory level).
4. `resume()` → Ok; poll `registers` up to 20× → pc differs at least once.
5. **Second cycle:** `pause()` → frozen pc stable again → `resume()` → Ok
   (repeatability is where queued-line bugs surface).
6. `stop()`; pgrep orphan check as before.

### 2.8 `ARCHITECTURE.md`
Update the session-layer section (two control planes table from §2.1), the
state-machine notes (pause = debugger freeze; `hatari-stop` banned), and
the project-structure tree for §3's new files.

Acceptance additions: `grep -rn "hatari-stop" src/` → no hits;
`cargo test --test integration -- --ignored --test-threads=1` green.

---

## 3. File-size cleanup (hard rule: no file > 200 lines)

Follow the P0/P1 precedents (`spawn/{mod,boot}.rs`, `parse/{mod,cpureg}.rs`):
mechanical moves, tests travel with their unit, `mod.rs` re-exports keep
every external path compiling unchanged.

| Current (lines) | Split into | Contents |
|---|---|---|
| `src/session/fifo.rs` (295, grows in §2) | `src/session/fifo/mod.rs` | sentinel consts + `is_sentinel_line`, generic `debug<W>`, `read_sentinel_with_retry`, `write_wrapper`/`write_line`, sentinel/protocol unit tests |
| | `src/session/fifo/log_tail.rs` | `LogTail` (struct + `read_until` + `read_new_bytes`) + its unit tests |
| `src/types.rs` (206) | `src/types/mod.rs` | `Offset`, `PluginId`, `MemImage` + their tests; `mod addr; pub use addr::Addr;` |
| | `src/types/addr.rs` | `Addr` (new/raw/parse/Display) + its tests |
| `src/debug/parse/mod.rs` (207) | `src/debug/parse/mod.rs` | `parse_err` (pub(super)), `fixture_block`, `mod` decls + re-exports (`pub use`) only |
| | `src/debug/parse/values.rs` | `parse_evaluate`, `parse_memdump` + their tests (numeric extraction family) |
| | `src/debug/parse/status.rs` | `parse_symbols`, `parse_screenshot` + their tests (outcome-line family) |
| (`src/debug/parse/cpureg.rs`) | unchanged | — |

`src/session/spawn/mod.rs` (141) stays under the ceiling because §2.3's
freeze/thaw/channel logic lands in the new `spawn/control.rs`, not there.
