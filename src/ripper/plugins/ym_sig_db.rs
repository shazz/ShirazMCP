//! Corpus-derived YM player fingerprints (P19, ADR-35): the const
//! `SIGNATURES` DB consumed by `ym_signatures::identify`. Data only —
//! matcher/types live in `ym_signatures.rs`; adding a driver = one entry
//! here + one corpus-excerpt test below (extraction recipe:
//! docs/plan/P19.md §4.1 — scan labelled samples for `$FFFF8800`/`$8802`
//! operand bytes, decode the recurring gadget, mask relocated
//! operands/varying register fields, cite file@offset, test against the
//! pinned excerpt). Entries are descending-confidence; every confidence is
//! a 2-decimal literal > `CONF_ANCHOR_ONLY`. Unknown still degrades to
//! ("unknown", 0.25) in the matcher — a wrong or dead entry here can only
//! ever produce "unknown", never a wrong name.
//!
//! P24 (ADR-41) scaled the DB against the SNDH archive (refs/sndh_lf,
//! gitignored, fetch recipe refs/README.md); methodology: P19.md §4.1 +
//! P24.md §4 (group discrimination — beware wrapper shims, the archive's
//! most frequent PSG gadgets). Corpus regression: `tests/ym_sig_corpus.rs`.

use super::ym_signatures::{SigScope, SigWord, Signature};

const fn w(value: u16, mask: u16) -> SigWord {
    SigWord { value, mask }
}

