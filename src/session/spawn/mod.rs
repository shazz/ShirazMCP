use std::path::PathBuf;

use tokio::process::{Child, ChildStdin};

use self::boot::{
    build_command, confirm_boot, kill_on_err, open_fifo_write, remove_stale, take_stdin,
    wait_for_fifo,
};
use self::control::ControlChannel;
use crate::error::Result;
use crate::session::fifo::{self, LogTail, HATARI_DEBUG_PREFIX};

mod boot;
mod control;

/// How to launch Hatari. Defaults reproduce the DEBUG.md reference
/// invocation for the MJJ demo (this is a local, single-user tool —
/// machine-local defaults are deliberate; every field is overridable via
/// the `start` tool).
#[derive(Debug, Clone)]
pub struct HatariConfig {
    pub hatari_path: PathBuf,
    pub tos_path: PathBuf,
    pub harddrive: Option<PathBuf>,
    pub machine: String,
    pub memsize_mb: u32,
    pub fast_boot: bool,
    pub fast_forward: bool,
    pub work_dir: PathBuf,
    pub sentinel_timeout_ms: u64,
    pub boot_timeout_ms: u64,
}

impl Default for HatariConfig {
    fn default() -> Self {
        Self {
            hatari_path: PathBuf::from("/usr/bin/hatari"),
            tos_path: PathBuf::from("/path/to/projects/MJJ/bin/hatari/TOS/tos162fr.img"),
            harddrive: None,
            machine: "ste".to_string(),
            memsize_mb: 1,
            fast_boot: true,
            fast_forward: true,
            work_dir: PathBuf::from("/tmp/shiraz-mcp"),
            sentinel_timeout_ms: 10_000,
            boot_timeout_ms: 15_000,
        }
    }
}

impl HatariConfig {
    pub fn fifo_path(&self) -> PathBuf {
        self.work_dir.join("hatari.fifo")
    }

    pub fn log_path(&self) -> PathBuf {
        self.work_dir.join("hatari.log")
    }

    pub fn ram_len(&self) -> usize {
        (self.memsize_mb as usize) << 20
    }
}

/// A live Hatari process + its two control planes (ADR-11) and log tail.
/// Owned by `Session<Running>` / `Session<Paused>` (type-state `Data`,
/// §5.2). Dropping it kills Hatari (RAII last resort via `kill_on_drop` —
/// §5.4 `stop` is the graceful path).
#[derive(Debug)]
pub struct HatariProcess {
    child: Child,
    fifo: tokio::fs::File,
    /// Hatari's stdin, held open for the session's life — the frozen-state
    /// control plane (ADR-11): DebugUI reads it once a breakpoint halts
    /// the CPU. Never closed except by `shutdown`/process exit.
    stdin: ChildStdin,
    /// Which of `fifo`/`stdin` currently services debugger commands.
    /// Mirrors the type-state (Running=Fifo, Paused=DebugStdin); mutated
    /// only by `freeze()`/`thaw()` (`control.rs`).
    control: ControlChannel,
    log: LogTail,
    config: HatariConfig,
}

impl HatariProcess {
    pub fn config(&self) -> &HatariConfig {
        &self.config
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Spawn + boot-gate. The ONLY constructor — see P1.md §5.1 for the
    /// exact seven-step procedure this implements (steps 1-5 live in
    /// `boot.rs`; this is the orchestration).
    pub async fn spawn(config: HatariConfig) -> Result<Self> {
        tokio::fs::create_dir_all(&config.work_dir).await?;
        remove_stale(&config.fifo_path()).await?;
        remove_stale(&config.log_path()).await?;

        let log_file = std::fs::File::create(config.log_path())?;
        let log_file_clone = log_file.try_clone()?;
        let mut child = build_command(&config, log_file, log_file_clone).spawn()?;
        let stdin_result = take_stdin(&mut child);
        let stdin = kill_on_err(&mut child, stdin_result)?;

        kill_on_err(&mut child, wait_for_fifo(&config).await)?;
        let mut fifo = kill_on_err(&mut child, open_fifo_write(&config).await)?;
        let mut log = LogTail::new(config.log_path());
        let boot = confirm_boot(&mut fifo, &mut log, config.boot_timeout_ms).await;
        kill_on_err(&mut child, boot)?;

        Ok(Self {
            child,
            fifo,
            stdin,
            control: ControlChannel::Fifo,
            log,
            config,
        })
    }

    /// Sentinel-synced debugger command (delegates to `fifo::debug`, §5.6),
    /// using `config.sentinel_timeout_ms`.
    pub async fn debug(&mut self, cmd: &str) -> Result<String> {
        self.debug_with_timeout(cmd, self.config.sentinel_timeout_ms)
            .await
    }

    pub async fn debug_with_timeout(&mut self, cmd: &str, timeout_ms: u64) -> Result<String> {
        let what = format!("sentinel after {cmd}");
        match self.control {
            ControlChannel::Fifo => {
                fifo::debug(
                    &mut self.fifo,
                    &mut self.log,
                    HATARI_DEBUG_PREFIX,
                    cmd,
                    timeout_ms,
                    &what,
                )
                .await
            }
            ControlChannel::DebugStdin => {
                fifo::debug(&mut self.stdin, &mut self.log, "", cmd, timeout_ms, &what).await
            }
        }
    }

    /// Raw wrapper-command writer (no sentinel — see §5.6 rationale).
    pub async fn write_wrapper(&mut self, line: &str) -> Result<()> {
        fifo::write_wrapper(&mut self.fifo, line).await
    }

    /// Send a raw control command (`hatari-path`/`hatari-option`/
    /// `hatari-shortcut`) over the fifo and wait until the log echoes
    /// `expect` (P11 §2.2 — each control command has a pinned echo; there is
    /// no debugger sentinel on this path, ADR-23). Collected lines are
    /// discarded: the echo is a synchronization barrier, not output.
    /// Fifo-plane only — callers must hold a Running session.
    pub async fn control_and_wait(
        &mut self,
        line: &str,
        expect: &str,
        what: &str,
        timeout_ms: u64,
    ) -> Result<()> {
        self.write_wrapper(line).await?;
        let mut sink = Vec::new();
        self.log
            .read_until(|l| l.contains(expect), what, timeout_ms, &mut sink)
            .await
    }

    // `shutdown` moved to `spawn/control.rs` (P11 §6.1 fallback) — this file
    // was at the 200-line ceiling once `control_and_wait` landed.
}
