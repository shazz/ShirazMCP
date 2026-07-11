; Tiny GEMDOS AUTO-folder fixture for tests/freeze_drain_live.rs: a minimal
; infinite loop with real DRI/GST-visible labels, assembled with vasm's
; `-Ftos` backend (`vasmm68k_mot -Ftos -quiet -spaces -o SYMTEST.PRG
; symtest.s`). Unlike MJJHACK.PRG (ADR-8) and the DHS "Uhane" demo's
; UHANE.PRG (both stripped/packed — `symbols prg` reports "symbol table
; missing from the program!" for either), vasm keeps ordinary code/data
; labels in the output's symbol table by default, so this genuinely
; exercises Hatari's DebugUI symbol-load banner on every debugger entry —
; the condition the freeze-drain fix (ADR-11 addendum) is proven against.
; A few dozen labels are enough to be a REAL, non-trivial symbol table
; (`symbols prg` prints "Loaded N symbols..."); the live race that actually
; corrupts the first post-freeze command turned out to need CPU contention
; more than symbol-table size (see freeze_drain_live.rs's doc comment) —
; keeping this small keeps the fixture cheap to assemble and commit.
; Never returns (GEMDOS AUTO execution just parks here), so the CPU is
; always sitting somewhere stable inside `loop` for `pause()` to freeze.

                    section     TEXT

start:
routine_000:
                    nop
routine_001:
                    nop
routine_002:
                    nop
routine_003:
                    nop
routine_004:
                    nop
routine_005:
                    nop
routine_006:
                    nop
routine_007:
                    nop
routine_008:
                    nop
routine_009:
                    nop
routine_010:
                    nop
routine_011:
                    nop
routine_012:
                    nop
routine_013:
                    nop
routine_014:
                    nop
routine_015:
                    nop
routine_016:
                    nop
routine_017:
                    nop
routine_018:
                    nop
routine_019:
                    nop
routine_020:
                    nop
routine_021:
                    nop
routine_022:
                    nop
routine_023:
                    nop
routine_024:
                    nop
routine_025:
                    nop
routine_026:
                    nop
routine_027:
                    nop
routine_028:
                    nop
routine_029:
                    nop
routine_030:
                    nop
routine_031:
                    nop
loop:
                    addq.w  #1,d0
                    bra.s   loop

                    section     DATA
marker:
                    dc.l    $12345678
