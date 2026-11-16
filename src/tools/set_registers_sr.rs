//! SR bits per CPU + the `notes` `set_registers` attaches to an SR write
//! (ADR-47 addendum). Pure; split from `set_registers_wire.rs`.
//!
//! The 68000/68010 SR has no T0 (bit 14) or M (bit 12): Hatari drops them on
//! write, so `$B300` reads back `$A300` — expected masking, not a failed
//! write. The 68020+ implements both. Readback is compared under this mask
//! and the dropped bits are reported as a note, so `ok` stays truthful.

use crate::session::HatariConfig;

/// Implemented SR bits on a 68000/68010: T1, S, IPL2-0, XNZVC.
pub(crate) const SR_MASK_68000: u16 = 0xA71F;
/// 68020+: adds T0 (bit 14) and M (bit 12).
pub(crate) const SR_MASK_68020: u16 = 0xF71F;
/// Trace bits (T1|T0).
const SR_TRACE: u16 = 0xC000;

/// The SR mask for a session. `cpu_level` wins; otherwise the machine's
/// Hatari default (TT and Falcon boot a 68030, the ST family a 68000).
pub(crate) fn sr_mask(machine: &str, cpu_level: Option<u8>) -> u16 {
    let level = cpu_level.unwrap_or_else(|| match machine.to_ascii_lowercase().as_str() {
        "tt" | "falcon" => 3,
        _ => 0,
    });
    if level >= 2 {
        SR_MASK_68020
    } else {
        SR_MASK_68000
    }
}

pub(crate) fn sr_mask_for(config: &HatariConfig) -> u16 {
    sr_mask(&config.machine, config.options.cpu_level)
}

/// Human notes for a requested SR: bits this CPU drops, and a set trace
/// bit (the CPU takes a Trace exception — vector 9, `$24` — after the first
/// instruction on resume, so execution leaves the requested PC; TOS's
/// handler does not return to it).
pub(crate) fn sr_notes(want: Option<u16>, mask: u16) -> Vec<String> {
    let Some(want) = want else {
        return Vec::new();
    };
    let mut notes = Vec::new();
    let dropped = want & !mask;
    if dropped != 0 {
        notes.push(format!(
            "sr: bits ${dropped:04X} not implemented on this CPU (mask ${mask:04X}) — \
             dropped, ${want:04X} reads back ${:04X}",
            want & mask
        ));
    }
    if want & mask & SR_TRACE != 0 {
        notes.push(
            "sr: trace bit set — on resume the CPU takes a Trace exception \
             (vector $24) after the first instruction, so execution leaves pc; \
             clear bits 15/14 unless you want single-step tracing"
                .to_string(),
        );
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_follows_machine_default_and_cpu_level() {
        assert_eq!(sr_mask("ste", None), SR_MASK_68000);
        assert_eq!(sr_mask("ST", None), SR_MASK_68000);
        assert_eq!(sr_mask("falcon", None), SR_MASK_68020);
        assert_eq!(sr_mask("tt", None), SR_MASK_68020);
        assert_eq!(sr_mask("ste", Some(3)), SR_MASK_68020);
        assert_eq!(sr_mask("falcon", Some(0)), SR_MASK_68000);
        assert_eq!(sr_mask("st", Some(1)), SR_MASK_68000);
    }

    #[test]
    fn notes_report_dropped_bit12_on_68000() {
        let n = sr_notes(Some(0x3300), SR_MASK_68000);
        assert_eq!(n.len(), 1);
        assert!(n[0].contains("$1000") && n[0].contains("$2300"), "{n:?}");
        assert!(sr_notes(Some(0x3300), SR_MASK_68020).is_empty());
    }

    #[test]
    fn notes_warn_on_trace_bit() {
        let n = sr_notes(Some(0xB300), SR_MASK_68000);
        assert_eq!(n.len(), 2, "{n:?}");
        assert!(n[1].contains("Trace exception"));
        // T0 alone is unimplemented on a 68000: dropped, so no trace warning.
        let n = sr_notes(Some(0x6300), SR_MASK_68000);
        assert_eq!(n.len(), 1);
        assert!(!n[0].contains("Trace"));
    }

    #[test]
    fn notes_empty_for_plain_sr_or_no_sr() {
        assert!(sr_notes(Some(0x2700), SR_MASK_68000).is_empty());
        assert!(sr_notes(None, SR_MASK_68000).is_empty());
    }
}
