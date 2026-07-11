//! External-assembler driver (ADR-18): shells out to vasm (`vasmm68k_mot`,
//! the MJJ demos' own assembler — dialect match) with an explicit argv
//! vector, never a shell. Temp files under a controlled dir, RAII cleanup,
//! bounded diagnostics. Leaf module (`error`/`types` only, only `tools/`
//! imports it); pure text prep + diagnostics parsing live in `source`.

pub mod source;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::error::{Error, Result};

pub const VASM_ENV: &str = "SHIRAZMCP_VASM";
pub const DEFAULT_VASM: &str = "/path/to/projects/MJJ/bin/vasm/vasmm68k_mot";
pub const SRC_MAX_BYTES: usize = 65_536;
pub const BIN_MAX_BYTES: usize = 65_536;
pub const VASM_TIMEOUT_MS: u64 = 10_000;

/// Outcome of one assembler run. `ok == false` means vasm rejected the
/// SOURCE (errors carry diagnostics); infra failures are `Error::Assembler`.
#[derive(Debug)]
pub struct AsmResult {
    pub ok: bool,
    pub bytes: Vec<u8>,          // empty iff !ok
    pub errors: Vec<String>,     // sanitized + bounded (§5.3)
    pub warnings: Vec<String>,   // sanitized + bounded
    pub listing: Option<String>, // bounded; Some only if requested AND ok
}

/// `Error::Assembler` builder — collapses the repeated struct literal below.
fn asm_err(context: &str, message: impl std::fmt::Display) -> Error {
    Error::Assembler {
        context: context.to_string(),
        message: message.to_string(),
    }
}

/// `SHIRAZMCP_VASM` override, else `DEFAULT_VASM`. No existence check here —
/// the spawn error is the single source of truth (avoids TOCTOU).
pub fn vasm_path() -> PathBuf {
    std::env::var(VASM_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_VASM))
}

/// Drive vasm over an already-prepared source (org injected, size-checked —
/// `source::prepare_source`). `want_listing` adds `-L`.
pub async fn assemble(vasm: &Path, prepared_src: &str, want_listing: bool) -> Result<AsmResult> {
    let tmp = TempAsm::new()?;
    tokio::fs::write(&tmp.src, prepared_src)
        .await
        .map_err(|e| asm_err("temp", format!("write {}: {e}", tmp.src.display())))?;
    let lst = want_listing.then_some(tmp.lst.as_path());
    let argv = build_argv(&tmp.src, &tmp.bin, lst);
    let (status_ok, diag) = run_vasm(vasm, argv).await?;
    let (mut errors, warnings) = source::parse_diagnostics(&diag, file_name_str(&tmp.src));
    let (bytes, listing) = collect_outputs(&tmp, status_ok, want_listing).await?;
    if !status_ok && errors.is_empty() {
        errors.push("assembler failed with no diagnostics".to_string());
    }
    Ok(AsmResult {
        ok: status_ok,
        bytes,
        errors,
        warnings,
        listing,
    })
}

/// Naming-uniqueness counter only (not shared state), for concurrent `assemble()` calls.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Temp-file trio (RAII cleanup); names: `asm-{pid}-{seq}.{s,bin,lst}` — no user input.
struct TempAsm {
    src: PathBuf,
    bin: PathBuf,
    lst: PathBuf,
}

impl TempAsm {
    fn new() -> Result<Self> {
        let dir = std::env::temp_dir().join("shirazmcp-asm");
        std::fs::create_dir_all(&dir)
            .map_err(|e| asm_err("temp", format!("create {}: {e}", dir.display())))?;
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let stem = format!("asm-{}-{seq}", std::process::id());
        Ok(Self {
            src: dir.join(format!("{stem}.s")),
            bin: dir.join(format!("{stem}.bin")),
            lst: dir.join(format!("{stem}.lst")),
        })
    }
}

