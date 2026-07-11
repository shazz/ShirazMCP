//! `info mfp` (MFP 68901 internal-struct dump) parsing — P22 §7.2. Same
//! family contract as `info_ym.rs`: `&str` in, `Result` out, failures are
//! `Error::Parse`, no regex. Pinned to Hatari's `MFP_Show` format string
//! (mfp.c:3487): `"<label>: 0x%02x"` lines, in a fixed order. IPRA/B and
//! ISRA/B are captured for *reporting only* (P22 §4.2) — a restore stub
//! must never reinstall clear-only pending/in-service registers.

use super::parse_err;
use crate::error::Result;

/// The MFP 68901 register set as `MFP_Show` dumps it. `tadr`..`tddr` carry
/// the last-written *reload* value (not a live countdown) — exactly what a
/// restore stub must reinstall (P22 §4.1 row 6). `// verify live: TADR line
/// semantics on a running timer.`
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MfpRegs {
    pub gpip: u8,
    pub aer: u8,
    pub ddr: u8,
    pub iera: u8,
    pub ierb: u8,
    pub ipra: u8,
    pub iprb: u8,
    pub isra: u8,
    pub isrb: u8,
    pub imra: u8,
    pub imrb: u8,
    pub vr: u8,
    pub tacr: u8,
    pub tbcr: u8,
    pub tcdcr: u8,
    pub tadr: u8,
    pub tbdr: u8,
    pub tcdr: u8,
    pub tddr: u8,
}

/// Label prefix -> field setter, in `MFP_Show`'s emission order.
type FieldSetter = fn(&mut MfpRegs, u8);

/// Matching is prefix-based (`starts_with`) so the fixed-width label
/// padding doesn't matter; lines from "Synchronous Data" onward
/// (USART/IRQ/TT block) are simply never matched and fall through to
/// "skipped".
const FIELDS: &[(&str, FieldSetter)] = &[
    ("General Purpose Pins:", |r, v| r.gpip = v),
    ("Active Edge:", |r, v| r.aer = v),
    ("Data Direction:", |r, v| r.ddr = v),
    ("Interrupt A Enable:", |r, v| r.iera = v),
    ("Interrupt B Enable:", |r, v| r.ierb = v),
    ("Interrupt A Pending:", |r, v| r.ipra = v),
    ("Interrupt B Pending:", |r, v| r.iprb = v),
    ("Interrupt A In-Service:", |r, v| r.isra = v),
    ("Interrupt B In-Service:", |r, v| r.isrb = v),
    ("Interrupt A Mask:", |r, v| r.imra = v),
    ("Interrupt B Mask:", |r, v| r.imrb = v),
    ("Vector:", |r, v| r.vr = v),
    ("Timer A Control:", |r, v| r.tacr = v),
    ("Timer B Control:", |r, v| r.tbcr = v),
    ("Timer C/D Control:", |r, v| r.tcdcr = v),
    ("Timer A Data:", |r, v| r.tadr = v),
    ("Timer B Data:", |r, v| r.tbdr = v),
    ("Timer C Data:", |r, v| r.tcdr = v),
    ("Timer D Data:", |r, v| r.tddr = v),
];

/// `"<label>: 0x%02x"` -> `(label_with_colon, value)`. `None` on any parse
/// failure (caller skips the line — same tolerance as `parse_info_ym`).
fn parse_value_line(line: &str) -> Option<(&str, u8)> {
    let (label, val) = line.rsplit_once(' ')?;
    let hex = val.strip_prefix("0x")?;
    let v = u8::from_str_radix(hex, 16).ok()?;
    Some((label.trim_end(), v))
}

/// Parse Hatari's `info mfp` output into the 19-field `MfpRegs`. Every
/// label in `FIELDS` must appear or the result is `Error::Parse { context:
/// "info mfp" }`; unmatched lines (banners, the TT block, USART/IRQ tail)
/// are ignored.
pub fn parse_info_mfp(out: &str) -> Result<MfpRegs> {
    let mut regs = MfpRegs::default();
    let mut seen = [false; FIELDS.len()];
    for line in out.lines() {
        let Some((label, v)) = parse_value_line(line) else {
            continue;
        };
        if let Some(i) = FIELDS.iter().position(|(l, _)| *l == label) {
            (FIELDS[i].1)(&mut regs, v);
            seen[i] = true;
        }
    }
    if seen.iter().all(|&s| s) {
        Ok(regs)
    } else {
        Err(parse_err("info mfp", out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    /// Verbatim `MFP_Show` format (mfp.c:3487), main-MFP block only.
    fn probe_block() -> String {
        "General Purpose Pins:    0x12\n\
Active Edge:             0x34\n\
Data Direction:          0x56\n\
Interrupt A Enable:      0x78\n\
Interrupt B Enable:      0x9a\n\
Interrupt A Pending:     0x01\n\
Interrupt B Pending:     0x02\n\
Interrupt A In-Service:  0x03\n\
Interrupt B In-Service:  0x04\n\
Interrupt A Mask:        0xbc\n\
Interrupt B Mask:        0xde\n\
Vector:                  0x40\n\
Timer A Control:         0x08\n\
Timer B Control:         0x08\n\
Timer C/D Control:       0x66\n\
Timer A Data:            0xff\n\
Timer B Data:            0xee\n\
Timer C Data:            0xdd\n\
Timer D Data:            0xcc\n\
Synchronous Data:        0x00\n\
USART Control:           0x88\n\
Receiver Status:         0x00\n\
Transmitter Status:      0x00\n\
USART Data:              0x00\n\
IRQ signal:              0x00\n\
Input signal on Timer A: 0x00\n\
Input signal on Timer B: 0x00"
            .to_string()
    }

    #[test]
    fn parse_pinned_block() {
        let r = parse_info_mfp(&probe_block()).unwrap();
        assert_eq!(r.gpip, 0x12);
        assert_eq!(r.aer, 0x34);
        assert_eq!(r.ddr, 0x56);
        assert_eq!(r.iera, 0x78);
        assert_eq!(r.ierb, 0x9a);
        assert_eq!(r.ipra, 0x01);
        assert_eq!(r.iprb, 0x02);
        assert_eq!(r.isra, 0x03);
        assert_eq!(r.isrb, 0x04);
        assert_eq!(r.imra, 0xbc);
        assert_eq!(r.imrb, 0xde);
        assert_eq!(r.vr, 0x40);
        assert_eq!(r.tacr, 0x08);
        assert_eq!(r.tbcr, 0x08);
        assert_eq!(r.tcdcr, 0x66);
        assert_eq!(r.tadr, 0xff);
        assert_eq!(r.tbdr, 0xee);
        assert_eq!(r.tcdr, 0xdd);
        assert_eq!(r.tddr, 0xcc);
    }

    #[test]
    fn parse_ignores_tt_block() {
        let block = format!("{}\nTT-MFP:\n{}", probe_block(), probe_block());
        let r = parse_info_mfp(&block).unwrap();
        assert_eq!(r.gpip, 0x12);
    }

    #[test]
    fn parse_missing_label_errors() {
        let block: String = probe_block()
            .lines()
            .filter(|l| !l.starts_with("Vector:"))
            .collect::<Vec<_>>()
            .join("\n");
        let err = parse_info_mfp(&block).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "info mfp"));
    }

    #[test]
    fn parse_garbage_errors() {
        let err = parse_info_mfp("nonsense").unwrap_err();
        assert!(matches!(err, Error::Parse { .. }));
    }
}