/// P19 §4.3 — order, confidences, and provenance comments are pinned there.
pub const SIGNATURES: &[Signature] = &[
    // maxymiser-init — move.l #$08/09/0A000000,$8800.w (silence vols A/B/C),
    // movem.l (sp)+,d0/a0-a2, rts; source:
    // Atari_ST_Sources/ASM/Preromanbritain/SOURCE/MYM_REPL.BIN@0x14A
    // (+ MULTSNDH.SND@0x290, TEST_STE.SND@0x1DC, byte-identical)
    Signature {
        player: "maxYMiser",
        scope: SigScope::AnchorWindow,
        confidence: 0.85,
        pattern: &[
            w(0x21FC, 0xFFFF),
            w(0x0800, 0xFFFF),
            w(0x0000, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x21FC, 0xFFFF),
            w(0x0900, 0xFFFF),
            w(0x0000, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x21FC, 0xFFFF),
            w(0x0A00, 0xFFFF),
            w(0x0000, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x4CDF, 0xFFFF),
            w(0x0701, 0xFFFF),
            w(0x4E75, 0xFFFF),
        ],
    },
    // quartet-mute — mixer mute (Grazey's "Multi Quartet Player" shell
    // around the Quartet engine). Source: 607/Good_Old_Times.sndh@0xd80; 293 files.
    Signature {
        player: "Quartet",
        scope: SigScope::AnchorWindow,
        confidence: 0.85,
        pattern: &[
            w(0x3F00, 0xFFFF),
            w(0x11FC, 0xFFFF),
            w(0x0007, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x103C, 0xFFFF),
            w(0x0007, 0xFFFF),
            w(0x8038, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x11C0, 0xFFFF),
            w(0x8802, 0xFFFF),
            w(0x301F, 0xFFFF),
            w(0x4E75, 0xFFFF),
        ],
    },
    // whittaker-mute — read mixer, ori.b #$3F (all tone+noise off), store;
    // 24-bit operands. Source: Whittaker_David/APB.sndh@0x528; 61 files.
    Signature {
        player: "David Whittaker",
        scope: SigScope::AnchorWindow,
        confidence: 0.85,
        pattern: &[
            w(0x13FC, 0xFFFF),
            w(0x0007, 0xFFFF),
            w(0x00FF, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x1239, 0xFFFF),
            w(0x00FF, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x0001, 0xFFFF),
            w(0x003F, 0xFFFF),
            w(0x13C1, 0xFFFF),
            w(0x00FF, 0xFFFF),
            w(0x8802, 0xFFFF),
        ],
    },
    // tcb-clear — dbf loop zeroing R13..R0 — the replay *code* fingerprint
    // (tcb-dot/-bang below are module magic). Source: Count_Zero/Awesome.sndh@0x322; 22 files.
    Signature {
        player: "TCB Tracker",
        scope: SigScope::AnchorWindow,
        confidence: 0.85,
        pattern: &[
            w(0x700D, 0xFFFF),
            w(0x11C0, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x11FC, 0xFFFF),
            w(0x0000, 0xFFFF),
            w(0x8802, 0xFFFF),
            w(0x51C8, 0xFFFF),
            w(0xFFF4, 0xFFFF),
            w(0x11FC, 0xFFFF),
            w(0x0007, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x11C1, 0xFFFF),
            w(0x8802, 0xFFFF),
            w(0x4E75, 0xFFFF),
        ],
    },
    // hippel-runl — movem.l $1C(a6),d0-d3, 4-long unrolled PSG blast; $001C
    // is literal (byte-identical in 174 hits). Source: Mad_Max/Car_Race.sndh@0x1e8; 174 files.
    Signature {
        player: "Jochen Hippel (Mad Max)",
        scope: SigScope::AnchorWindow,
        confidence: 0.85,
        pattern: &[
            w(0x4CEE, 0xFFFF),
            w(0x000F, 0xFFFF),
            w(0x001C, 0xFFFF),
            w(0x21C0, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x21C1, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x21C2, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x21C3, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x4E75, 0xFFFF),
        ],
    },
    // hippel-loop24 — dbf loop writing R12..R0, 24-bit operands (needs
    // is_psg fix, §6, to anchor live). Source:
    // Mad_Max/Demos/Best_In_Galaxy/Ace_2.sndh@0xdc2; 130 files.
    Signature {
        player: "Jochen Hippel (Mad Max)",
        scope: SigScope::AnchorWindow,
        confidence: 0.85,
        pattern: &[
            w(0x303C, 0xFFFF),
            w(0x000C, 0xFFFF),
            w(0x13C0, 0xFFFF),
            w(0x00FF, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x13E0, 0xFFFF),
            w(0x00FF, 0xFFFF),
            w(0x8802, 0xFFFF),
            w(0x51C8, 0xFFFF),
            w(0xFFF2, 0xFFFF),
            w(0x4E75, 0xFFFF),
        ],
    },
    // follin-mixer — masked mixer update (andi.b #$C0/#$3F, or.b, store),
    // 24-bit operands. Source: Follin_Tim/Bionic_Commando.sndh@0xd1a; 6 files.
    Signature {
        player: "Tim Follin",
        scope: SigScope::AnchorWindow,
        confidence: 0.85,
        pattern: &[
            w(0x13C0, 0xFFFF),
            w(0x00FF, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x1239, 0xFFFF),
            w(0x00FF, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x0201, 0xFFFF),
            w(0x00C0, 0xFFFF),
            w(0x0202, 0xFFFF),
            w(0x003F, 0xFFFF),
            w(0x8202, 0xFFFF),
            w(0x13C1, 0xFFFF),
            w(0x00FF, 0xFFFF),
            w(0x8802, 0xFFFF),
        ],
    },
    // hippel-movem-w — movem.l d0-d3,$8800.w, rts (the Mad Max 4-long PSG
    // blast: the mirrored PSG decodes each long as select+data); source:
    // Atari_ST_Sources/ASM/Various/Mug UK/JOCHEN.HIP/ATOMINO/ATOMINO1.MUS@0x17E
    // (+ every JOCHEN.HIP rip and MADMAX/COSO1 binary, e.g.
    // COSO1/MAD3.MUS@0x15C — 20+ files)
    Signature {
        player: "Jochen Hippel (Mad Max)",
        scope: SigScope::AnchorWindow,
        confidence: 0.82,
        pattern: &[
            w(0x48F8, 0xFFFF),
            w(0x000F, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x4E75, 0xFFFF),
        ],
    },
    // hippel-movem-l-long — movem.l (aN),d0-d7/a0-a2 (source reg masked: a6
    // and a4 observed), movem.l d0-d7/a0-a2,$FFFF8800.l, rts (11-long
    // blast); source: Atari_ST_Sources/ASM/Mcoder/unreleased/MADMAX/TFMX1/
    // UNION5.MUS@0x36 (+ TFMX1/WHAT16.IMG@0xAA, identical)
    Signature {
        player: "Jochen Hippel (Mad Max)",
        scope: SigScope::AnchorWindow,
        confidence: 0.82,
        pattern: &[
            w(0x4CD0, 0xFFF8),
            w(0x07FF, 0xFFFF),
            w(0x48F9, 0xFFFF),
            w(0x07FF, 0xFFFF),
            w(0xFFFF, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x4E75, 0xFFFF),
        ],
    },
    // hippel-movem-l-short — same blast, absolute-short destination;
    // source: Atari_ST_Sources/ASM/Various/Mug UK/JOCHEN.HIP/INSECTS/
    // INSECTFX.MUS@0x1CC (single sample, but >=2 counting the long-form
    // family — the two forms are one routine assembled two ways, kept at
    // 0.82)
    Signature {
        player: "Jochen Hippel (Mad Max)",
        scope: SigScope::AnchorWindow,
        confidence: 0.82,
        pattern: &[
            w(0x4CD0, 0xFFF8),
            w(0x07FF, 0xFFFF),
            w(0x48F8, 0xFFFF),
            w(0x07FF, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x4E75, 0xFFFF),
        ],
    },
    // whittaker-read — read mixer, move.b d16(pc),d3 (opcode word only —
    // displacement is layout-dependent, deliberately unmasked/excluded).
    // Source: Whittaker_David/APB.sndh@0x4f2; 61 files.
    Signature {
        player: "David Whittaker",
        scope: SigScope::AnchorWindow,
        confidence: 0.82,
        pattern: &[
            w(0x13FC, 0xFFFF),
            w(0x0007, 0xFFFF),
            w(0x00FF, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x1039, 0xFFFF),
            w(0x00FF, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x163A, 0xFFFF),
        ],
    },
    // jess-init — movem save, lea $8800.w,a0, mask IRQs before the
    // register loop. Source: Jess/A_Sixties_Slow.sndh@0x1a0; 54 files.
    Signature {
        player: "Jess (Overlanders)",
        scope: SigScope::AnchorWindow,
        confidence: 0.82,
        pattern: &[
            w(0x48E7, 0xFFFF),
            w(0xC080, 0xFFFF),
            w(0x700D, 0xFFFF),
            w(0x41F8, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x40E7, 0xFFFF),
            w(0x7200, 0xFFFF),
            w(0x46FC, 0xFFFF),
            w(0x2700, 0xFFFF),
        ],
    },
    // tao-silence — select R10, volume C := 0, then a state-flag store.
    // Source: Tao/13_(UCM_13).sndh@0x396; 132 files (Tao + 2 collabs).
    Signature {
        player: "Tao (tSCc)",
        scope: SigScope::AnchorWindow,
        confidence: 0.82,
        pattern: &[
            w(0x21FC, 0xFFFF),
            w(0x0A00, 0xFFFF),
            w(0x0000, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x397C, 0xFFFF),
            w(0x0001, 0xFFFF),
            w(0x0010, 0xFFFF),
            w(0x4E75, 0xFFFF),
        ],
    },
    // bigalec-blast — head of a 14-register PSG blast (Hippel-style; 4CD0
    // literal, only (a0) observed). Source: Big_Alec/A_Case_For_Two.sndh@0x34a; 18 files.
    Signature {
        player: "Big Alec",
        scope: SigScope::AnchorWindow,
        confidence: 0.82,
        pattern: &[
            w(0x4CD0, 0xFFFF),
            w(0x3FFF, 0xFFFF),
            w(0x21C8, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x21C9, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x21CA, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x21CB, 0xFFFF),
            w(0x8800, 0xFFFF),
        ],
    },
    // musicmon-env — pc-relative song-data read (displacement masked),
    // envelope shape from data. Source: Vidovic_Augustin/Music_Mon_Intro.sndh@0x5dc; 26 files.
    Signature {
        player: "Music Mon",
        scope: SigScope::AnchorWindow,
        confidence: 0.82,
        pattern: &[
            w(0x363A, 0xFFFF),
            w(0x0000, 0x0000),
            w(0x11FC, 0xFFFF),
            w(0x000D, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x11C3, 0xFFFF),
            w(0x8802, 0xFFFF),
        ],
    },
    // tcb-dot — ASCII "AN COOL." (unchanged bytes from P6, provenance
    // upgraded from documented to corpus-cited); source: first 8 bytes of
    // Atari_ST_Sources/ASM/Adrenalin U.K (ADR)/ADR34/TCB.MOD (+
    // Hextracker_0.849/TCB_MOD/SCOOPEX.MOD, STOS/Crystallic/TCB.MOD,
    // ASM/Mystic/MODULE.MOD, ASM/Crystalic/FREAKY.MOD)
    Signature {
        player: "TCB Tracker",
        scope: SigScope::WholeImage,
        confidence: 0.80,
        pattern: &[
            w(0x414E, 0xFFFF),
            w(0x2043, 0xFFFF),
            w(0x4F4F, 0xFFFF),
            w(0x4C2E, 0xFFFF),
        ],
    },
    // tcb-bang — ASCII "AN COOL!"; provisional — kept: documented variant
    // (skill atari-st-ym-players §3), zero local module attests it —
    // // verify live
    Signature {
        player: "TCB Tracker",
        scope: SigScope::WholeImage,
        confidence: 0.80,
        pattern: &[
            w(0x414E, 0xFFFF),
            w(0x2043, 0xFFFF),
            w(0x4F4F, 0xFFFF),
            w(0x4C21, 0xFFFF),
        ],
    },
    // rg-triplex — move.l a5,-(sp), move.l #$0AFF00FF,$8800.w (select R10,
    // volume C := 0), lea next_timer(pc),a5 (displacement masked), move.l
    // a5,$134.w (re-arm Timer-A vector) — the SID-voice timer chain, from
    // ASM/Reservoir Gods (RG)/TRIPLEX.2K4/SNDH/DRIVER.S sound_timer1a..4a;
    // corpus-verified against Damo/YM_Rucka.sndh@0xb64 (Damo is Reservoir
    // Gods); confidence stays 0.70 (still a single observed binary)
    Signature {
        player: "Reservoir Gods driver",
        scope: SigScope::AnchorWindow,
        confidence: 0.70,
        pattern: &[
            w(0x2F0D, 0xFFFF),
            w(0x21FC, 0xFFFF),
            w(0x0AFF, 0xFFFF),
            w(0x00FF, 0xFFFF),
            w(0x8800, 0xFFFF),
            w(0x4BFA, 0xFFFF),
            w(0x0000, 0x0000),
            w(0x21CD, 0xFFFF),
            w(0x0134, 0xFFFF),
        ],
    },
    // tfmx — ASCII "TFMX" + NUL guard byte (third word's high byte must be
    // $00; P6's unguarded 2-word entry matched the word TFMX in any
    // scrolltext); source: embedded data magic at Atari_ST_Sources/ASM/
    // Mcoder/unreleased/MADMAX/TFMX1/UNION5.MUS@0x1BA0 (+
    // TFMX1/WHAT16.IMG@0x13F4, both "TFMX\0")
    Signature {
        player: "Jochen Hippel (Mad Max)",
        scope: SigScope::WholeImage,
        confidence: 0.65,
        pattern: &[w(0x5446, 0xFFFF), w(0x4D58, 0xFFFF), w(0x0000, 0xFF00)],
    },
    // maxymiser-string — ASCII "maxYMise" (unchanged bytes from P6,
    // provenance upgraded); source: MYM_REPL.BIN@0x17: "-*- maxYMiser
    // replay routine by gwEm (Gareth Morris) (c) 2008 -*-" (also in both
    // .SND exports)
    Signature {
        player: "maxYMiser",
        scope: SigScope::WholeImage,
        confidence: 0.60,
        pattern: &[
            w(0x6D61, 0xFFFF),
            w(0x7859, 0xFFFF),
            w(0x4D69, 0xFFFF),
            w(0x7365, 0xFFFF),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ripper::plugins::ym_signatures::{identify, CONF_ANCHOR_ONLY};

    #[test]
    fn maxymiser_init_matches_corpus_excerpt() {
        // Atari_ST_Sources/ASM/Preromanbritain/SOURCE/MYM_REPL.BIN@0x14A
        let buf = [
            0x21, 0xFC, 0x08, 0x00, 0x00, 0x00, 0x88, 0x00, 0x21, 0xFC, 0x09, 0x00, 0x00, 0x00,
            0x88, 0x00, 0x21, 0xFC, 0x0A, 0x00, 0x00, 0x00, 0x88, 0x00, 0x4C, 0xDF, 0x07, 0x01,
            0x4E, 0x75,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "maxYMiser");
        assert!((id.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn quartet_mute_matches_corpus_excerpt() {
        // 607/Good_Old_Times.sndh@0xd80
        let buf = [
            0x3F, 0x00, 0x11, 0xFC, 0x00, 0x07, 0x88, 0x00, 0x10, 0x3C, 0x00, 0x07, 0x80, 0x38,
            0x88, 0x00, 0x11, 0xC0, 0x88, 0x02, 0x30, 0x1F, 0x4E, 0x75,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "Quartet");
        assert!((id.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn whittaker_mute_matches_corpus_excerpt() {
        // Whittaker_David/APB.sndh@0x528
        let buf = [
            0x13, 0xFC, 0x00, 0x07, 0x00, 0xFF, 0x88, 0x00, 0x12, 0x39, 0x00, 0xFF, 0x88, 0x00,
            0x00, 0x01, 0x00, 0x3F, 0x13, 0xC1, 0x00, 0xFF, 0x88, 0x02,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "David Whittaker");
        assert!((id.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn tcb_clear_matches_corpus_excerpt() {
        // Count_Zero/Awesome.sndh@0x322
        let buf = [
            0x70, 0x0D, 0x11, 0xC0, 0x88, 0x00, 0x11, 0xFC, 0x00, 0x00, 0x88, 0x02, 0x51, 0xC8,
            0xFF, 0xF4, 0x11, 0xFC, 0x00, 0x07, 0x88, 0x00, 0x11, 0xC1, 0x88, 0x02, 0x4E, 0x75,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "TCB Tracker");
        assert!((id.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn hippel_runl_matches_corpus_excerpt() {
        // Mad_Max/Car_Race.sndh@0x1e8
        let buf = [
            0x4C, 0xEE, 0x00, 0x0F, 0x00, 0x1C, 0x21, 0xC0, 0x88, 0x00, 0x21, 0xC1, 0x88, 0x00,
            0x21, 0xC2, 0x88, 0x00, 0x21, 0xC3, 0x88, 0x00, 0x4E, 0x75,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "Jochen Hippel (Mad Max)");
        assert!((id.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn hippel_loop24_matches_corpus_excerpt() {
        // Mad_Max/Demos/Best_In_Galaxy/Ace_2.sndh@0xdc2
        let buf = [
            0x30, 0x3C, 0x00, 0x0C, 0x13, 0xC0, 0x00, 0xFF, 0x88, 0x00, 0x13, 0xE0, 0x00, 0xFF,
            0x88, 0x02, 0x51, 0xC8, 0xFF, 0xF2, 0x4E, 0x75,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "Jochen Hippel (Mad Max)");
        assert!((id.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn follin_mixer_matches_corpus_excerpt() {
        // Follin_Tim/Bionic_Commando.sndh@0xd1a
        let buf = [
            0x13, 0xC0, 0x00, 0xFF, 0x88, 0x00, 0x12, 0x39, 0x00, 0xFF, 0x88, 0x00, 0x02, 0x01,
            0x00, 0xC0, 0x02, 0x02, 0x00, 0x3F, 0x82, 0x02, 0x13, 0xC1, 0x00, 0xFF, 0x88, 0x02,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "Tim Follin");
        assert!((id.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn hippel_movem_w_matches_corpus_excerpt() {
        // ASM/Various/Mug UK/JOCHEN.HIP/ATOMINO/ATOMINO1.MUS@0x17E
        let buf = [0x48, 0xF8, 0x00, 0x0F, 0x88, 0x00, 0x4E, 0x75];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "Jochen Hippel (Mad Max)");
        assert!((id.confidence - 0.82).abs() < 1e-6);
    }

    #[test]
    fn hippel_movem_l_long_matches_corpus_excerpt() {
        // ASM/Mcoder/unreleased/MADMAX/TFMX1/UNION5.MUS@0x36
        let buf = [
            0x4C, 0xD6, 0x07, 0xFF, 0x48, 0xF9, 0x07, 0xFF, 0xFF, 0xFF, 0x88, 0x00, 0x4E, 0x75,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "Jochen Hippel (Mad Max)");
        assert!((id.confidence - 0.82).abs() < 1e-6);
    }

    #[test]
    fn hippel_movem_l_short_matches_corpus_excerpt() {
        // ASM/Various/Mug UK/JOCHEN.HIP/INSECTS/INSECTFX.MUS@0x1CC
        let buf = [
            0x4C, 0xD4, 0x07, 0xFF, 0x48, 0xF8, 0x07, 0xFF, 0x88, 0x00, 0x4E, 0x75,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "Jochen Hippel (Mad Max)");
        assert!((id.confidence - 0.82).abs() < 1e-6);
    }

    #[test]
    fn whittaker_read_matches_corpus_excerpt() {
        // Whittaker_David/APB.sndh@0x4f2 (observed displacement $0449)
        let buf = [
            0x13, 0xFC, 0x00, 0x07, 0x00, 0xFF, 0x88, 0x00, 0x10, 0x39, 0x00, 0xFF, 0x88, 0x00,
            0x16, 0x3A, 0x04, 0x49,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "David Whittaker");
        assert!((id.confidence - 0.82).abs() < 1e-6);
    }

    #[test]
    fn jess_init_matches_corpus_excerpt() {
        // Jess/A_Sixties_Slow.sndh@0x1a0
        let buf = [
            0x48, 0xE7, 0xC0, 0x80, 0x70, 0x0D, 0x41, 0xF8, 0x88, 0x00, 0x40, 0xE7, 0x72, 0x00,
            0x46, 0xFC, 0x27, 0x00,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "Jess (Overlanders)");
        assert!((id.confidence - 0.82).abs() < 1e-6);
    }

    #[test]
    fn tao_silence_matches_corpus_excerpt() {
        // Tao/13_(UCM_13).sndh@0x396
        let buf = [
            0x21, 0xFC, 0x0A, 0x00, 0x00, 0x00, 0x88, 0x00, 0x39, 0x7C, 0x00, 0x01, 0x00, 0x10,
            0x4E, 0x75,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "Tao (tSCc)");
        assert!((id.confidence - 0.82).abs() < 1e-6);
    }

    #[test]
    fn bigalec_blast_matches_corpus_excerpt() {
        // Big_Alec/A_Case_For_Two.sndh@0x34a
        let buf = [
            0x4C, 0xD0, 0x3F, 0xFF, 0x21, 0xC8, 0x88, 0x00, 0x21, 0xC9, 0x88, 0x00, 0x21, 0xCA,
            0x88, 0x00, 0x21, 0xCB, 0x88, 0x00,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "Big Alec");
        assert!((id.confidence - 0.82).abs() < 1e-6);
    }

    #[test]
    fn musicmon_env_matches_corpus_excerpt() {
        // Vidovic_Augustin/Music_Mon_Intro.sndh@0x5dc
        let buf = [
            0x36, 0x3A, 0x07, 0xF8, 0x11, 0xFC, 0x00, 0x0D, 0x88, 0x00, 0x11, 0xC3, 0x88, 0x02,
        ];
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "Music Mon");
        assert!((id.confidence - 0.82).abs() < 1e-6);
    }

    #[test]
    fn musicmon_displacement_masked() {
        // Same excerpt with the displacement (bytes 2-3) patched to two
        // different layout-dependent values — both must still match.
        let base = [
            0x36, 0x3A, 0x07, 0xF8, 0x11, 0xFC, 0x00, 0x0D, 0x88, 0x00, 0x11, 0xC3, 0x88, 0x02,
        ];
        for disp in [[0x01, 0x00], [0xFF, 0x2E]] {
            let mut buf = base;
            buf[2] = disp[0];
            buf[3] = disp[1];
            let id = identify(&buf, 0, buf.len());
            assert_eq!(id.player, "Music Mon");
            assert!((id.confidence - 0.82).abs() < 1e-6);
        }
    }

    #[test]
    fn hippel_movem_load_register_masked() {
        let base = [
            0x4C, 0xD6, 0x07, 0xFF, 0x48, 0xF9, 0x07, 0xFF, 0xFF, 0xFF, 0x88, 0x00, 0x4E, 0x75,
        ];
        for reg in 0u8..=7 {
            let mut buf = base;
            buf[1] = 0xD0 | reg;
            let id = identify(&buf, 0, buf.len());
            assert_eq!(id.player, "Jochen Hippel (Mad Max)", "a{reg} should match");
        }
        let mut buf = base;
        buf[1] = 0xD8; // (aN)+ mode, not the direct-register class the mask allows
        let id = identify(&buf, 0, buf.len());
        assert_eq!(id.player, "unknown");
    }

    #[test]
    fn tcb_dot_matches_real_module_head() {
        let mut buf = vec![0u8; 0x100];
        // first 16 bytes of ASM/Adrenalin U.K (ADR)/ADR34/TCB.MOD
        buf[0x40..0x40 + 16].copy_from_slice(b"AN COOL.\0\0\0\x11\t\0\0\x01");
        let id = identify(&buf, 0x80, 0x84);
        assert_eq!(id.player, "TCB Tracker");
        assert!((id.confidence - 0.80).abs() < 1e-6);
    }

    #[test]
    fn tfmx_guard_requires_nul() {
        // UNION5.MUS@0x1BA0
        let matches = [0x54, 0x46, 0x4D, 0x58, 0x00, 0x0D];
        let id = identify(&matches, 0, matches.len());
        assert_eq!(id.player, "Jochen Hippel (Mad Max)");
        assert!((id.confidence - 0.65).abs() < 1e-6);

        let no_match = b"TFMX IS COOL"; // scrolltext shape, no NUL guard
        let id = identify(no_match, 0, no_match.len());
        assert_eq!(id.player, "unknown");
    }

    #[test]
    fn rg_triplex_displacement_masked() {
        for disp in [[0x00, 0x08], [0xFF, 0x2E]] {
            let mut buf = [
                0x2F, 0x0D, 0x21, 0xFC, 0x0A, 0xFF, 0x00, 0xFF, 0x88, 0x00, 0x4B, 0xFA, 0, 0, 0x21,
                0xCD, 0x01, 0x34,
            ];
            buf[12] = disp[0];
            buf[13] = disp[1];
            let id = identify(&buf, 0, buf.len());
            assert_eq!(id.player, "Reservoir Gods driver");
            assert!((id.confidence - 0.70).abs() < 1e-6);
        }
    }

    #[test]
    fn maxymiser_string_matches_author_banner() {
        // MYM_REPL.BIN@0x13
        let s = b"-*- maxYMiser replay routine by gwEm";
        let mut buf = vec![0u8; s.len() + 1];
        buf[1..].copy_from_slice(s); // planted at an odd offset
        let id = identify(&buf, 0, 0);
        assert_eq!(id.player, "maxYMiser");
        assert!((id.confidence - 0.60).abs() < 1e-6);
    }

    #[test]
    fn unrelated_code_stays_unknown() {
        let mut buf = vec![0u8; 64];
        buf[0..10].copy_from_slice(&[0x11, 0xC0, 0x88, 0x00, 0x11, 0xC0, 0x88, 0x02, 0x4E, 0x75]);
        let tail = b"QUARTET RULES AN COOL ";
        buf[10..10 + tail.len()].copy_from_slice(tail);
        let id = identify(&buf, 0, 10);
        assert_eq!(id.player, "unknown");
        assert!((id.confidence - CONF_ANCHOR_ONLY).abs() < 1e-6);
    }

    #[test]
    fn db_is_sane() {
        let mut prev = f32::MAX;
        for sig in SIGNATURES {
            assert!(!sig.pattern.is_empty());
            assert!(sig.confidence > CONF_ANCHOR_ONLY && sig.confidence <= 0.85);
            assert!(!sig.player.is_empty());
            assert_ne!(sig.player, "unknown");
            assert_eq!(
                format!("{:.2}", sig.confidence).parse::<f32>(),
                Ok(sig.confidence)
            );
            assert!(sig.confidence <= prev, "DB must be descending-confidence");
            prev = sig.confidence;
        }
    }
}
