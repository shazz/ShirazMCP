use crate::error::{Error, Result};
use crate::session::spawn::HatariProcess;
use crate::types::{Addr, MemImage};

/// Snapshot `len` bytes at bus address `base` into a `MemImage` via
/// `savebin` (ADR-9). Used whole-RAM by `acquire` and ranged by
/// `view_memory_as_screen` (P4).
pub async fn acquire_range(proc: &mut HatariProcess, base: Addr, len: usize) -> Result<MemImage> {
    let path = proc.config().work_dir.join("ram.bin");
    proc.debug(&format!(
        "savebin {} ${:x} ${len:x}",
        path.display(),
        base.raw()
    ))
    .await?;

    let bytes = tokio::fs::read(&path).await.map_err(|e| Error::Fifo {
        context: format!("read ram dump {}", path.display()),
        source: e,
    })?;
    if bytes.len() != len {
        return Err(Error::Parse {
            context: "savebin length".to_string(),
            input: format!("{} bytes, expected {len}", bytes.len()),
        });
    }
    Ok(MemImage::new(bytes, base))
}

/// Snapshot all ST RAM (base $0, len = `config.ram_len()`). Valid in Running
/// or Paused; Paused gives a stable snapshot (the freezer-cartridge
/// workflow) — callers choose.
pub async fn acquire(proc: &mut HatariProcess) -> Result<MemImage> {
    let len = proc.config().ram_len();
    acquire_range(proc, Addr::new(0), len).await
}
