//! Pure register/hardware-block -> `CpuRegs`/`HwRegs` mapping for `save_prg`
//! mode="snapshot" (P22 §8.3), split from `snapshot_prg.rs` for the
//! 200-line budget. No session/emulator access — everything here operates
//! on already-captured values.

use crate::debug::parse::MfpRegs;
use crate::debug::{Registers, SrFlags};
use crate::ripper::snapshot::{CpuRegs, HwRegs};

/// `SrFlags` -> raw SR word (M68000 UM bit layout): T=bit15 (`trace != 0`),
/// S=bit13, IPL=bits 10..8, X=4, N=3, Z=2, V=1, C=0.
pub(super) fn sr_word(f: &SrFlags) -> u16 {
    let mut sr = 0u16;
    if f.trace != 0 {
        sr |= 1 << 15;
    }
    if f.supervisor {
        sr |= 1 << 13;
    }
    sr |= (u16::from(f.imask) & 0x7) << 8;
    if f.x {
        sr |= 1 << 4;
    }
    if f.n {
        sr |= 1 << 3;
    }
    if f.z {
        sr |= 1 << 2;
    }
    if f.v {
        sr |= 1 << 1;
    }
    if f.c {
        sr |= 1;
    }
    sr
}

fn to_cpu_regs(regs: &Registers) -> CpuRegs {
    let mut a = [0u32; 7];
    a.copy_from_slice(&regs.a[..7]);
    CpuRegs {
        d: regs.d,
        a,
        usp: regs.usp,
        ssp: regs.isp,
        sr: sr_word(&regs.sr),
        pc: regs.pc.raw(),
    }
}

fn video_bytes(block: &[u8], memcfg: u8) -> [u8; 8] {
    [
        block[0x01],
        block[0x03],
        block[0x0D],
        block[0x0F],
        block[0x65],
        block[0x0A],
        block[0x60],
        memcfg,
    ]
}

fn video_palette(block: &[u8]) -> [u16; 16] {
    let mut pal = [0u16; 16];
    for (i, chunk) in block[0x40..0x60].chunks_exact(2).enumerate() {
        pal[i] = u16::from_be_bytes([chunk[0], chunk[1]]);
    }
    pal
}

fn mfp_bytes(m: &MfpRegs) -> [u8; 15] {
    [
        m.gpip, m.aer, m.ddr, m.iera, m.ierb, m.imra, m.imrb, m.vr, m.tacr, m.tbcr, m.tcdcr,
        m.tadr, m.tbdr, m.tcdr, m.tddr,
    ]
}

/// §5.1 `f_dmasnd` order: control, mode, start hi/mid/lo, end hi/mid/lo —
/// taken from the raw `$FF8900` block (`analyze::dmasnd`'s own offsets).
fn dmasnd_bytes(block: Option<&[u8]>) -> [u8; 8] {
    let Some(b) = block else {
        return [0; 8];
    };
    [
        b[0x01], b[0x21], b[0x03], b[0x05], b[0x07], b[0x0F], b[0x11], b[0x13],
    ]
}

const ROM_NOTE: &str =
    "snapshot assumes the same TOS/machine config — ROM contents are not captured";

pub(super) fn hole_warning(vr: u8) -> Option<String> {
    (vr & 0xF0 >= 0x70).then(|| {
        "MFP vector base overlaps the restore-stub hole — resumed MFP interrupts will crash"
            .to_string()
    })
}

pub(super) fn pending_interrupt_warning(mfp: &MfpRegs) -> Option<String> {
    (mfp.ipra | mfp.iprb | mfp.isra | mfp.isrb != 0).then(|| {
        "a hardware interrupt was pending/in-service at the freeze; it is lost".to_string()
    })
}

pub(super) fn ssp_warning(isp: u32, ram_len: u32) -> Option<String> {
    (isp < 0x10 || isp > ram_len).then(|| {
        "frozen SSP is implausible; the RTE frame write may corrupt live state".to_string()
    })
}

