//! `info ym` (PSG register dump) parsing — P17. Same family contract as
//! values.rs: `&str` in, `Result` out, failures are `Error::Parse`, no
//! regex.

use super::parse_err;
use crate::error::Result;

/// Parse Hatari's `info ym` output: 16 lines `Reg $NN : $VV` (psg.c
/// PSG_Info, pinned live 2026-07-10). Non-matching lines are skipped (same
/// tolerance as `parse_memdump`); all 16 indices 0..=15 must appear or the
/// result is `Error::Parse { context: "info ym" }`.
pub fn parse_info_ym(out: &str) -> Result<[u8; 16]> {
    let mut regs: [Option<u8>; 16] = [None; 16];
    for line in out.lines() {
        if let Some((idx, val)) = parse_reg_line(line) {
            if let Some(slot) = regs.get_mut(usize::from(idx)) {
                *slot = Some(val);
            }
        }
    }
    let mut out_regs = [0u8; 16];
    for (i, slot) in regs.iter().enumerate() {
        out_regs[i] = slot.ok_or_else(|| parse_err("info ym", out))?;
    }
    Ok(out_regs)
}

/// `"Reg $NN : $VV"` -> `(NN, VV)`. `None` on any parse failure (caller
/// skips the line, same tolerance as `parse_memdump`'s non-matching lines).
fn parse_reg_line(line: &str) -> Option<(u8, u8)> {
    let rest = line.strip_prefix("Reg $")?;
    let (idx_str, val_str) = rest.split_once(" : $")?;
    let idx = u8::from_str_radix(idx_str.trim(), 16).ok()?;
    let val = u8::from_str_radix(val_str.trim(), 16).ok()?;
    Some((idx, val))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    /// §2.2 probe 1, verbatim.
    fn probe_regs() -> [u8; 16] {
        [
            0x59, 0x00, 0xEE, 0x0E, 0xCD, 0x02, 0x07, 0x2A, 0x0B, 0x0B, 0x0E, 0x00, 0x00, 0x00,
            0x25, 0x00,
        ]
    }

    fn probe_block() -> String {
        "Reg $00 : $59\n\
Reg $01 : $00\n\
Reg $02 : $EE\n\
Reg $03 : $0E\n\
Reg $04 : $CD\n\
Reg $05 : $02\n\
Reg $06 : $07\n\
Reg $07 : $2A\n\
Reg $08 : $0B\n\
Reg $09 : $0B\n\
Reg $0A : $0E\n\
Reg $0B : $00\n\
Reg $0C : $00\n\
Reg $0D : $00\n\
Reg $0E : $25\n\
Reg $0F : $00"
            .to_string()
    }

    #[test]
    fn parse_live_pin() {
        assert_eq!(parse_info_ym(&probe_block()).unwrap(), probe_regs());
    }

    #[test]
    fn parse_skips_noise_lines() {
        let block = format!("INFO : some banner\n{}\n", probe_block());
        assert_eq!(parse_info_ym(&block).unwrap(), probe_regs());
    }

    #[test]
    fn parse_missing_register() {
        let block: String = probe_block()
            .lines()
            .filter(|l| !l.starts_with("Reg $07"))
            .collect::<Vec<_>>()
            .join("\n");
        let err = parse_info_ym(&block).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "info ym"));
    }

    #[test]
    fn parse_garbage() {
        let err = parse_info_ym("nonsense").unwrap_err();
        assert!(matches!(err, Error::Parse { .. }));
    }
}
