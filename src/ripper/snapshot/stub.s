; ShirazMCP P22 snapshot restore stub.
; Two stages: stage0 runs at the GEMDOS load address (position-independent);
; the hole block [fields..payload) is copied to HOLE_BASE and survives the
; full-RAM decode sweep. Heritage: r1pp3r's preamble/trampoline
; (R1PP3R.S 5253-5748); the crash page $380-$3FC precedent is TRAMP_DEST
; (R1PP3R.S 5703). See docs/plan/P22.md §4.5 for the memory layout.
;
; Layout (normative): stage0 code, then fields (starts with magic "SNP1"),
; then hole_code, then label `payload` (end of blob; Rust appends the SNAP1
; stream there). The hole block copied to HOLE_BASE is [fields, payload).

        org     0

HOLE_BASE   = $1C0          ; vectors 112..255 + crash page — never raised on ST(E)

; ---------------------------------------------------------------- stage 0
stub_start:
        clr.l   -(a7)               ; Super(0)
        move.w  #$20,-(a7)
        trap    #1
        addq.l  #6,a7

        move.l  $42e.w,d0           ; phystop
        cmp.l   f_region_end(pc),d0
        bcc.s   .fits
        pea     .msg(pc)            ; Cconws("snapshot needs more RAM")
        move.w  #9,-(a7)
        trap    #1
        addq.l  #6,a7
        clr.w   -(a7)               ; Pterm0   // verify live: from supervisor
        trap    #1
.fits:
        move.w  #37,-(a7)           ; Vsync — reduce takeover tearing
        trap    #14
        addq.l  #2,a7

        move.w  #$2700,sr           ; mask everything; no stack use below

        ; copy hole block (fields + code) to HOLE_BASE — fwd copy, no overlap
        lea     fields(pc),a0
        lea     HOLE_BASE.w,a1
        move.w  #(payload-fields)/2-1,d0
.cph:   move.w  (a0)+,(a1)+
        dbf     d0,.cph

        ; stage payload flush against top of frozen RAM: [R-P, R)
        ; backward copy (r1pp3r .cp_bwd) — dest > src always (§4.5)
        lea     payload(pc),a0
        move.l  f_payload_len(pc),d0
        adda.l  d0,a0               ; src end
        movea.l f_region_end(pc),a1 ; dest end = R
.cpp:   move.b  -(a0),-(a1)
        subq.l  #1,d0
        bne.s   .cpp

        jmp     (HOLE_BASE+(hole_code-fields)).w

.msg:   dc.b    13,10,"Snapshot needs more RAM than this machine has.",13,10,0
        even

; ------------------------------------------------------- fields (§5.1)
fields:
f_magic:        dc.b    "SNP1"
f_flags:        dc.w    0
f_region_end:   dc.l    0
f_payload_len:  dc.l    0
f_pc:           dc.l    0
f_sr:           dc.w    0
f_usp:          dc.l    0
f_ssp:          dc.l    0
f_dregs:        ds.l    8
f_aregs:        ds.l    7
f_pal:          ds.w    16
f_video:        ds.b    8           ; basehi,basemid,baselo,linewidth,hscroll,sync,res,memcfg
f_ym:           ds.b    16          ; regs 0-13 + pad
f_mfp:          ds.b    16          ; gpip,aer,ddr,iera,ierb,imra,imrb,vr,tacr,tbcr,tcdcr,tadr,tbdr,tcdr,tddr,pad
f_dmasnd:       ds.b    8           ; control,mode,starthi,startmid,startlo,endhi,endmid,endlo
        even

; ---------------------------------------------- hole code (runs at $1C0+)
; RULES: no stack, no subroutines, PC-relative data only (offsets survive
; the copy because fields+code move together).
hole_code:
        ; ---- SNAP1 decode: input a0 = R-P, output a1 = 0, sweep to R
        movea.l f_region_end(pc),a0
        suba.l  f_payload_len(pc),a0
        suba.l  a1,a1
.op:    moveq   #0,d1
        move.b  (a0)+,d0
        cmpi.b  #3,d0
        beq.s   .tail
        move.b  (a0)+,d1            ; u24 big-endian length
        lsl.l   #8,d1
        move.b  (a0)+,d1
        lsl.l   #8,d1
        move.b  (a0)+,d1
        tst.b   d0
        beq.s   .zero
        cmpi.b  #1,d0
        beq.s   .lit
        adda.l  d1,a1               ; 02 = skip (the hole itself)
        bra.s   .op
.zero:  clr.b   (a1)+
        subq.l  #1,d1
        bne.s   .zero
        bra.s   .op
.lit:   move.b  (a0)+,(a1)+
        subq.l  #1,d1
        bne.s   .lit
        bra.s   .op
