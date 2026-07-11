//! Pure YM2149 register decode (P17) — the register→frequency/note/volume
//! math from STEIsNotDead's tools/audio/ym2149.py, reimplemented over a raw
//! `[u8; 16]`. No I/O, no emulator access, no synthesis (the LFSR/envelope
//! renderer is deliberately NOT ported — this decodes state, it does not
//! make sound).

/// Atari ST YM2149 master clock (ym2149.py CLOCK).
pub const YM_CLOCK_HZ: f64 = 2_000_000.0;

/// 16-level logarithmic volume table, normalised 0..=1 (~3 dB/step) —
/// ym2149.py's `_VOL`, verbatim.
pub const YM_VOLUME: [f64; 16] = [
    0.0, 0.0056, 0.0079, 0.0112, 0.0158, 0.0224, 0.0316, 0.0447, 0.0632, 0.0892, 0.1259, 0.1778,
    0.2512, 0.3548, 0.5012, 1.0,
];

/// (ascii, continuous) per envelope shape value 0..=15 (R13 & $0F). One-shot
/// shapes hold their final level; continuous shapes are the "buzzer" tones.
/// Matches ym2149.py's `_ENV_SHAPE` for 8/A/C/E; the remaining values follow
/// the AY/YM datasheet CONT/ATT/ALT/HOLD bits.
const ENV_SHAPES: [(&str, bool); 16] = [
    ("\\_", false), // 0: decay, hold 0
    ("\\_", false), // 1: decay, hold 0
    ("\\_", false), // 2: decay, hold 0
    ("\\_", false), // 3: decay, hold 0
    ("/_", false),  // 4: attack, drop, hold 0
    ("/_", false),  // 5: attack, drop, hold 0
    ("/_", false),  // 6: attack, drop, hold 0
    ("/_", false),  // 7: attack, drop, hold 0
    ("\\\\", true), // 8: repeating saw down
    ("\\_", false), // 9: decay, hold 0
    ("\\/", true),  // A: repeating triangle
    ("\\¯", false), // B: decay, hold 1
    ("//", true),   // C: repeating saw up
    ("/¯", false),  // D: attack, hold 1
    ("/\\", true),  // E: repeating inverted triangle
    ("/_", false),  // F: attack, drop, hold 0
];

/// Module-private: `Note::name`'s octave/pitch-class table.
const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Nearest equal-tempered note to a frequency (A4 = 440 Hz = MIDI 69).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    pub midi: u8,
    pub cents: i16,
}

