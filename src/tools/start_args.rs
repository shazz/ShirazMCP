//! `start`'s input struct and the overlay + validation that turns it into a
//! `HatariConfig`. Split from `lifecycle.rs` to keep both files small.

use std::path::{Path, PathBuf};

use rmcp::ErrorData as McpError;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::session::spawn::options::{Dsp, Fpu, Monitor};
use crate::session::{HatariConfig, MachineOptions};

/// `start` — input. Every field optional; `None` → `HatariConfig::default()`
/// value (and, for the CPU/RAM/DSP options, "no Hatari flag"). Combinations
/// are validated before Hatari is launched.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct StartArgs {
    /// Path to the hatari binary.
    pub hatari_path: Option<String>,
    /// TOS ROM image path (must exist). Alias: `tos`.
    #[serde(alias = "tos")]
    pub tos_path: Option<String>,
    /// Host directory to mount as the GEMDOS drive (must exist; TOS
    /// auto-runs its AUTO/*.PRG). Alias: `hard_drive_dir`.
    #[serde(alias = "hard_drive_dir")]
    pub harddrive: Option<String>,
    /// "st" | "megast" | "ste" | "megaste" | "tt" | "falcon".
    pub machine: Option<String>,
    /// ST-RAM size in MiB (1-14; 14 is the 24-bit-bus hardware limit —
    /// use `tt_ram_mb` for more).
    pub memsize_mb: Option<u32>,
    /// CPU: 0-6 (0=68000 .. 3=68030, 4=68040, 6=68060). Level 6 needs
    /// `addr24=false`.
    pub cpu_level: Option<u8>,
    /// FPU: none | 68881 | 68882 | internal.
    pub fpu: Option<Fpu>,
    /// CPU clock in MHz: 8, 16 or 32.
    pub cpu_clock: Option<u32>,
    /// Hatari's slower, more compatible prefetch mode.
    pub compatible: Option<bool>,
    /// Cycle-exact CPU emulation.
    pub cpu_exact: Option<bool>,
    /// MMU emulation (needs cpu_level >= 3).
    pub mmu: Option<bool>,
    /// 24-bit addressing (the ST default). Set false for 32-bit addresses
    /// (TT-RAM, 68060); read/write/watch/disasm then accept addresses up to
    /// $FFFFFFFF.
    pub addr24: Option<bool>,
    /// TT-RAM in MiB, 0-1024, multiple of 4 (tt/falcon only; needs
    /// `addr24=false`). Appears from $01000000. rip/search_memory/snapshot
    /// tools scan ST-RAM only; use read_memory for TT-RAM.
    pub tt_ram_mb: Option<u32>,
    /// DSP emulation: none | dummy | emu (Falcon only).
    pub dsp: Option<Dsp>,
    /// Monitor: mono | rgb | vga | tv.
    pub monitor: Option<Monitor>,
    /// GEMDOS drive letter (C-Z, default C) for the mounted directory;
    /// needs `harddrive`.
    pub gemdos_drive: Option<String>,
    /// Program to auto-start, a path ON THE GEMDOS DRIVE (e.g. "RIFT.TOS" or
    /// "C:\\AUTO\\X.PRG"); no `..`; needs `harddrive`.
    pub auto_run: Option<String>,
}

fn invalid(msg: impl Into<String>) -> McpError {
    McpError::invalid_params(msg.into(), None)
}

/// A path the caller named in THIS call must exist (defaults are trusted).
fn existing(label: &str, raw: &str, want_dir: bool) -> Result<PathBuf, McpError> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(invalid(format!("{label}: empty or invalid path")));
    }
    let p = Path::new(raw);
    let ok = if want_dir { p.is_dir() } else { p.is_file() };
    if !ok {
        let kind = if want_dir { "directory" } else { "file" };
        return Err(invalid(format!("{label}: {raw} is not an existing {kind}")));
    }
    Ok(p.to_path_buf())
}

fn drive_letter(s: &str) -> Result<char, McpError> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Ok(c.to_ascii_uppercase()),
        _ => Err(invalid(format!(
            "gemdos_drive '{s}' must be one letter C-Z"
        ))),
    }
}