; ADR-38: 0x03 = zero-fill output until it catches up to the input cursor
; (a1==a0 -- the terminal gap the packer absorbed for free, 0 stream
; bytes), then lockstep-copy remaining input to region_end. Safe
; unconditionally: the fill only ever touches [o,i), strictly below
; unread input.
.tail:
.zfill: cmpa.l  a0,a1
        beq.s   .cpy
        clr.b   (a1)+
        bra.s   .zfill
.cpy:   cmpa.l  f_region_end(pc),a1
        beq.s   .hw
        move.b  (a0)+,(a1)+
        bra.s   .cpy

        ; ---- hardware reinstall (order is normative)
.hw:    lea     f_video(pc),a2
        move.b  7(a2),$ffff8001.w   ; memory config
        move.b  (a2),$ffff8201.w    ; video base high
        move.b  1(a2),$ffff8203.w   ; video base mid
        move.w  f_flags(pc),d0
        btst    #0,d0
        beq.s   .nst1
        move.b  2(a2),$ffff820d.w   ; STE base low
        move.b  3(a2),$ffff820f.w   ; STE linewidth
        move.b  4(a2),$ffff8265.w   ; STE hscroll
.nst1:  move.b  5(a2),$ffff820a.w   ; sync (50/60)
        move.b  6(a2),$ffff8260.w   ; resolution
        lea     f_pal(pc),a2        ; 16 palette words
        lea     $ffff8240.w,a3
        moveq   #7,d1
.pal:   move.l  (a2)+,(a3)+
        dbf     d1,.pal

        lea     f_ym(pc),a2         ; YM 0..13 — reg 13 written last (retrigger)
        lea     $ffff8800.w,a3
        moveq   #0,d1
.ym:    move.b  d1,(a3)
        move.b  (a2)+,2(a3)
        addq.b  #1,d1
        cmpi.b  #14,d1
        bne.s   .ym

        ; MFP: stop -> clear -> static regs -> reloads -> control -> IER -> IMR
        clr.b   $fffffa19.w         ; TACR
        clr.b   $fffffa1b.w         ; TBCR
        clr.b   $fffffa1d.w         ; TCDCR
        clr.b   $fffffa07.w         ; IERA=0 (clears pending too)
        clr.b   $fffffa09.w         ; IERB
        clr.b   $fffffa0b.w         ; IPRA
        clr.b   $fffffa0d.w         ; IPRB
        clr.b   $fffffa0f.w         ; ISRA
        clr.b   $fffffa11.w         ; ISRB
        move.b  f_mfp+0(pc),$fffffa01.w   ; GPIP
        move.b  f_mfp+1(pc),$fffffa03.w   ; AER
        move.b  f_mfp+2(pc),$fffffa05.w   ; DDR
        move.b  f_mfp+7(pc),$fffffa17.w   ; VR
        move.b  f_mfp+11(pc),$fffffa1f.w  ; TADR (reload)
        move.b  f_mfp+12(pc),$fffffa21.w  ; TBDR
        move.b  f_mfp+13(pc),$fffffa23.w  ; TCDR
        move.b  f_mfp+14(pc),$fffffa25.w  ; TDDR
        move.b  f_mfp+8(pc),$fffffa19.w   ; TACR (starts timer A)
        move.b  f_mfp+9(pc),$fffffa1b.w   ; TBCR
        move.b  f_mfp+10(pc),$fffffa1d.w  ; TCDCR
        move.b  f_mfp+3(pc),$fffffa07.w   ; IERA
        move.b  f_mfp+4(pc),$fffffa09.w   ; IERB
        move.b  f_mfp+5(pc),$fffffa13.w   ; IMRA
        move.b  f_mfp+6(pc),$fffffa15.w   ; IMRB

        move.w  f_flags(pc),d0      ; STE DMA sound
        btst    #0,d0
        beq.s   .nst2
        clr.b   $ffff8901.w
        move.b  f_dmasnd+2(pc),$ffff8903.w
        move.b  f_dmasnd+3(pc),$ffff8905.w
        move.b  f_dmasnd+4(pc),$ffff8907.w
        move.b  f_dmasnd+5(pc),$ffff890f.w
        move.b  f_dmasnd+6(pc),$ffff8911.w
        move.b  f_dmasnd+7(pc),$ffff8913.w
        move.b  f_dmasnd+1(pc),$ffff8921.w
        move.b  f_dmasnd+0(pc),$ffff8901.w
.nst2:
        ; ---- CPU context + launch (r1pp3r RTE model, R1PP3R.S 5641-5675)
        movea.l f_usp(pc),a0
        move    a0,usp
        movea.l f_ssp(pc),a7
        move.l  f_pc(pc),-(a7)      ; frame: [SSP-6,SSP) = SR.w + PC.l
        move.w  f_sr(pc),-(a7)
        movem.l f_dregs(pc),d0-d7
        movem.l f_aregs(pc),a0-a6
        rte
payload:                            ; Rust appends the SNAP1 stream here
