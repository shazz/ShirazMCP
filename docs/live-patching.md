# Live-patching a running Atari ST demo through ShirazMCP

A worked tutorial: how an AI agent used the MCP tools to **hot-patch a running
demo** — no source, no rebuild — changing its scroller text, gradient colours, and
logo palette *while it ran*. This is the freezer-cartridge workflow (freeze →
inspect → poke → observe), done through MCP tool calls.

The case study is the "Blade Runners / Kickstart" demo (`MJJHACK.PRG`). Every
address below is real, from the actual run.

---

## The mental model

You are never editing a file. You are reaching into the **live RAM of a running
machine**, finding where a piece of data lives, and overwriting it. Three questions
drive everything:

1. **Where is the data?** (`search_memory`)
2. **Is the copy I found the one actually being used?** (`disasm` the code that reads it)
3. **Will my write stick, or does the demo recompute it every frame?** (patch, then observe)

Freeze first (`pause`) so nothing moves while you look.

---

## Step 1 — Boot and confirm

```
start(harddrive="/…/MJJ/build", machine="ste", memsize_mb=1)   # auto-runs AUTO/MJJHACK.PRG
screenshot()                                                    # confirm the scene you expect
```
`start` returns only once the machine is booted (sentinel-synced). The `screenshot`
is your ground truth — confirm you're looking at the right program before you touch
anything. (A wrong hard-drive silently booting the wrong demo is a classic trap.)

## Step 2 — Find the scroller text (data read live → easy)