impl Drop for TempAsm {
    fn drop(&mut self) {
        for p in [&self.src, &self.bin, &self.lst] {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn file_name_str(p: &Path) -> &str {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("")
}

/// Pure argv builder (§0 invocation): `["-quiet","-Fbin","-o",bin,("-L",lst)?,src]`.
fn build_argv(src: &Path, bin: &Path, lst: Option<&Path>) -> Vec<OsString> {
    let mut argv = vec![
        OsString::from("-quiet"),
        OsString::from("-Fbin"),
        OsString::from("-o"),
        bin.as_os_str().to_os_string(),
    ];
    if let Some(lst) = lst {
        argv.push(OsString::from("-L"));
        argv.push(lst.as_os_str().to_os_string());
    }
    argv.push(src.as_os_str().to_os_string());
    argv
}

/// Spawn vasm with an explicit argv vector, wait (bounded by
/// `VASM_TIMEOUT_MS`); returns `(exit_success, stdout ⧺ "\n" ⧺ stderr)`.
// SECURITY: never a shell, never string interpolation — the source text is
// file content, not an argument.
async fn run_vasm(vasm: &Path, argv: Vec<OsString>) -> Result<(bool, String)> {
    let child = tokio::process::Command::new(vasm)
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| spawn_error(vasm, e))?;
    let timeout = Duration::from_millis(VASM_TIMEOUT_MS);
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| asm_err("timeout", format!("vasm exceeded {VASM_TIMEOUT_MS} ms")))?
        .map_err(|e| asm_err("spawn", e))?;
    let diag = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok((output.status.success(), diag))
}

fn spawn_error(vasm: &Path, e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        let msg = format!(
            "vasm not found at {}; set {VASM_ENV} to the vasmm68k_mot binary",
            vasm.display()
        );
        asm_err("spawn", msg)
    } else {
        asm_err("spawn", e)
    }
}

/// Read `bin` (must exist when `ok` — missing with exit 0 is infra
/// failure), enforce `BIN_MAX_BYTES`; read + bound the listing if requested.
async fn collect_outputs(
    tmp: &TempAsm,
    ok: bool,
    want_listing: bool,
) -> Result<(Vec<u8>, Option<String>)> {
    if !ok {
        return Ok((Vec::new(), None));
    }
    let bytes = read_checked_bin(&tmp.bin).await?;
    let listing = if want_listing {
        let text = tokio::fs::read_to_string(&tmp.lst)
            .await
            .unwrap_or_default();
        Some(source::bound_listing(&text, file_name_str(&tmp.src)))
    } else {
        None
    };
    Ok((bytes, listing))
}

async fn read_checked_bin(bin: &Path) -> Result<Vec<u8>> {
    let bytes = tokio::fs::read(bin).await.map_err(|_| {
        asm_err(
            "output",
            format!("vasm exited 0 but {} is missing", bin.display()),
        )
    })?;
    if bytes.len() > BIN_MAX_BYTES {
        return Err(asm_err("output", "assembled output exceeds 64 KiB"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_no_listing() {
        let argv = build_argv(Path::new("/t/a.s"), Path::new("/t/a.bin"), None);
        assert_eq!(
            argv,
            vec![
                OsString::from("-quiet"),
                OsString::from("-Fbin"),
                OsString::from("-o"),
                OsString::from("/t/a.bin"),
                OsString::from("/t/a.s"),
            ]
        );
    }

    #[test]
    fn argv_with_listing() {
        let argv = build_argv(
            Path::new("/t/a.s"),
            Path::new("/t/a.bin"),
            Some(Path::new("/t/a.lst")),
        );
        assert_eq!(
            argv,
            vec![
                OsString::from("-quiet"),
                OsString::from("-Fbin"),
                OsString::from("-o"),
                OsString::from("/t/a.bin"),
                OsString::from("-L"),
                OsString::from("/t/a.lst"),
                OsString::from("/t/a.s"),
            ]
        );
    }

    #[test]
    fn vasm_path_env_override() {
        // SAFETY-adjacent, serialized within this test only: set/restore
        // the env var around the assertion — `env::set_var`/`remove_var`
        // race other tests touching the same var, but no other test does.
        std::env::set_var(VASM_ENV, "/x/y");
        assert_eq!(vasm_path(), PathBuf::from("/x/y"));
        std::env::remove_var(VASM_ENV);
        assert_eq!(vasm_path(), PathBuf::from(DEFAULT_VASM));
    }

    #[test]
    fn temp_asm_drop_removes() {
        let tmp = TempAsm::new().unwrap();
        let (src, bin, lst) = (tmp.src.clone(), tmp.bin.clone(), tmp.lst.clone());
        std::fs::write(&src, "x").unwrap();
        std::fs::write(&bin, "x").unwrap();
        std::fs::write(&lst, "x").unwrap();
        drop(tmp);
        assert!(!src.exists());
        assert!(!bin.exists());
        assert!(!lst.exists());
    }

    #[tokio::test]
    async fn run_vasm_missing_binary() {
        let err = run_vasm(Path::new("/nonexistent/vasm"), vec![])
            .await
            .unwrap_err();
        match err {
            Error::Assembler { message, .. } => assert!(message.contains(VASM_ENV)),
            other => panic!("expected Error::Assembler, got {other:?}"),
        }
    }
}
