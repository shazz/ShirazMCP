//! Wire structs + pure plan/build for the P20 rip_sample tool — split from
//! rip_sample.rs for the 200-line budget, same pattern as view_wire.rs. No
//! session/emulator access.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::analyze::dmasnd::{self, BufferCheck, DmaSndState};
use crate::types::Addr;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RipSampleArgs {
    /// Absolute output path ending in ".wav". Default:
    /// "<work_dir>/sample-NNNN.wav".
    pub out_path: Option<String>,
    /// Also rip when the DMA play bit is off but the frame registers still
    /// delimit a valid buffer (the last sample played). Default false.
    pub force: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RipSampleOutput {
    /// "running" | "paused" — state at call time (restored on exit).
    pub session: String,
    /// True iff a running session was momentarily frozen for coherence.
    pub momentary_freeze: bool,
    /// "playing" (play bit set, buffer ripped) | "stopped" (valid buffer,
    /// play off — ripped only with force=true) | "none" (nothing to rip).
    pub status: String,
    /// Set iff no WAV was written — the machine-derived explanation.
    pub reason: Option<String>,
    /// $FF8901 bit 0. May read true for a just-finished one-shot (the
    /// hardware latch is not cleared in the register file) — see P20 §2.2.
    pub play: bool,
    #[serde(rename = "loop")]
    pub loop_on: bool,
    pub mono: bool,
    /// 1 (mono) or 2 (stereo, LR byte-interleaved in ST RAM).
    pub channels: u16,
    /// 6258 | 12517 | 25033 | 50066.
    pub rate_hz: u32,
    /// "$030000" style (Addr Display).
    pub frame_start: String,
    pub frame_end: String,
    /// Informational only: last value the program itself latched, not the
    /// live DMA position (P20 §2.2).
    pub frame_counter: String,
    /// frame_end - frame_start (bytes); 0 when status == "none".
    pub len_bytes: usize,
    /// (len_bytes / channels) / rate_hz; 0.0 when status == "none".
    pub duration_s: f64,
    /// On-disk WAV — a path handle, never bytes (ADR-5). None iff no rip.
    pub wav_path: Option<String>,
    /// Max |sample| of the written WAV (read back via wav::read_mono).
    pub peak: Option<f32>,
    /// peak < audio_wire::SILENT_PEAK — "is it actually silence?".
    pub silent: Option<bool>,
    pub control_raw: String, // "$03"
    pub mode_raw: String,    // "$81"
}

/// Machines with the STE DMA sound engine (Config_IsMachineSTE/TT in
/// Hatari terms). Falcon is excluded deliberately — crossbar/16-bit
/// differences, P20 §3.6.
pub(crate) fn has_dma_sound(machine: &str) -> bool {
    matches!(
        machine.to_ascii_lowercase().as_str(),
        "ste" | "megaste" | "tt"
    )
}

/// The extraction decision. `extract` = (frame_start, len) iff bytes
/// should be acquired.
pub(crate) struct RipPlan {
    pub(crate) status: &'static str, // "playing" | "stopped" | "none"
    pub(crate) reason: Option<String>,
    pub(crate) extract: Option<(u32, usize)>,
}

fn none_plan(reason: String) -> RipPlan {
    RipPlan {
        status: "none",
        reason: Some(reason),
        extract: None,
    }
}

/// Pure: machine gate → check_buffer → play/force (P20 §9.3 exact mapping).
pub(crate) fn plan(machine: &str, s: &DmaSndState, ram_len: usize, force: bool) -> RipPlan {
    if !has_dma_sound(machine) {
        return none_plan(format!(
            "machine '{machine}' has no STE DMA sound — start with machine=\"ste\""
        ));
    }
    match dmasnd::check_buffer(s, ram_len) {
        BufferCheck::Empty => none_plan(format!(
            "frame start == frame end ({}) — no sample buffer programmed",
            Addr::new(s.frame_start)
        )),
        BufferCheck::Inverted => none_plan(format!(
            "frame end {} is below frame start {} — registers mid-update or garbage",
            Addr::new(s.frame_end),
            Addr::new(s.frame_start)
        )),
        BufferCheck::OutOfRam { end, ram } => none_plan(format!(
            "frame end {} exceeds ST RAM ({ram} bytes)",
            Addr::new(end)
        )),
        BufferCheck::Ok { len } => plan_ok(s, len, force),
    }
}

fn plan_ok(s: &DmaSndState, len: usize, force: bool) -> RipPlan {
    if s.play {
        return RipPlan {
            status: "playing",
            reason: None,
            extract: Some((s.frame_start, len)),
        };
    }
    if force {
        return RipPlan {
            status: "stopped",
            reason: None,
            extract: Some((s.frame_start, len)),
        };
    }
    RipPlan {
        status: "stopped",
        reason: Some(
            "DMA replay is off — the registers still delimit the last sample; pass \
             force=true to rip it"
                .to_string(),
        ),
        extract: None,
    }
}

/// Pure assembly: state + plan + optional written-wav facts → output.
pub(crate) struct WavFacts {
    pub(crate) path: String,
    pub(crate) peak: f32,
    pub(crate) silent: bool,
}

/// `duration_s = (len / channels) as f64 / rate as f64` (0.0 when no len);
/// hex fields via Addr Display / format!("${:02X}"). `len_bytes`/
/// `duration_s` are set from `p.extract` only when a WAV was actually
/// written (`wav.is_some()`) — a stopped-without-force result reports the
/// registers but zero artifact numbers.
pub(crate) fn build_output(
    session: &str,
    momentary: bool,
    s: &DmaSndState,
    p: &RipPlan,
    wav: Option<WavFacts>,
) -> RipSampleOutput {
    let (len_bytes, duration_s) = match (wav.is_some(), p.extract) {
        (true, Some((_, len))) => (len, duration_of(len, s)),
        _ => (0, 0.0),
    };
    RipSampleOutput {
        session: session.to_string(),
        momentary_freeze: momentary,
        status: p.status.to_string(),
        reason: p.reason.clone(),
        play: s.play,
        loop_on: s.loop_on,
        mono: s.mono,
        channels: s.channels(),
        rate_hz: s.rate_hz,
        frame_start: Addr::new(s.frame_start).to_string(),
        frame_end: Addr::new(s.frame_end).to_string(),
        frame_counter: Addr::new(s.frame_counter).to_string(),
        len_bytes,
        duration_s,
        wav_path: wav.as_ref().map(|w| w.path.clone()),
        peak: wav.as_ref().map(|w| w.peak),
        silent: wav.as_ref().map(|w| w.silent),
        control_raw: format!("${:02X}", s.control),
        mode_raw: format!("${:02X}", s.mode),
    }
}

fn duration_of(len_bytes: usize, s: &DmaSndState) -> f64 {
    (len_bytes / usize::from(s.channels())) as f64 / f64::from(s.rate_hz)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §11.1 pinned state: play=true, loop, mono, 12517, start $030000,
    /// end $03A000.
    fn pinned_state() -> DmaSndState {
        dmasnd::decode(&{
            let mut b = [0u8; dmasnd::DMASND_BLOCK_LEN];
            b[0x01] = 0x03;
            b[0x03] = 0x03;
            b[0x07] = 0x01;
            b[0x0F] = 0x03;
            b[0x11] = 0xA0;
            b[0x21] = 0x81;
            b
        })
        .unwrap()
    }

    #[test]
    fn args_defaults() {
        let a: RipSampleArgs = serde_json::from_str("{}").unwrap();
        assert_eq!(a.out_path, None);
        assert_eq!(a.force, None);
    }

    #[test]
    fn has_dma_sound_gate() {
        for m in ["ste", "STE", "megaste", "tt"] {
            assert!(has_dma_sound(m), "{m}");
        }
        for m in ["st", "megast", "falcon"] {
            assert!(!has_dma_sound(m), "{m}");
        }
    }

    #[test]
    fn plan_playing() {
        let s = pinned_state();
        let p = plan("ste", &s, 4 * 1024 * 1024, false);
        assert_eq!(p.status, "playing");
        assert!(p.reason.is_none());
        assert_eq!(p.extract, Some((0x030000, 0xA000)));
    }

    #[test]
    fn plan_stopped_no_force() {
        let mut s = pinned_state();
        s.play = false;
        let p = plan("ste", &s, 4 * 1024 * 1024, false);
        assert_eq!(p.status, "stopped");
        assert!(p.reason.as_ref().unwrap().contains("force=true"));
        assert!(p.extract.is_none());
    }

    #[test]
    fn plan_stopped_force() {
        let mut s = pinned_state();
        s.play = false;
        let p = plan("ste", &s, 4 * 1024 * 1024, true);
        assert_eq!(p.status, "stopped");
        assert!(p.reason.is_none());
        assert!(p.extract.is_some());
    }

    #[test]
    fn plan_none_machine() {
        let s = pinned_state();
        let p = plan("st", &s, 4 * 1024 * 1024, false);
        assert_eq!(p.status, "none");
        assert!(p.reason.as_ref().unwrap().contains("STE DMA"));
        assert!(p.extract.is_none());
    }

    #[test]
    fn plan_none_empty_inverted_oob() {
        let mut empty = pinned_state();
        empty.frame_end = empty.frame_start;
        let p = plan("ste", &empty, 4 * 1024 * 1024, false);
        assert_eq!(p.status, "none");
        assert!(p
            .reason
            .as_ref()
            .unwrap()
            .contains(&Addr::new(empty.frame_start).to_string()));
        assert!(p.extract.is_none());

        let mut inverted = pinned_state();
        inverted.frame_end = inverted.frame_start - 0x1000;
        let p = plan("ste", &inverted, 4 * 1024 * 1024, false);
        assert_eq!(p.status, "none");
        assert!(p
            .reason
            .as_ref()
            .unwrap()
            .contains(&Addr::new(inverted.frame_end).to_string()));
        assert!(p.extract.is_none());

        let oob = pinned_state();
        let p = plan("ste", &oob, 0x1000, false);
        assert_eq!(p.status, "none");
        assert!(p
            .reason
            .as_ref()
            .unwrap()
            .contains(&Addr::new(oob.frame_end).to_string()));
        assert!(p.extract.is_none());
    }

    #[test]
    fn build_output_playing() {
        let s = pinned_state();
        let p = plan("ste", &s, 4 * 1024 * 1024, false);
        let wav = WavFacts {
            path: "/tmp/sample-0001.wav".to_string(),
            peak: 0.5,
            silent: false,
        };
        let out = build_output("running", true, &s, &p, Some(wav));
        assert_eq!(out.len_bytes, 0xA000);
        assert!((out.duration_s - 40_960.0 / 12517.0).abs() < 1e-9);
        assert_eq!(out.frame_start, "$030000");
        assert_eq!(out.frame_end, "$03A000");
        assert_eq!(out.control_raw, "$03");
        assert_eq!(out.mode_raw, "$81");
        assert_eq!(out.wav_path, Some("/tmp/sample-0001.wav".to_string()));
        assert_eq!(out.channels, 1);
        assert_eq!(out.rate_hz, 12517);
    }

    #[test]
    fn build_output_stopped_no_wav() {
        let mut s = pinned_state();
        s.play = false;
        let p = plan("ste", &s, 4 * 1024 * 1024, false);
        let out = build_output("running", true, &s, &p, None);
        assert_eq!(out.len_bytes, 0);
        assert_eq!(out.duration_s, 0.0);
        assert_eq!(out.wav_path, None);
        assert_eq!(out.peak, None);
        assert_eq!(out.silent, None);
        assert_eq!(out.frame_start, "$030000");
        assert_eq!(out.frame_end, "$03A000");
    }

    #[test]
    fn json_serializes_loop_rename() {
        let s = pinned_state();
        let p = plan("ste", &s, 4 * 1024 * 1024, false);
        let out = build_output("running", true, &s, &p, None);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"loop\":true"));
        assert!(!json.contains("loop_on"));
        assert!(json.contains("\"status\":\"playing\""));
    }

    /// §11.5: duration math cross-check — stereo mode, ÷channels-before-÷rate.
    #[test]
    fn build_output_stereo_duration_order() {
        let mut s = pinned_state();
        s.mono = false;
        let p = RipPlan {
            status: "playing",
            reason: None,
            extract: Some((s.frame_start, 0xA000)),
        };
        let wav = WavFacts {
            path: "/tmp/x.wav".to_string(),
            peak: 0.1,
            silent: false,
        };
        let out = build_output("running", true, &s, &p, Some(wav));
        assert!((out.duration_s - 1.6362).abs() < 1e-4);
    }
}
