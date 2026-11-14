//! Machine/CPU launch options for `start` (CT60-like Falcon support):
//! typed, validated, and turned into Hatari command-line flags in ONE place.
//!
//! Every field is `Option`: `None` means "do not pass the flag", so a `start`
//! that sets none of them produces exactly the command line it always did.
//! Validation is combinational (TT-RAM needs a TT/Falcon and 32-bit
//! addressing, 68060 needs 32-bit addressing, DSP is Falcon-only, ...) and
//! runs on the FINAL overlaid config, so a base config can't smuggle in an
//! invalid mix either.

use schemars::JsonSchema;
use serde::Deserialize;

use super::HatariConfig;
use crate::types::Addr;

/// Machine names Hatari 2.4 accepts for `--machine`.
pub const MACHINES: [&str; 6] = ["st", "megast", "ste", "megaste", "tt", "falcon"];
/// ST-RAM ceiling (the 24-bit-bus hardware limit); TT-RAM is separate.
pub const ST_RAM_MAX_MB: u32 = 14;
pub const TT_RAM_MAX_MB: u32 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
pub enum Fpu {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "68881")]
    F68881,
    #[serde(rename = "68882")]
    F68882,
    #[serde(rename = "internal")]
    Internal,
}

impl Fpu {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::F68881 => "68881",
            Self::F68882 => "68882",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Dsp {
    None,
    Dummy,
    Emu,
}

impl Dsp {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Dummy => "dummy",
            Self::Emu => "emu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Monitor {
    Mono,
    Rgb,
    Vga,
    Tv,
}

impl Monitor {
    fn as_str(self) -> &'static str {
        match self {
            Self::Mono => "mono",
            Self::Rgb => "rgb",
            Self::Vga => "vga",
            Self::Tv => "tv",
        }
    }
}

/// See the module doc. `Default` = "pass nothing".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineOptions {
    pub cpu_level: Option<u8>,
    pub fpu: Option<Fpu>,
    pub cpu_clock: Option<u32>,
    pub compatible: Option<bool>,
    pub cpu_exact: Option<bool>,
    pub mmu: Option<bool>,
    pub addr24: Option<bool>,
    pub tt_ram_mb: Option<u32>,
    pub dsp: Option<Dsp>,
    pub monitor: Option<Monitor>,
    /// Uppercase `C`..=`Z`.
    pub gemdos_drive: Option<char>,
    /// Program on the GEMDOS drive, as the caller wrote it (validated and
    /// turned into `C:\DIR\PROG.PRG` by [`atari_auto_path`]).
    pub auto_run: Option<String>,
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

impl MachineOptions {
    /// Effective address-bus width: 24-bit unless `addr24=false` was set.
    /// (Conservative: an unset flag is treated as Hatari's ST default, so
    /// nothing 32-bit-only is allowed by accident.)
    pub fn addr24_effective(&self) -> bool {
        self.addr24.unwrap_or(true)
    }

    /// Reject invalid combinations. `machine`/`memsize_mb` come from the
    /// owning config; `has_harddrive` says whether a GEMDOS dir is mounted.
    pub fn validate(
        &self,
        machine: &str,
        memsize_mb: u32,
        has_harddrive: bool,
    ) -> Result<(), String> {
        let m = machine.to_ascii_lowercase();
        if !MACHINES.contains(&m.as_str()) {
            return Err(format!(
                "machine '{machine}' is not one of {}",
                MACHINES.join("|")
            ));
        }
        if !(1..=ST_RAM_MAX_MB).contains(&memsize_mb) {
            return Err(format!(
                "memsize_mb {memsize_mb} out of range 1-{ST_RAM_MAX_MB} (ST-RAM only; \
                 14 MB is the 24-bit-bus limit - use tt_ram_mb for more)"
            ));
        }
        self.validate_cpu()?;
        self.validate_ram_and_dsp(&m)?;
        self.validate_drive(has_harddrive)
    }

