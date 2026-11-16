//! `cpureg` parsing — split out of `parse/mod.rs` purely to keep that file
//! under the line ceiling. See P1.md §6.2 for the exact fixture shape this
//! is pinned to.

use super::parse_err;
use crate::debug::{Registers, SrFlags};
use crate::error::Result;
use crate::types::Addr;

pub fn parse_cpureg(out: &str) -> Result<Registers> {
    let mut regs = RegAcc::default();
    let lines: Vec<&str> = out.lines().collect();
    for line in &lines {
        regs.scan_registers(line);
        regs.scan_flags(line);
    }
    regs.pc = find_pc(&lines);
    regs.build().ok_or_else(|| parse_err("cpureg", out))
}

#[derive(Default)]
struct RegAcc {
    d: [Option<u32>; 8],
    a: [Option<u32>; 8],
    usp: Option<u32>,
    isp: Option<u32>,
    sr: Option<SrFlags>,
    pc: Option<Addr>,
}

impl RegAcc {
    fn scan_registers(&mut self, line: &str) {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        for pair in tokens.windows(2) {
            let (name, value) = (pair[0], pair[1]);
            let Some(v) = hex8(value) else { continue };
            match reg_slot(name) {
                Some(Slot::D(i)) => self.d[i] = Some(v),
                Some(Slot::A(i)) => self.a[i] = Some(v),
                Some(Slot::Usp) => self.usp = Some(v),
                Some(Slot::Isp) => self.isp = Some(v),
                None => {}
            }
        }
    }

    fn scan_flags(&mut self, line: &str) {
        // Hatari 2.4 prints "IMASK=6"; 2.6 prints "SR=2400 … IM=4".
        if !line.contains("IMASK=") && !line.contains(" IM=") {
            return;
        }
        let mut f = SrFlags {
            trace: 0,
            supervisor: false,
            x: false,
            n: false,
            z: false,
            v: false,
            c: false,
            imask: 0,
        };
        for tok in line.split_whitespace() {
            let Some((key, val)) = tok.split_once('=') else {
                continue;
            };
            match key {
                "T" => f.trace = val.parse().unwrap_or(0),
                "S" => f.supervisor = val == "1",
                "X" => f.x = val == "1",
                "N" => f.n = val == "1",
                "Z" => f.z = val == "1",
                "V" => f.v = val == "1",
                "C" => f.c = val == "1",
                "IMASK" | "IM" => f.imask = val.parse().unwrap_or(0),
                _ => {}
            }
        }
        self.sr = Some(f);
    }

    fn build(self) -> Option<Registers> {
        let d = collect8(self.d)?;
        let a = collect8(self.a)?;
        Some(Registers {
            d,
            a,
            usp: self.usp?,
            isp: self.isp?,
            sr: self.sr?,
            pc: self.pc?,
        })
    }
}

fn collect8(slots: [Option<u32>; 8]) -> Option<[u32; 8]> {
    let mut out = [0u32; 8];
    for (i, s) in slots.into_iter().enumerate() {
        out[i] = s?;
    }
    Some(out)
}

enum Slot {
    D(usize),
    A(usize),
    Usp,
    Isp,
}

fn reg_slot(name: &str) -> Option<Slot> {
    if name == "USP" {
        return Some(Slot::Usp);
    }
    if name == "ISP" {
        return Some(Slot::Isp);
    }
    let (prefix, rest) = name.split_at_checked(1)?;
    let idx: usize = rest.parse().ok()?;
    if idx >= 8 {
        return None;
    }
    match prefix {
        "D" => Some(Slot::D(idx)),
        "A" => Some(Slot::A(idx)),
        _ => None,
    }
}

fn hex8(s: &str) -> Option<u32> {
    if s.len() != 8 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(s, 16).ok()
}

/// PC = the first token of the line immediately before the "Next PC:" line.
fn find_pc(lines: &[&str]) -> Option<Addr> {
    let idx = lines.iter().position(|l| l.starts_with("Next PC:"))?;
    let prev = lines.get(idx.checked_sub(1)?)?;
    let first = prev.split_whitespace().next()?;
    u32::from_str_radix(first, 16).ok().map(Addr::new)
}

#[cfg(test)]
mod tests {
    use super::super::fixture_block;
    use super::*;
    use crate::error::Error;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/hatari_session.log");

    #[test]
    fn cpureg_parses_all_registers() {
        let block = fixture_block(FIXTURE, "cpureg");
        let regs = parse_cpureg(&block).unwrap();
        assert_eq!(regs.d[0], 0x03FF_0052);
        assert_eq!(regs.d[1], 0x0003_5CF0);
        assert_eq!(regs.d[7], 0x0000_005D);
        assert_eq!(regs.a[0], 0x00FF_8A00);
        assert_eq!(regs.a[7], 0x000F_7FE2);
        assert_eq!(regs.usp, 0x000F_7FEE);
        assert_eq!(regs.isp, 0x000F_7FE2);
    }

    #[test]
    fn cpureg_parses_flags_and_pc() {
        let block = fixture_block(FIXTURE, "cpureg");
        let regs = parse_cpureg(&block).unwrap();
        assert!(regs.sr.supervisor);
        assert!(!regs.sr.z);
        assert!(regs.sr.n);
        assert_eq!(regs.sr.imask, 6);
        assert_eq!(regs.pc, Addr::new(0xdc74));
    }

    /// Hatari 2.6.1 shape (captured live): "SR=…" prefix and "IM=" key.
    #[test]
    fn cpureg_parses_hatari_26_flags_line() {
        let out = "  D0 FFFF00E4   D1 00000F25   D2 02E00C25   D3 0000FFFF \n\
  D4 00000050   D5 00100000   D6 00000017   D7 0000032F \n\
  A0 00000FEC   A1 00000A8C   A2 000037C2   A3 00E0385C \n\
  A4 00002B16   A5 00000000   A6 FFFF8606   A7 00003794 \n\
USP  00000000 ISP  00003794 \n\
SR=2400 T=00 S=1 M=0 X=0 N=0 Z=0 V=0 C=0 IM=4 STP=0\n\
Prefetch 52b9 (ADD) 0000 (OR) Chip latch 00000000\n\
00e00802 52b9 0000 0466           addq.l #$01,$00000466 [000000c3]\n\
Next PC: 00e00808";
        let regs = parse_cpureg(out).unwrap();
        assert!(regs.sr.supervisor);
        assert_eq!(regs.sr.imask, 4);
        assert_eq!(regs.isp, 0x3794);
        assert_eq!(regs.pc, Addr::new(0xe00802));
    }

    #[test]
    fn cpureg_garbage_is_parse_error() {
        let err = parse_cpureg("nonsense").unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "cpureg"));
    }
}
