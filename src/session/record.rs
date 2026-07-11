//! P11 recording driver: Hatari's native WAV recorder over the control fifo
//! (ADR-23). Wire strings + log echoes pinned live — see
//! docs/plan/P11.md §2.2/§2.3. Owns the fast-forward suspend/restore and the
//! real-time record window; knows nothing about DSP or tool schemas.

use std::path::Path;

use crate::error::Result;
use crate::session::spawn::HatariProcess;

pub(crate) const RECSOUND_CMD: &str = "hatari-shortcut recsound";
pub(crate) const SOUNDOUT_MARK: &str = "soundout: ";
pub(crate) const FF_MARK: &str = "- '--fast-forward'";
pub(crate) const REC_STARTED_MARK: &str = "sound data recording has been started";
pub(crate) const REC_STOPPED_MARK: &str = "data recording has been stopped";

pub(crate) fn soundout_cmd(path: &Path) -> String {
    format!("hatari-path soundout {}", path.display())
}

pub(crate) fn ff_cmd(on: bool) -> String {
    format!(
        "hatari-option --fast-forward {}",
        if on { "on" } else { "off" }
    )
}

/// Record `seconds` of real-time audio to `wav_path` (absolute, .wav —
/// validated by the caller). Returns whether fast-forward was suspended and
/// restored (true iff the session config boots with it on).
///
/// Sequence (each step = `control_and_wait` with the pinned mark,
/// `config.sentinel_timeout_ms`):
///   1. `soundout_cmd(wav_path)`               → `SOUNDOUT_MARK`
///   2. `ff_cmd(false)`  [if `config.fast_forward`] → `FF_MARK`
///   3. `RECSOUND_CMD`                          → `REC_STARTED_MARK`
///   4. `tokio::time::sleep(seconds)`           — the real-time window (§2.3)
///   5. `RECSOUND_CMD`                          → `REC_STOPPED_MARK`
///   6. `ff_cmd(true)`   [if `config.fast_forward`] → `FF_MARK`
///
/// Error precedence mirrors ADR-20: a record-window error (3-5) wins over a
/// restore error (6); restore is still attempted after a window failure.
pub async fn record_wav(proc: &mut HatariProcess, seconds: f64, wav_path: &Path) -> Result<bool> {
    let t = proc.config().sentinel_timeout_ms;
    let ff = proc.config().fast_forward;
    proc.control_and_wait(
        &soundout_cmd(wav_path),
        SOUNDOUT_MARK,
        "soundout path change",
        t,
    )
    .await?;
    if ff {
        proc.control_and_wait(&ff_cmd(false), FF_MARK, "fast-forward off", t)
            .await?;
    }
    let window = run_window(proc, seconds, t).await;
    let restore = if ff {
        proc.control_and_wait(&ff_cmd(true), FF_MARK, "fast-forward on", t)
            .await
    } else {
        Ok(())
    };
    window.and(restore).map(|()| ff)
}

async fn run_window(proc: &mut HatariProcess, seconds: f64, t: u64) -> Result<()> {
    proc.control_and_wait(RECSOUND_CMD, REC_STARTED_MARK, "sound recording start", t)
        .await?;
    tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)).await;
    proc.control_and_wait(RECSOUND_CMD, REC_STOPPED_MARK, "sound recording stop", t)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_strings_pinned() {
        assert_eq!(
            soundout_cmd(Path::new("/tmp/a.wav")),
            "hatari-path soundout /tmp/a.wav"
        );
        assert_eq!(ff_cmd(false), "hatari-option --fast-forward off");
        assert_eq!(ff_cmd(true), "hatari-option --fast-forward on");
        assert_eq!(RECSOUND_CMD, "hatari-shortcut recsound");
    }

    #[test]
    fn log_marks_pinned() {
        let started = "INFO : WAV sound data recording has been started.";
        let stopped = "INFO : WAV Sound data recording has been stopped.";
        assert!(started.contains(REC_STARTED_MARK));
        assert!(stopped.contains(REC_STOPPED_MARK));
        assert!(!stopped.contains(REC_STARTED_MARK));
        assert!("- '--fast-forward'".contains(FF_MARK));
        assert!("soundout: a -> b".contains(SOUNDOUT_MARK));
    }
}