    fn validate_cpu(&self) -> Result<(), String> {
        if let Some(l) = self.cpu_level {
            if l > 6 {
                return Err(format!("cpu_level {l} out of range 0-6 (6 = 68060)"));
            }
            if l == 6 && self.addr24_effective() {
                return Err("cpu_level 6 (68060) requires addr24=false".to_string());
            }
        }
        if let Some(c) = self.cpu_clock {
            if ![8, 16, 32].contains(&c) {
                return Err(format!("cpu_clock {c} must be 8, 16 or 32"));
            }
        }
        if self.mmu == Some(true) && self.cpu_level.is_some_and(|l| l < 3) {
            return Err("mmu=true needs cpu_level 3 (68030) or higher".to_string());
        }
        Ok(())
    }

    fn validate_ram_and_dsp(&self, machine: &str) -> Result<(), String> {
        if let Some(t) = self.tt_ram_mb {
            if t > TT_RAM_MAX_MB {
                return Err(format!("tt_ram_mb {t} out of range 0-{TT_RAM_MAX_MB}"));
            }
            if t % 4 != 0 {
                return Err(format!("tt_ram_mb {t} must be a multiple of 4"));
            }
            if t > 0 && !["tt", "falcon"].contains(&machine) {
                return Err(format!(
                    "tt_ram_mb needs machine tt or falcon (got {machine})"
                ));
            }
            if t > 0 && self.addr24 != Some(false) {
                return Err("tt_ram_mb > 0 requires addr24=false (TT-RAM lives above \
                            the 24-bit bus)"
                    .to_string());
            }
        }
        if self.dsp.is_some_and(|d| d != Dsp::None) && machine != "falcon" {
            return Err(format!("dsp is Falcon-only (got machine {machine})"));
        }
        Ok(())
    }

    fn validate_drive(&self, has_harddrive: bool) -> Result<(), String> {
        if let Some(d) = self.gemdos_drive {
            if !('C'..='Z').contains(&d) {
                return Err(format!("gemdos_drive '{d}' must be a letter C-Z"));
            }
            if !has_harddrive {
                return Err("gemdos_drive needs harddrive/hard_drive_dir".to_string());
            }
        }
        if let Some(p) = &self.auto_run {
            if !has_harddrive {
                return Err("auto_run needs harddrive/hard_drive_dir".to_string());
            }
            atari_auto_path(p, self.gemdos_drive.unwrap_or('C'))?;
        }
        Ok(())
    }

    /// Hatari flags, in an order that keeps `--machine` (applied first by the
    /// caller) ahead of the options it would otherwise reset. Call only on a
    /// validated value (`auto_run` is re-normalized; a bad one yields no flag).
    pub fn to_args(&self, machine: &str) -> Vec<String> {
        let mut a: Vec<String> = Vec::new();
        let mut push = |k: &str, v: String| {
            a.push(k.to_string());
            a.push(v);
        };
        if let Some(v) = self.tt_ram_mb {
            push("--ttram", v.to_string());
        }
        if let Some(v) = self.cpu_level {
            push("--cpulevel", v.to_string());
        }
        if let Some(v) = self.fpu {
            push("--fpu", v.as_str().to_string());
        }
        if let Some(v) = self.cpu_clock {
            push("--cpuclock", v.to_string());
        }
        for (k, v) in [
            ("--compatible", self.compatible),
            ("--cpu-exact", self.cpu_exact),
            ("--mmu", self.mmu),
            ("--addr24", self.addr24),
        ] {
            if let Some(v) = v {
                push(k, on_off(v).to_string());
            }
        }
        if let Some(d) = self.dsp.filter(|d| *d != Dsp::None || machine == "falcon") {
            push("--dsp", d.as_str().to_string());
        }
        if let Some(v) = self.monitor {
            push("--monitor", v.as_str().to_string());
        }
        if let Some(d) = self.gemdos_drive {
            push("--gemdos-drive", d.to_ascii_lowercase().to_string());
        }
        let drive = self.gemdos_drive.unwrap_or('C');
        if let Some(Ok(p)) = self.auto_run.as_deref().map(|p| atari_auto_path(p, drive)) {
            push("--auto", p);
        }
        a
    }
}