/// Overlay `args` on `base` and validate the result.
pub(super) fn overlay_config(
    base: HatariConfig,
    args: StartArgs,
) -> Result<HatariConfig, McpError> {
    let tos_path = match args.tos_path {
        Some(p) => existing("tos", &p, false)?,
        None => base.tos_path,
    };
    let harddrive = match args.harddrive {
        Some(p) => Some(existing("hard_drive_dir", &p, true)?),
        None => base.harddrive,
    };
    let b = base.options;
    let options = MachineOptions {
        cpu_level: args.cpu_level.or(b.cpu_level),
        fpu: args.fpu.or(b.fpu),
        cpu_clock: args.cpu_clock.or(b.cpu_clock),
        compatible: args.compatible.or(b.compatible),
        cpu_exact: args.cpu_exact.or(b.cpu_exact),
        mmu: args.mmu.or(b.mmu),
        addr24: args.addr24.or(b.addr24),
        tt_ram_mb: args.tt_ram_mb.or(b.tt_ram_mb),
        dsp: args.dsp.or(b.dsp),
        monitor: args.monitor.or(b.monitor),
        gemdos_drive: args
            .gemdos_drive
            .as_deref()
            .map(drive_letter)
            .transpose()?
            .or(b.gemdos_drive),
        auto_run: args.auto_run.or(b.auto_run),
    };
    let config = HatariConfig {
        hatari_path: args
            .hatari_path
            .map(PathBuf::from)
            .unwrap_or(base.hatari_path),
        tos_path,
        harddrive,
        machine: args.machine.unwrap_or(base.machine),
        memsize_mb: args.memsize_mb.unwrap_or(base.memsize_mb),
        options,
        ..base
    };
    config
        .options
        .validate(
            &config.machine,
            config.memsize_mb,
            config.harddrive.is_some(),
        )
        .map_err(invalid)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> String {
        std::env::temp_dir().display().to_string()
    }

    fn run(args: StartArgs) -> Result<HatariConfig, McpError> {
        overlay_config(HatariConfig::default(), args)
    }

    #[test]
    fn no_args_keeps_defaults() {
        let c = run(StartArgs::default()).unwrap();
        assert_eq!(c.machine, "ste");
        assert_eq!(c.options, MachineOptions::default());
    }

    #[test]
    fn ct60_like_falcon_validates() {
        let c = run(StartArgs {
            machine: Some("falcon".into()),
            memsize_mb: Some(14),
            cpu_level: Some(6),
            fpu: Some(Fpu::Internal),
            cpu_clock: Some(32),
            compatible: Some(false),
            cpu_exact: Some(false),
            mmu: Some(false),
            addr24: Some(false),
            tt_ram_mb: Some(256),
            dsp: Some(Dsp::Emu),
            monitor: Some(Monitor::Vga),
            harddrive: Some(dir()),
            gemdos_drive: Some("c".into()),
            auto_run: Some("RIFT.TOS".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(c.options.gemdos_drive, Some('C'));
        assert_eq!(c.options.tt_ram_mb, Some(256));
    }

    #[test]
    fn invalid_combos_are_invalid_params() {
        let tt_on_st = StartArgs {
            machine: Some("st".into()),
            tt_ram_mb: Some(8),
            addr24: Some(false),
            ..Default::default()
        };
        assert!(run(tt_on_st).unwrap_err().message.contains("tt or falcon"));
        let cpu6 = StartArgs {
            cpu_level: Some(6),
            ..Default::default()
        };
        assert!(run(cpu6).unwrap_err().message.contains("addr24=false"));
        let dsp = StartArgs {
            dsp: Some(Dsp::Emu),
            ..Default::default()
        };
        assert!(run(dsp).unwrap_err().message.contains("Falcon-only"));
        let mem = StartArgs {
            memsize_mb: Some(15),
            ..Default::default()
        };
        assert!(run(mem).unwrap_err().message.contains("1-14"));
    }

    #[test]
    fn paths_must_exist_and_drive_must_be_a_letter() {
        let tos = StartArgs {
            tos_path: Some("/nonexistent/tos.img".into()),
            ..Default::default()
        };
        assert!(run(tos)
            .unwrap_err()
            .message
            .contains("not an existing file"));
        let hd = StartArgs {
            harddrive: Some("/nonexistent/hd".into()),
            ..Default::default()
        };
        assert!(run(hd)
            .unwrap_err()
            .message
            .contains("not an existing directory"));
        let drv = StartArgs {
            harddrive: Some(dir()),
            gemdos_drive: Some("CD".into()),
            ..Default::default()
        };
        assert!(run(drv).unwrap_err().message.contains("one letter"));
        let trav = StartArgs {
            harddrive: Some(dir()),
            auto_run: Some("../evil.prg".into()),
            ..Default::default()
        };
        assert!(run(trav).is_err());
    }

    #[test]
    fn serde_accepts_aliases_and_rejects_bad_enum() {
        let a: StartArgs =
            serde_json::from_str(r#"{"tos":"x","hard_drive_dir":"y","fpu":"68882"}"#).unwrap();
        assert_eq!(a.tos_path.as_deref(), Some("x"));
        assert_eq!(a.harddrive.as_deref(), Some("y"));
        assert_eq!(a.fpu, Some(Fpu::F68882));
        assert!(serde_json::from_str::<StartArgs>(r#"{"monitor":"crt"}"#).is_err());
    }
}