/// `debug::Registers` + captured blocks -> `(CpuRegs, HwRegs)` + the §4.2
/// conditional warnings computable without `ram_len` (the SSP-bounds
/// warning needs it and is added by the caller, which has it).
pub(super) fn to_capture(
    regs: &Registers,
    ym: [u8; 16],
    mfp: &MfpRegs,
    video_block: &[u8],
    memcfg: u8,
    dmasnd_block: Option<&[u8]>,
    ste: bool,
) -> (CpuRegs, HwRegs, Vec<String>) {
    let mut ym14 = [0u8; 14];
    ym14.copy_from_slice(&ym[..14]);
    let hw = HwRegs {
        ste,
        palette: video_palette(video_block),
        video: video_bytes(video_block, memcfg),
        ym: ym14,
        mfp: mfp_bytes(mfp),
        dmasnd: dmasnd_bytes(dmasnd_block),
    };
    let mut warnings: Vec<String> = hole_warning(mfp.vr).into_iter().collect();
    warnings.extend(pending_interrupt_warning(mfp));
    warnings.push(ROM_NOTE.to_string());
    (to_cpu_regs(regs), hw, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sr_flags(supervisor: bool, imask: u8, trace: u8) -> SrFlags {
        SrFlags {
            trace,
            supervisor,
            x: true,
            n: true,
            z: true,
            v: true,
            c: true,
            imask,
        }
    }

    #[test]
    fn sr_word_pinned() {
        assert_eq!(sr_word(&sr_flags(true, 7, 0)), 0x271F);
    }

    #[test]
    fn sr_word_user_traced() {
        let sr = sr_word(&sr_flags(false, 0, 1));
        assert_eq!(sr & 0x8000, 0x8000);
        assert_eq!(sr & 0x2000, 0);
    }

    fn video_block() -> [u8; 0x66] {
        let mut b = [0u8; 0x66];
        b[0x01] = 0x11;
        b[0x03] = 0x22;
        b[0x0D] = 0x33;
        b[0x0F] = 0x44;
        b[0x65] = 0x55;
        b[0x0A] = 0x66;
        b[0x60] = 0x77;
        for (i, w) in (0..16u16).enumerate() {
            let bytes = (w * 0x0101).to_be_bytes();
            b[0x40 + i * 2] = bytes[0];
            b[0x40 + i * 2 + 1] = bytes[1];
        }
        b
    }

    #[test]
    fn to_capture_maps_video_offsets() {
        let regs = Registers {
            d: [0; 8],
            a: [0; 8],
            usp: 0,
            isp: 0x2000,
            sr: sr_flags(true, 0, 0),
            pc: crate::types::Addr::new(0x1000),
        };
        let mfp = MfpRegs {
            gpip: 0,
            aer: 0,
            ddr: 0,
            iera: 0,
            ierb: 0,
            ipra: 0,
            iprb: 0,
            isra: 0,
            isrb: 0,
            imra: 0,
            imrb: 0,
            vr: 0x40,
            tacr: 0,
            tbcr: 0,
            tcdcr: 0,
            tadr: 0,
            tbdr: 0,
            tcdr: 0,
            tddr: 0,
        };
        let (_, hw, warnings) = to_capture(&regs, [0; 16], &mfp, &video_block(), 0x88, None, true);
        assert_eq!(hw.video, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
        assert_eq!(hw.palette[1], 0x0101);
        assert!(hw.ste); // ste flag passed through
        assert!(warnings.iter().any(|w| w == ROM_NOTE));
    }

    #[test]
    fn warning_triggers_vr_hole() {
        assert!(hole_warning(0x70).is_some());
        assert!(hole_warning(0x60).is_none());
    }

    #[test]
    fn warning_triggers_pending_interrupt() {
        let mut mfp = MfpRegs {
            gpip: 0,
            aer: 0,
            ddr: 0,
            iera: 0,
            ierb: 0,
            ipra: 1,
            iprb: 0,
            isra: 0,
            isrb: 0,
            imra: 0,
            imrb: 0,
            vr: 0,
            tacr: 0,
            tbcr: 0,
            tcdcr: 0,
            tadr: 0,
            tbdr: 0,
            tcdr: 0,
            tddr: 0,
        };
        assert!(pending_interrupt_warning(&mfp).is_some());
        mfp.ipra = 0;
        assert!(pending_interrupt_warning(&mfp).is_none());
    }

    #[test]
    fn warning_triggers_implausible_ssp() {
        assert!(ssp_warning(4, 0x10_0000).is_some());
        assert!(ssp_warning(0x2000, 0x10_0000).is_none());
        assert!(ssp_warning(0x20_0000, 0x10_0000).is_some());
    }
}