/// Turn a caller-supplied program path into Hatari's `--auto` form
/// (`C:\DIR\PROG.PRG`). The path is relative to the GEMDOS drive root: `/` or
/// `\` separators, an optional `C:` prefix (must match `drive`), no `.`/`..`
/// or empty components, none of `: < > " | ? *` - so it can never name
/// anything outside the mounted directory.
pub fn atari_auto_path(raw: &str, drive: char) -> Result<String, String> {
    let bad = |why: &str| format!("auto_run '{raw}': {why}");
    let mut s = raw.trim();
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        if !s[..1].eq_ignore_ascii_case(&drive.to_string()) {
            return Err(bad(&format!("drive prefix must be {drive}:")));
        }
        s = &s[2..];
    }
    let s = s.strip_prefix(['/', '\\']).unwrap_or(s);
    if s.is_empty() {
        return Err(bad("empty program path"));
    }
    let mut parts = Vec::new();
    for c in s.split(['/', '\\']) {
        if c.is_empty() || c == "." || c == ".." {
            return Err(bad("empty, '.' or '..' path component"));
        }
        if c.chars()
            .any(|ch| ch.is_control() || ":<>\"|?*".contains(ch))
        {
            return Err(bad("illegal character in path component"));
        }
        parts.push(c);
    }
    Ok(format!(
        "{}:\\{}",
        drive.to_ascii_uppercase(),
        parts.join("\\")
    ))
}

impl HatariConfig {
    /// Exclusive top of the addressable bus for this session.
    pub fn bus_limit(&self) -> u64 {
        if self.options.addr24_effective() {
            u64::from(Addr::BUS_LEN)
        } else {
            1 << 32
        }
    }

