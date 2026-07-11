//! Text pretty-printer for `ym_state` (P17) — split from ym_wire.rs for the
//! 200-line budget, same pattern as machine_pretty.rs. Pure formatting over
//! the wire output; no session/emulator access.

use super::ym_wire::{ChannelOutput, EnvelopeOutput, YmStateOutput};

/// Format is contractual (golden test `pretty_golden_running`, P17.md §7.3).
pub(crate) fn pretty(o: &YmStateOutput) -> String {
    let mut lines = vec![
        "=== ym_state — YM2149 PSG snapshot ===".to_string(),
        format!("session : {}", o.session),
        format!(
            "mixer   : {}, tone {}, noise {}",
            o.mixer_raw,
            enabled_list(&o.channels, |c| c.tone_enabled),
            enabled_list(&o.channels, |c| c.noise_enabled),
        ),
    ];
    lines.extend(o.channels.iter().map(channel_line));
    lines.push(format!(
        "noise   : period ${:02X} ({:.2} Hz)",
        o.noise.period, o.noise.freq_hz
    ));
    lines.push(envelope_line(&o.envelope));
    lines.join("\n")
}

fn channel_line(c: &ChannelOutput) -> String {
    let note_part = c
        .note
        .as_ref()
        .map_or_else(|| "-".to_string(), |n| format!("{} {:+}c", n.name, n.cents));
    let vol_part = if c.use_envelope {
        "env".to_string()
    } else {
        c.volume.to_string()
    };
    format!(
        "{:<8}: {note_part} ({:.2} Hz), period ${:03X}, vol {vol_part}, tone {}, noise {}",
        c.name,
        c.freq_hz,
        c.period,
        onoff(c.tone_enabled),
        onoff(c.noise_enabled),
    )
}

fn envelope_line(e: &EnvelopeOutput) -> String {
    let use_str = if e.used_by.is_empty() {
        "unused".to_string()
    } else {
        format!("used by {}", e.used_by.join(" "))
    };
    let shape_kind = if e.continuous {
        "continuous"
    } else {
        "one-shot"
    };
    format!(
        "envelope: period ${:04X} ({:.2} Hz), shape {} {} ({shape_kind}), {use_str}",
        e.period, e.freq_hz, e.shape_raw, e.shape,
    )
}

/// Space-joined names of channels for which `pick` is true, or "none".
fn enabled_list(channels: &[ChannelOutput], pick: impl Fn(&ChannelOutput) -> bool) -> String {
    let names: Vec<&str> = channels
        .iter()
        .filter(|c| pick(c))
        .map(|c| c.name.as_str())
        .collect();
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(" ")
    }
}

fn onoff(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::ym::decode;
    use crate::tools::ym_wire::build_output;

    /// §2.2 probe 1, verbatim (the duplicated-tiny-fixture convention,
    /// P17.md §10).
    fn probe_regs() -> [u8; 16] {
        [
            0x59, 0x00, 0xEE, 0x0E, 0xCD, 0x02, 0x07, 0x2A, 0x0B, 0x0B, 0x0E, 0x00, 0x00, 0x00,
            0x25, 0x00,
        ]
    }

    #[test]
    fn pretty_golden_running() {
        let regs = probe_regs();
        let o = build_output("running", &decode(&regs), &regs);
        let expected = "=== ym_state — YM2149 PSG snapshot ===\n\
session : running\n\
mixer   : $2A, tone A C, noise B\n\
A       : F6 +9c (1404.49 Hz), period $059, vol 11, tone on, noise off\n\
B       : C1 +0c (32.71 Hz), period $EEE, vol 11, tone off, noise on\n\
C       : F3 -3c (174.34 Hz), period $2CD, vol 14, tone on, noise off\n\
noise   : period $07 (17857.14 Hz)\n\
envelope: period $0000 (7812.50 Hz), shape $00 \\_ (one-shot), unused";
        assert_eq!(pretty(&o), expected);
    }

    #[test]
    fn pretty_no_note_and_env() {
        let mut regs = probe_regs();
        regs[0] = 0x00;
        regs[1] = 0x00; // channel A period 0
        regs[8] = 0x10; // channel A: envelope, vol 0
        regs[7] = 0x3E; // mixer: tone A only
        let o = build_output("running", &decode(&regs), &regs);
        let text = pretty(&o);
        let a_line = text.lines().find(|l| l.starts_with("A       :")).unwrap();
        assert!(a_line.starts_with("A       : - (125000.00 Hz)"));
        assert!(a_line.contains("vol env"));
        let env_line = text.lines().find(|l| l.starts_with("envelope")).unwrap();
        assert!(env_line.ends_with("used by A"));
        let mixer_line = text.lines().find(|l| l.starts_with("mixer")).unwrap();
        assert_eq!(mixer_line, "mixer   : $3E, tone A, noise none");
    }
}
