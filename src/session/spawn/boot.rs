//! Free functions behind `HatariProcess::spawn` (P1.md §5.1 steps 1-5).
//! Split out of `spawn/mod.rs` purely to keep that file under the line
//! ceiling — no behavior lives here that isn't part of the boot sequence.

use std::os::unix::fs::FileTypeExt;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, ChildStdin, Command};

use super::HatariConfig;
use crate::error::{Error, Result};
use crate::session::fifo::{self, LogTail, HATARI_DEBUG_PREFIX};

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// Best-effort kill on any post-spawn failure (the child is also
/// `kill_on_drop` — this just fails fast without waiting for reap).
pub(super) fn kill_on_err<T>(child: &mut Child, result: Result<T>) -> Result<T> {
    if result.is_err() {
        let _ = child.start_kill();
    }
    result
}

/// Extract the piped stdin handle — `build_command` always sets
/// `Stdio::piped()`, so this is the frozen-state control plane (ADR-11).
pub(super) fn take_stdin(child: &mut Child) -> Result<ChildStdin> {
    child.stdin.take().ok_or_else(|| {
        // WHY: `build_command` always sets `.stdin(Stdio::piped())` — a
        // missing pipe here means tokio/the OS violated that contract, not
        // a normal runtime failure.
        Error::Session("hatari spawned without a stdin pipe".to_string())
    })
}

pub(super) async fn remove_stale(path: &std::path::Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// `--alert-level fatal` (ADR-15): Hatari's `Log_AlertDlg` pops a modal SDL
/// dialog for any alert at or above the configured level (default `error`)
/// — e.g. a corrupt/version-mismatched `stateload` — blocking emulation
/// until a human clicks. `fatal` keeps every alert line in the log (file
/// logging is gated by `TextLogLevel`, independent of the dialog gate —
/// verified in log.c) while making dialogs impossible; the `state` tool
/// depends on error paths surfacing as parseable log lines, not dialogs.
///
/// stdin is left `Stdio::piped()` (ADR-11): the frozen-state control plane
/// (DebugUI reads it once a breakpoint halts the CPU), held open for the
/// session's life so `freeze()`/`thaw()` can write to it.
pub(super) fn build_command(
    config: &HatariConfig,
    log_file: std::fs::File,
    log_file_clone: std::fs::File,
) -> Command {
    let mut cmd = Command::new(&config.hatari_path);
    cmd.args(["--machine", &config.machine])
        .args(["--memsize", &config.memsize_mb.to_string()])
        .arg("--tos")
        .arg(&config.tos_path);
    if let Some(hd) = &config.harddrive {
        cmd.arg("--harddrive").arg(hd);
    }
    cmd.args(["--fast-boot", on_off(config.fast_boot)])
        .args(["--fast-forward", on_off(config.fast_forward)])
        .args(["--confirm-quit", "off"])
        .args(["--alert-level", "fatal"])
        .arg("--cmd-fifo")
        .arg(config.fifo_path())
        .stdout(log_file)
        .stderr(log_file_clone)
        .stdin(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

fn timeout_err(what: &str, waited_ms: u64) -> Error {
    Error::Timeout {
        what: what.to_string(),
        waited_ms,
    }
}

fn fifo_exists(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.file_type().is_fifo())
        .unwrap_or(false)
}

pub(super) async fn wait_for_fifo(config: &HatariConfig) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(config.boot_timeout_ms);
    while !fifo_exists(&config.fifo_path()) {
        if tokio::time::Instant::now() >= deadline {
            return Err(timeout_err("hatari fifo creation", config.boot_timeout_ms));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

pub(super) async fn open_fifo_write(config: &HatariConfig) -> Result<tokio::fs::File> {
    let mut opts = tokio::fs::OpenOptions::new();
    let open = opts.write(true).open(config.fifo_path());
    tokio::time::timeout(Duration::from_millis(config.boot_timeout_ms), open)
        .await
        .map_err(|_| timeout_err("opening hatari fifo for write", config.boot_timeout_ms))?
        .map_err(|e| Error::Fifo {
            context: "open fifo for write".to_string(),
            source: e,
        })
}

/// Step 5: the boot-gate sentinel round-trip — the debugger control plane
/// answering `evaluate pc` over the fifo is what "booted" means (no fixed
/// sleep).
pub(super) async fn confirm_boot(
    fifo: &mut tokio::fs::File,
    log: &mut LogTail,
    timeout_ms: u64,
) -> Result<String> {
    fifo::debug(
        fifo,
        log,
        HATARI_DEBUG_PREFIX,
        "evaluate pc",
        timeout_ms,
        "sentinel after evaluate pc",
    )
    .await
}