    /// Bus-range gate for tool-supplied addresses: `addr + len` must fit the
    /// session's bus (`len` 0 = start address only).
    pub fn check_addr(&self, addr: Addr, len: u64) -> Result<(), String> {
        if u64::from(addr.raw()) + len.max(1) > self.bus_limit() {
            let bits = if self.options.addr24_effective() {
                "24-bit ST bus; start with addr24=false for 32-bit addresses"
            } else {
                "32-bit address space"
            };
            return Err(format!("{addr} + {len} bytes exceeds the {bits}"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ct60() -> MachineOptions {
        MachineOptions {
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
            gemdos_drive: Some('C'),
            auto_run: Some("RIFT.TOS".into()),
        }
    }

    #[test]
    fn default_passes_no_flags_and_validates() {
        let o = MachineOptions::default();
        assert!(o.to_args("ste").is_empty());
        assert_eq!(o.validate("ste", 1, false), Ok(()));
    }

    #[test]
    fn ct60_falcon_builds_the_reference_flags() {
        let o = ct60();
        assert_eq!(o.validate("falcon", 14, true), Ok(()));
        let a = o.to_args("falcon").join(" ");
        assert_eq!(
            a,
            "--ttram 256 --cpulevel 6 --fpu internal --cpuclock 32 --compatible off \
             --cpu-exact off --mmu off --addr24 off --dsp emu --monitor vga \
             --gemdos-drive c --auto C:\\RIFT.TOS"
        );
    }

    fn err(o: &MachineOptions, machine: &str) -> String {
        o.validate(machine, 4, true).unwrap_err()
    }

    #[test]
    fn rejects_invalid_combinations() {
        let o = MachineOptions {
            tt_ram_mb: Some(8),
            addr24: Some(false),
            ..Default::default()
        };
        assert!(err(&o, "st").contains("tt or falcon"));
        let o = MachineOptions {
            tt_ram_mb: Some(8),
            ..Default::default()
        };
        assert!(err(&o, "tt").contains("addr24=false"));
        let o = MachineOptions {
            cpu_level: Some(6),
            addr24: Some(true),
            ..Default::default()
        };
        assert!(err(&o, "falcon").contains("68060"));
        let o = MachineOptions {
            cpu_level: Some(6),
            ..Default::default()
        };
        assert!(err(&o, "falcon").contains("addr24=false"));
        let o = MachineOptions {
            dsp: Some(Dsp::Emu),
            ..Default::default()
        };
        assert!(err(&o, "tt").contains("Falcon-only"));
    }

    #[test]
    fn rejects_out_of_range_values() {
        for (o, want) in [
            (
                MachineOptions {
                    cpu_level: Some(7),
                    ..Default::default()
                },
                "0-6",
            ),
            (
                MachineOptions {
                    cpu_clock: Some(24),
                    ..Default::default()
                },
                "8, 16 or 32",
            ),
            (
                MachineOptions {
                    tt_ram_mb: Some(1028),
                    ..Default::default()
                },
                "0-1024",
            ),
            (
                MachineOptions {
                    tt_ram_mb: Some(6),
                    ..Default::default()
                },
                "multiple of 4",
            ),
            (
                MachineOptions {
                    cpu_level: Some(2),
                    mmu: Some(true),
                    ..Default::default()
                },
                "mmu",
            ),
            (
                MachineOptions {
                    gemdos_drive: Some('A'),
                    ..Default::default()
                },
                "C-Z",
            ),
        ] {
            assert!(err(&o, "falcon").contains(want), "{want}");
        }
        let o = MachineOptions::default();
        assert!(o
            .validate("falcon", 15, false)
            .unwrap_err()
            .contains("1-14"));
        assert!(o.validate("falcon", 0, false).is_err());
        assert!(o
            .validate("amiga", 4, false)
            .unwrap_err()
            .contains("machine"));
    }

    #[test]
    fn drive_and_auto_need_a_harddrive() {
        let o = MachineOptions {
            auto_run: Some("A.PRG".into()),
            ..Default::default()
        };
        assert!(o
            .validate("ste", 1, false)
            .unwrap_err()
            .contains("hard_drive_dir"));
        let o = MachineOptions {
            gemdos_drive: Some('D'),
            ..Default::default()
        };
        assert!(o
            .validate("ste", 1, false)
            .unwrap_err()
            .contains("hard_drive_dir"));
    }

    #[test]
    fn dsp_none_is_allowed_everywhere_but_only_emitted_on_falcon() {
        let o = MachineOptions {
            dsp: Some(Dsp::None),
            ..Default::default()
        };
        assert_eq!(o.validate("ste", 1, false), Ok(()));
        assert!(o.to_args("ste").is_empty());
        assert_eq!(o.to_args("falcon"), ["--dsp", "none"]);
    }

    #[test]
    fn auto_path_normalizes_and_confines() {
        assert_eq!(atari_auto_path("RIFT.TOS", 'C').unwrap(), "C:\\RIFT.TOS");
        assert_eq!(
            atari_auto_path("c:\\auto\\x.prg", 'C').unwrap(),
            "C:\\auto\\x.prg"
        );
        assert_eq!(
            atari_auto_path("/sub/x.prg", 'd').unwrap(),
            "D:\\sub\\x.prg"
        );
        for bad in [
            "", "../x", "a/../x", "a//b", "./x", "D:\\x", "a\\b:c", "a\0b", "x*",
        ] {
            assert!(atari_auto_path(bad, 'C').is_err(), "{bad:?}");
        }
    }

    #[test]
    fn bus_check_follows_addr24() {
        let mut c = HatariConfig::default();
        assert!(c.check_addr(Addr::new(0xFF_FFFF), 1).is_ok());
        assert!(c.check_addr(Addr::new(0x100_0000), 0).is_err());
        c.options.addr24 = Some(false);
        assert!(c.check_addr(Addr::new(0x1000_0000), 4).is_ok());
        assert!(c.check_addr(Addr::new(0xFFFF_FFFC), 4).is_ok());
        assert!(c.check_addr(Addr::new(0xFFFF_FFFC), 5).is_err());
    }
}
