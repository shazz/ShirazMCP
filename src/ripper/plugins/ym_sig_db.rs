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
    // a5,$134.w (re-arm Timer-A vector) — the SID-voice timer chain;
    // hand-assembled from Atari_ST_Sources/ASM/Reservoir Gods (RG)/
    // TRIPLEX.2K4/SNDH/DRIVER.S sound_timer1a..4a (~line 1113 ff.); no
    // binary available — // hand-assembled, verify live
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