```
search_memory(pattern_ascii="PLEASE, READ ALL THIS SCROLL !!!")   → $02D645
```
Text scrollers read their message straight from a buffer every frame, so overwriting
that buffer changes what scrolls. **Respect the original length** (write over it in
place; don't run past the buffer into the next field):

```
write_memory($02D645, "MATT + ANIMA HOT-PATCHED THIS LIVE THROUGH SHIRAZMCP !!!")   # padded to original length
```
Tip used here: the agent cross-referenced the demo's own `scrolltext.s` to learn the
exact span — but with no source you'd bound it by the surrounding non-ASCII bytes.

## Step 3 — Find the gradient colour table (confirm it's live with `disasm`)

The blue→red gradient is a **raster effect**: a table of ST palette words written to
`$FFFF824x` on every horizontal-blank. Find the table by searching for a slice of the
known ramp:

```
search_memory(pattern_hex="<16 bytes of the blue→red ramp>")   → $03201A and $047250   (two copies!)
```
Two copies — which is live? **Disassemble the handler that consumes it:**
```
disasm(ArmTimerBRaster / TimerBHandler)   → reads the table directly every HBL, no shadow copy
```
That confirms the table is read live (so a patch sticks) and either copy works. Build
a replacement ramp with the **assembler** instead of hand-encoding words:
```
assemble("dc.w $0e0,$0c0,… (green→purple ramp)")   → bytes
write_memory($031EF8, <those bytes>)
```

## Step 4 — Find the logo palette (the hard case: an *indirect pointer*)

```
search_memory(<32-byte top_logo_palette>)   → $00171C, $00E06A, $0F7F22   (THREE copies)
```
Three copies. The agent's first patch hit `$00171C` — it verified in RAM but **never
rendered**. Why? `disasm` the VBL handler that reinstalls the palette:
```
disasm($00DB2A):
    movea.l $473B8.l,a0        ; <-- the live table address is stored INDIRECTLY
    movem.l (a0),d0-d7
    movem.l d0-d7,$FF8240.l
```
The code doesn't reference a table directly — it follows a **pointer at `$473B8`**.
So resolve the pointer to find the copy that's actually used:
```
read_memory($473B8, 4)   → $00E06A     (the live copy)
patch($00E06A, "<recoloured palette>")
```
**Lesson:** when a patch verifies in memory but doesn't show on screen, you patched a
dead copy — disassemble the *consumer* and follow its pointers to the live one.

## Step 5 — Catch a moving change (timing)

The scroller only shows your text when it scrolls into view. The agent found the
live cursor variable by disassembling the scroll routine (`move.l a0,$470E8`), then
**polled it** and `pause()`d exactly when the message re-entered the screen after a
full lap. `screenshot()` there caught "…ED THIS" mid-scroll.

## Step 6 — `patch` vs `write_memory`

- **`write_memory(addr, bytes)`** — raw poke. Fast, for data you've already encoded.
- **`patch(addr, source)`** — assembles 68k source → writes → **reads back and
  disassembles to confirm** (`verified: true`). Use it for code, and whenever you
  want the write self-checked.
- **`assemble(source)`** alone — just get the bytes (e.g. to build a `dc.w` table).

---

## Does a patch "stick"? The one rule

A write holds **iff the demo doesn't overwrite that memory itself**. In this demo all
three targets were static `incbin` data read live every frame — so all three stuck
simultaneously and permanently. If a value is *recomputed* each frame (e.g. a
plasma's palette generated in code), your poke lasts one frame and is redrawn — patch
the *code that computes it* instead, or the source constant it reads.

## Checklist

1. `start` → `screenshot` (confirm).
2. `pause` (freeze for stable inspection).
3. `search_memory` (ascii for text, hex for known byte patterns) → candidate address(es).
4. If multiple copies or "doesn't render": `disasm` the consumer → follow pointers → find the *live* copy.
5. `assemble`/`write_memory`/`patch` the live copy.
6. `resume`; `screenshot` (poll/pause for moving elements).

---

## What was missing — operations the exercise wished for

The agent got there, but several steps were harder than they should be. These are
real gaps this demo surfaced (tracked in `TODO.md`):

1. **Memory watchpoints — "break when address X is read/written."** The single
   biggest friction was *"which of these 3 copies is the live one?"*. The agent
   solved it by hand-disassembling handlers and chasing an indirect pointer. A
   watchpoint (Hatari's debugger supports access breakpoints) would answer it
   instantly: set a read-watch on `$FF8240`/the palette, resume one frame, and the
   PC that hits *is* the consumer — no disassembly archaeology. **Highest-value
   missing op.**

2. **Data cross-references — "what code touches this address?"** The static cousin of
   (1): given the gradient table address, list the instructions that read it. Turns
   "disassemble likely handlers and hope" into a direct lookup.

3. **Deterministic frame stepping — `run_frames(n)` / step-VBL.** Catching the moving
   scroller meant polling a variable and racing to `pause`. A "run exactly N VBLs then
   freeze" op makes transient/animated states reproducible instead of a timing gamble.
   (Also enables clean before/after diffs.)

4. **Memory-change scan ("cheat search").** The classic Action-Replay feature: snapshot
   RAM, let it run, snapshot again, and list addresses whose value *changed* (or
   matches a filter). That's how you'd find the scroll cursor — or any game variable —
   *without* disassembly. A natural fit for this cartridge and a strong RE primitive.

5. **Robust long writes.** `write_memory`/`patch` truncated writes >~16 bytes (Hatari's
   fifo assumes one read == one command); the agent had to chunk to 16 bytes by hand.
   *(Already being fixed — safe chunking + verify.)*

Of these, **watchpoints (1)** and a **memory-change scan (4)** are the two that would
most change the workflow — they turn "read the disassembly and reason" into "ask the
machine directly," which is exactly what a freezer cartridge should let you do.

---

## Redux — the RE toolkit, all three patches

A follow-up pass on the same demo (now identified from its own symbol table as
`STRGOOSE.PRG`), fixing two patches that verified in RAM but didn't land where
intended, and proving all three survive a fresh boot from a standalone `.PRG`.

### 1. Logo row-swap — a bitmap band exchange, not a palette chase

Swapping "RUNNERS" on top of "BLADE" is a pure **bitmap-band exchange**, unlike
the indirect-pointer chase Step 4 needed for a mere recolor. `search_memory` on
one fully-opaque scanline (line 71 of the 74-line, 320×74, 4-plane logo) finds
three copies: `$00E08E` is the **contiguous source bitmap** (160 B/line × 74,
TEXT segment — the 24 bytes after each candidate's line-71 slice are still
non-zero image data), `$032200` and `$03B300` are the two double-buffered
**screen copies** (184-byte stride, BSS). Scan lines 28–46 for the
least-dense line (fewest non-zero bytes) to find the BLADE/RUNNERS gap — line
39 — then `swapped = img[39*160:] + img[:39*160]` and write it back to all
three copies (contiguous for the source, 184-stride for the buffers). No
external asset is imported; it's the demo's own bitmap, re-ordered.

### 2. The scroller — one buffer, not "the wrong one"

The big 3-layer font scroller and the small "PLEASE, READ ALL THIS SCROLL
!!!" line are **the same buffer**: a single 1632-byte ASCII blob at
`$02D640`, running right up against an embedded SNDH tune ("Thrust", Rob
Hubbard) at `$02DCA0`. The original fix (`write_memory($02D649, ...)`,
matching the 33-char span) was correct but too short — the scroll passes 33
characters in about a second, then spends minutes showing the untouched
remainder ("…BOY FROM S.T.C.S… VERY NICE… DESIGNED BY NINJA… 22-09-88…"),
which is what verification screenshots caught. Fix: overwrite almost the
entire 1632 bytes (keep the last 16 — a loop/control tail right before the
SNDH data — untouched) with a repeating message, so wherever the scroll
cursor lands, it's ours. Both patches are kept; the small one is now a
subset of the big one.

### 3. Gradient — the real raster ramp, not the logo palette

`watch($FF8242, on="write")` caught the **wrong writer**: the once-per-VBL
top-logo palette reinstall (`movem.l d0-d7,$00FF8240` at `$00DB4A`, an
8-register block that happens to touch `$FF8242` as its second word). The
real per-HBL ramp writer was found from the demo's own symbol table via
`disasm(..., source="live")` — `bsr.w #$0118 ArmTimerBRaster == $0000DC6C`
led straight to the reset stub, and a later branch's disasm named the table
directly: `lea.l $00031EFC raster_table,a0`. **`$00031EFC`**: 200 words / 400
bytes, TEXT segment, reset every VBL and swept once per frame ending at
`$0003208C`. Values `< $1000` are plain `$0RGB` colours; values `>= $1000`
are control/sentinel words — `$1000-$7FFF` holds the current colour (skip
this HBL), `$8000+/$9000+/$A000+` also trigger a 7-register palette-block
install from three other pointers used at transition points. The recolor
only remaps plain colour words (`new_r=old_r, new_g=old_b, new_b=old_r` —
green ramps in where blue did, a symmetric purple grows in where red did)
and leaves every command/sentinel word byte-for-byte untouched, so the
interrupt handler's control flow is unaffected (125 of 200 words changed;
the rest were the black lead-in or sentinel/hold codes).

**Lesson**: when a `watch()` hit's neighboring code doesn't match the effect
you're chasing (an 8-register block instead of a single-register per-HBL
write), don't trust the first hit — a register can be touched by more than
one routine. Check `disasm(..., source="live")` for named symbols near a
`bsr`/`jsr` before reaching for more `watch`/`xrefs` archaeology.

### Persistence — what survives a fresh boot, and why

| Patch | Address | Segment | Persists? |
|---|---|---|---|
| Logo source bitmap | `$00E08E` | TEXT (`$00AA9A`–`$03217C`) | **Yes** |
| Logo screen-buffer copies | `$032200`, `$03B300` | BSS | No (harmless, see below) |
| Scroller text buffer | `$02D640` (1632 B) | TEXT | **Yes** |
| Gradient `raster_table` | `$00031EFC` (400 B) | TEXT | **Yes** |
| Raster read cursor | `$0000DC7C` | TEXT code, relocated pointer | No (`skipped_reloc`) |

BSS isn't part of the `.PRG` file by definition (zero bytes on disk,
allocated at load time) — `save_prg` can't write there, and doesn't need to:
the logo screen buffers are runtime renders the program refills from
`$00E08E` every boot, so patching the *source* bitmap is enough. The one
genuine miss is `$0000DC7C`: a **relocated long-word pointer** baked into
the code by TOS's loader. `save_prg`'s repatch mode compares the live value
against what a fresh relocation would produce and refuses to persist it when
they differ (our transient mid-sweep cursor isn't the canonical reset
value) — logged as `"1 patched relocated pointer(s) NOT persisted (not
file-representable)"`. Harmless: the `$4712C`/`$DC6C` reset stub reloads
`$DC7C` from the canonical, correctly-relocated address every VBL regardless.

Verified by copying `STRGOOSE_hacked.PRG` alone into a fresh GEMDOS
hard-drive folder's `AUTO/` (plus the demo's non-code assets — fonts,
palettes, logo `.img`s — no original `.PRG`) and booting from that folder:
swapped logo, green→purple gradient, and SHIRAZMCP scroller all render
identically to the live-patched session.

### Running the hacked build

Bootable artifact: `$HOME/projects/MJJ/hacked/hd/AUTO/STRGOOSE_hacked.PRG`
(flat copy also at `$HOME/projects/MJJ/hacked/STRGOOSE_hacked.PRG`).
`start`'s `harddrive` parameter maps straight to Hatari's `--harddrive` flag
(`src/session/spawn/boot.rs`), which mounts a folder as a GEMDOS drive and
auto-runs anything in its `AUTO/` folder:

```
start(harddrive="$HOME/projects/MJJ/hacked/hd", machine="ste", memsize_mb=1)
```

Equivalent standalone Hatari invocation:

```
hatari --machine ste --memsize 1 --tos $HOME/projects/MJJ/bin/hatari/TOS/tos162fr.img \
       --harddrive $HOME/projects/MJJ/hacked/hd --fast-boot on
```