impl Note {
    /// "C-1".."G9" style: `NOTE_NAMES[midi % 12] + (midi/12 - 1)`.
    pub fn name(&self) -> String {
        let octave = i32::from(self.midi) / 12 - 1;
        format!("{}{octave}", NOTE_NAMES[usize::from(self.midi % 12)])
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelState {
    pub period: u16,
    pub freq_hz: f64,
    pub note: Option<Note>,
    pub volume: u8,
    pub use_envelope: bool,
    pub tone_enabled: bool,
    pub noise_enabled: bool,
}

impl ChannelState {
    /// `(tone_enabled || noise_enabled) && (volume > 0 || use_envelope)`.
    pub fn audible(&self) -> bool {
        (self.tone_enabled || self.noise_enabled) && (self.volume > 0 || self.use_envelope)
    }

    /// `YM_VOLUME[volume]`; `None` when `use_envelope` (the envelope, not
    /// the level nibble, controls loudness then).
    pub fn amplitude(&self) -> Option<f64> {
        (!self.use_envelope).then(|| YM_VOLUME[usize::from(self.volume)])
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvelopeState {
    pub period: u16,
    /// `YM_CLOCK_HZ / (256 · max(1, period))` — the envelope cycle rate
    /// (ym2149.py `freq_to_ep` inverse); triangle shapes sound one octave
    /// lower (32-step cycle).
    pub freq_hz: f64,
    pub shape: u8,
}

impl EnvelopeState {
    pub fn ascii(&self) -> &'static str {
        ENV_SHAPES[usize::from(self.shape)].0
    }

    pub fn continuous(&self) -> bool {
        ENV_SHAPES[usize::from(self.shape)].1
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct YmState {
    /// A, B, C.
    pub channels: [ChannelState; 3],
    pub noise_period: u8,
    pub noise_freq_hz: f64,
    pub envelope: EnvelopeState,
    pub mixer: u8,
}

/// `YM_CLOCK_HZ / (16 · max(1, period))` — ym2149.py `tp_to_freq`.
pub fn tone_freq_hz(period: u16) -> f64 {
    YM_CLOCK_HZ / (16.0 * f64::from(period.max(1)))
}

/// Nearest note: `midi_f = 69 + 12·log2(freq/440)`; `nearest =
/// midi_f.round()`; `None` if `nearest` is outside `0.0..=127.0`; `cents =
/// ((midi_f − nearest)·100).round()`.
pub fn note_of(freq_hz: f64) -> Option<Note> {
    let midi_f = 69.0 + 12.0 * (freq_hz / 440.0).log2();
    let nearest = midi_f.round();
    if !(0.0..=127.0).contains(&nearest) {
        return None;
    }
    let cents = ((midi_f - nearest) * 100.0).round() as i16;
    // WHY: `nearest` was just checked to lie in 0.0..=127.0, so the cast is
    // lossless (no fractional part survives `.round()`).
    Some(Note {
        midi: nearest as u8,
        cents,
    })
}

/// Decode the 16 raw PSG registers. Masks defensively (Hatari masks on
/// write — psg.c — but this fn accepts arbitrary bytes): tone coarse &
/// $0F, noise & $1F, level & $1F, shape & $0F.
pub fn decode(regs: &[u8; 16]) -> YmState {
    let mixer = regs[7];
    let noise_period = regs[6] & 0x1F;
    let env_period = u16::from(regs[11]) | (u16::from(regs[12]) << 8);
    YmState {
        channels: std::array::from_fn(|ch| channel(regs, mixer, ch)),
        noise_period,
        noise_freq_hz: tone_freq_hz(u16::from(noise_period)),
        envelope: EnvelopeState {
            period: env_period,
            freq_hz: YM_CLOCK_HZ / (256.0 * f64::from(env_period.max(1))),
            shape: regs[13] & 0x0F,
        },
        mixer,
    }
}

/// `period = regs[2*ch] | ((regs[2*ch+1] & $0F) << 8)`; `level =
/// regs[8+ch] & $1F`; mixer bit `ch` gates tone, bit `3+ch` gates noise
/// (both active-low).
fn channel(regs: &[u8; 16], mixer: u8, ch: usize) -> ChannelState {
    let coarse = regs[2 * ch + 1] & 0x0F;
    let period = u16::from(regs[2 * ch]) | (u16::from(coarse) << 8);
    let level = regs[8 + ch] & 0x1F;
    let freq_hz = tone_freq_hz(period);
    ChannelState {
        period,
        freq_hz,
        note: note_of(freq_hz),
        volume: level & 0x0F,
        use_envelope: level & 0x10 != 0,
        tone_enabled: (mixer >> ch) & 1 == 0,
        noise_enabled: (mixer >> (3 + ch)) & 1 == 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §2.2 probe 1, verbatim.
    fn probe_regs() -> [u8; 16] {
        [
            0x59, 0x00, 0xEE, 0x0E, 0xCD, 0x02, 0x07, 0x2A, 0x0B, 0x0B, 0x0E, 0x00, 0x00, 0x00,
            0x25, 0x00,
        ]
    }

    #[test]
    fn decode_live_probe_pin() {
        let s = decode(&probe_regs());
        let a = &s.channels[0];
        assert_eq!(a.period, 89);
        assert_eq!(a.note, Some(Note { midi: 89, cents: 9 }));
        assert_eq!(a.note.unwrap().name(), "F6");
        assert_eq!(a.volume, 11);
        assert!(!a.use_envelope);
        assert!(a.tone_enabled);
        assert!(!a.noise_enabled);
        assert!(a.audible());

        let b = &s.channels[1];
        assert_eq!(b.period, 3822);
        assert_eq!(b.note.unwrap().midi, 24);
        assert_eq!(b.note.unwrap().name(), "C1");
        assert_eq!(b.note.unwrap().cents, 0);
        assert_eq!(b.volume, 11);
        assert!(!b.tone_enabled);
        assert!(b.noise_enabled);
        assert!(b.audible());

        let c = &s.channels[2];
        assert_eq!(c.period, 717);
        assert_eq!(c.note.unwrap().midi, 53);
        assert_eq!(c.note.unwrap().name(), "F3");
        assert_eq!(c.note.unwrap().cents, -3);
        assert_eq!(c.volume, 14);

        assert_eq!(s.noise_period, 7);
        assert_eq!(s.envelope.period, 0);
        assert_eq!(s.envelope.shape, 0);
        assert!(!s.envelope.continuous());
        assert_eq!(s.envelope.ascii(), "\\_");
        assert_eq!(s.mixer, 0x2A);
    }

    #[test]
    fn freq_pins() {
        let s = decode(&probe_regs());
        assert!((1404.49..1404.50).contains(&s.channels[0].freq_hz));
        assert!((17857.14..17857.15).contains(&s.noise_freq_hz));
        assert_eq!(s.envelope.freq_hz, 7812.5);
    }

    #[test]
    fn period_zero_clamps() {
        let s = decode(&[0u8; 16]);
        assert_eq!(s.channels[0].freq_hz, 125_000.0);
        assert_eq!(s.channels[0].note, None);
    }

    #[test]
    fn note_of_a440() {
        let n = note_of(tone_freq_hz(284)).unwrap();
        assert_eq!(n, Note { midi: 69, cents: 1 });
        assert_eq!(n.name(), "A4");
    }

    #[test]
    fn note_name_edges() {
        assert_eq!(Note { midi: 0, cents: 0 }.name(), "C-1");
        assert_eq!(
            Note {
                midi: 127,
                cents: 0
            }
            .name(),
            "G9"
        );
    }

    #[test]
    fn volume_table_pins() {
        assert_eq!(YM_VOLUME[0], 0.0);
        assert_eq!(YM_VOLUME[11], 0.1778);
        assert_eq!(YM_VOLUME[14], 0.5012);
        assert_eq!(YM_VOLUME[15], 1.0);
        let s = decode(&probe_regs());
        assert_eq!(s.channels[0].amplitude(), Some(0.1778));
    }

    #[test]
    fn use_envelope_amplitude_none() {
        let mut regs = probe_regs();
        regs[8] = 0x1F; // level reg: vol 15 + envelope bit set
        let s = decode(&regs);
        assert_eq!(s.channels[0].volume, 15);
        assert!(s.channels[0].use_envelope);
        assert_eq!(s.channels[0].amplitude(), None);
        // Mixer (unchanged from the probe) still gates A's tone on, so the
        // channel is audible purely via `use_envelope` even though the
        // level-nibble volume is moot once the envelope drives loudness.
        assert!(s.channels[0].audible());
    }

    #[test]
    fn mixer_all_off_inaudible() {
        let mut regs = probe_regs();
        regs[7] = 0x3F;
        let s = decode(&regs);
        for ch in &s.channels {
            assert!(!ch.tone_enabled);
            assert!(!ch.noise_enabled);
            assert!(!ch.audible());
        }
    }

    #[test]
    fn envelope_shape_table() {
        let cases: [(u8, &str, bool); 8] = [
            (0x00, "\\_", false),
            (0x08, "\\\\", true),
            (0x0A, "\\/", true),
            (0x0B, "\\¯", false),
            (0x0C, "//", true),
            (0x0D, "/¯", false),
            (0x0E, "/\\", true),
            (0x0F, "/_", false),
        ];
        for (shape, ascii, continuous) in cases {
            let mut regs = probe_regs();
            regs[13] = shape;
            let s = decode(&regs);
            assert_eq!(s.envelope.ascii(), ascii, "shape {shape:#04x}");
            assert_eq!(s.envelope.continuous(), continuous, "shape {shape:#04x}");
        }
    }

    #[test]
    fn defensive_masks() {
        let mut regs = probe_regs();
        regs[1] = 0xFE;
        regs[6] = 0xFF;
        regs[8] = 0xFF;
        regs[13] = 0xFF;
        let s = decode(&regs);
        assert_eq!(s.channels[0].period, 0x0E59);
        assert_eq!(s.noise_period, 31);
        assert_eq!(s.channels[0].volume, 15);
        assert!(s.channels[0].use_envelope);
        assert_eq!(s.envelope.shape, 15);
    }
}
