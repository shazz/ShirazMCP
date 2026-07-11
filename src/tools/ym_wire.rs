//! Wire structs + pure build/pretty for the `ym_state` tool (P17) — the
//! machine_wire/machine_pretty pattern, small enough for one file. No
//! session/emulator access.

use schemars::JsonSchema;
use serde::Serialize;

use crate::analyze::ym::{ChannelState, YmState};

#[derive(Debug, Serialize, JsonSchema)]
pub struct YmStateOutput {
    /// "running" | "paused" — state at call time; never changed by this tool.
    pub session: String,
    /// Exactly 3, order A, B, C.
    pub channels: Vec<ChannelOutput>,
    pub noise: NoiseOutput,
    pub envelope: EnvelopeOutput,
    /// Mixer register R7 verbatim, "$2A".
    pub mixer_raw: String,
    /// The 14 sound registers R0..R13 as "$XX" (I/O ports R14/R15 omitted —
    /// they are floppy/parallel lines, not sound).
    pub registers_raw: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChannelOutput {
    /// "A" | "B" | "C".
    pub name: String,
    /// 12-bit tone period, 0..=4095.
    pub period: u16,
    /// 2_000_000 / (16 · max(1, period)), rounded to 2 decimals.
    pub freq_hz: f64,
    /// Nearest note; None when outside MIDI 0..=127 (e.g. period 0/1).
    pub note: Option<NoteOutput>,
    /// Level nibble 0..=15 (meaningful when use_envelope is false).
    pub volume: u8,
    /// Log-table amplitude 0.0..=1.0 for `volume`; null when use_envelope.
    pub amplitude: Option<f64>,
    /// Level-register bit 4: envelope drives this channel's loudness.
    pub use_envelope: bool,
    /// Mixer R7 bit ch == 0.
    pub tone_enabled: bool,
    /// Mixer R7 bit 3+ch == 0.
    pub noise_enabled: bool,
    /// (tone_enabled || noise_enabled) && (volume > 0 || use_envelope).
    pub audible: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NoteOutput {
    /// "A4", "F#2", "C-1" (NOTE_NAMES[midi%12] + octave midi/12-1).
    pub name: String,
    pub midi: u8,
    /// -50..=50 offset from the exact equal-tempered pitch.
    pub cents: i16,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NoiseOutput {
    /// 5-bit noise period, 0..=31.
    pub period: u8,
    /// 2_000_000 / (16 · max(1, period)), rounded to 2 decimals.
    pub freq_hz: f64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct EnvelopeOutput {
    /// 16-bit envelope period (R11 | R12<<8).
    pub period: u16,
    /// 2_000_000 / (256 · max(1, period)), rounded to 2 decimals.
    pub freq_hz: f64,
    /// Shape register value, "$00".
    pub shape_raw: String,
    /// Shape glyph: "\_", "/_", "\\", "\/", "\¯", "//", "/¯", "/\".
    pub shape: String,
    /// True for the repeating "buzzer" shapes ($08/$0A/$0C/$0E).
    pub continuous: bool,
    /// Channel names ("A".."C") whose level register has bit 4 set.
    pub used_by: Vec<String>,
}

const CHANNEL_NAMES: [&str; 3] = ["A", "B", "C"];

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// Pure: `analyze::ym::YmState` + the raw registers -> wire output.
/// `session` = `AnySession::state_name()` at call time.
pub(crate) fn build_output(session: &str, s: &YmState, regs: &[u8; 16]) -> YmStateOutput {
    let channels = CHANNEL_NAMES
        .iter()
        .zip(&s.channels)
        .map(|(name, ch)| channel_output(name, ch))
        .collect();
    let used_by = CHANNEL_NAMES
        .iter()
        .zip(&s.channels)
        .filter(|(_, ch)| ch.use_envelope)
        .map(|(name, _)| (*name).to_string())
        .collect();
    YmStateOutput {
        session: session.to_string(),
        channels,
        noise: NoiseOutput {
            period: s.noise_period,
            freq_hz: round2(s.noise_freq_hz),
        },
        envelope: EnvelopeOutput {
            period: s.envelope.period,
            freq_hz: round2(s.envelope.freq_hz),
            shape_raw: format!("${:02X}", s.envelope.shape),
            shape: s.envelope.ascii().to_string(),
            continuous: s.envelope.continuous(),
            used_by,
        },
        mixer_raw: format!("${:02X}", s.mixer),
        registers_raw: regs[0..14].iter().map(|b| format!("${b:02X}")).collect(),
    }
}

fn channel_output(name: &str, ch: &ChannelState) -> ChannelOutput {
    ChannelOutput {
        name: name.to_string(),
        period: ch.period,
        freq_hz: round2(ch.freq_hz),
        note: ch.note.map(|n| NoteOutput {
            name: n.name(),
            midi: n.midi,
            cents: n.cents,
        }),
        volume: ch.volume,
        amplitude: ch.amplitude().map(round4),
        use_envelope: ch.use_envelope,
        tone_enabled: ch.tone_enabled,
        noise_enabled: ch.noise_enabled,
        audible: ch.audible(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::ym::decode;

    /// §2.2 probe 1, verbatim (the duplicated-tiny-fixture convention,
    /// P17.md §10).
    fn probe_regs() -> [u8; 16] {
        [
            0x59, 0x00, 0xEE, 0x0E, 0xCD, 0x02, 0x07, 0x2A, 0x0B, 0x0B, 0x0E, 0x00, 0x00, 0x00,
            0x25, 0x00,
        ]
    }

    #[test]
    fn build_output_probe_pin() {
        let regs = probe_regs();
        let o = build_output("running", &decode(&regs), &regs);
        let names: Vec<&str> = o.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["A", "B", "C"]);
        assert_eq!(o.channels[0].freq_hz, 1404.49);
        assert_eq!(o.channels[1].freq_hz, 32.71);
        assert_eq!(o.channels[2].freq_hz, 174.34);
        assert_eq!(o.noise.freq_hz, 17857.14);
        assert_eq!(o.envelope.freq_hz, 7812.5);
        assert_eq!(o.mixer_raw, "$2A");
        assert_eq!(o.registers_raw.len(), 14);
        assert_eq!(o.registers_raw[0], "$59");
        assert_eq!(o.registers_raw[13], "$00");
        assert!(o.channels[1].audible);
    }

    #[test]
    fn json_serializes() {
        let regs = probe_regs();
        let o = build_output("running", &decode(&regs), &regs);
        let json = serde_json::to_string(&o).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["channels"][0]["note"]["name"], "F6");
        assert_eq!(v["channels"][0]["note"]["cents"], 9);
    }
}
