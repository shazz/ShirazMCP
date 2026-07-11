//! Text pretty-printer for `machine_state` (P10) — split from
//! machine_wire.rs for the 200-line budget, same pattern as insn_out.rs.
//! Pure formatting over the wire output; no session/emulator access.

use super::debugger::RegistersOutput;
use super::machine_wire::{MachineStateOutput, PaletteEntry, VideoMemoryOutput, VideoOutput};

/// Format is contractual (golden test `pretty_golden_running`, P10.md §6.5).
pub(crate) fn pretty(o: &MachineStateOutput) -> String {
    let freeze = if o.momentary_freeze {
        " (momentarily frozen)"
    } else {
        " (already frozen)"
    };
    let mut lines = vec![
        "=== machine_state — Atari ST snapshot ===".to_string(),
        format!("{:<8}: {}{freeze}", "session", o.session),
    ];
    lines.extend(cpu_lines(&o.cpu));
    lines.extend(video_lines(&o.video));
    lines.extend(mem_lines(&o.video_memory));
    lines.join("\n")
}

fn video_lines(v: &VideoOutput) -> Vec<String> {
    let dims = v
        .dimensions
        .as_deref()
        .map_or_else(String::new, |d| format!(" ({d})"));
    let row = |c: &[PaletteEntry]| {
        c.iter()
            .map(|e| e.raw.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    vec![
        format!(
            "{:<8}: {}{dims}, {} Hz, base {}, {} palette",
            "video", v.resolution, v.sync_hz, v.screen_base, v.palette_type
        ),
        format!("{:<8}: {}", "palette", row(&v.palette[0..8])),
        format!("{:10}{}", "", row(&v.palette[8..16])),
    ]
}

fn cpu_lines(c: &RegistersOutput) -> Vec<String> {
    let f = &c.sr;
    let b = |v: bool| if v { "1" } else { "0" };
    let sr = format!(
        "PC {}   SR T={} S={} X={} N={} Z={} V={} C={} IMASK={}",
        c.pc,
        f.trace,
        b(f.supervisor),
        b(f.x),
        b(f.n),
        b(f.z),
        b(f.v),
        b(f.c),
        f.imask
    );
    vec![
        sr,
        format!("D0 {} D1 {} D2 {} D3 {}", c.d[0], c.d[1], c.d[2], c.d[3]),
        format!("D4 {} D5 {} D6 {} D7 {}", c.d[4], c.d[5], c.d[6], c.d[7]),
        format!("A0 {} A1 {} A2 {} A3 {}", c.a[0], c.a[1], c.a[2], c.a[3]),
        format!("A4 {} A5 {} A6 {} A7 {}", c.a[4], c.a[5], c.a[6], c.a[7]),
        format!("USP {}  ISP {}", c.usp, c.isp),
    ]
}

/// `note` wins over `path`; `None` field -> both lines "not requested".
fn mem_lines(m: &Option<VideoMemoryOutput>) -> Vec<String> {
    let (note, png, raw) = m.as_ref().map_or((None, None, None), |mem| {
        (
            mem.note.as_deref(),
            mem.png_path.as_deref(),
            mem.raw_path.as_deref(),
        )
    });
    let line = |label: &str, path: Option<&str>, suffix: &str| {
        let text = match (note, path) {
            (Some(n), _) => n.to_string(),
            (None, Some(p)) => format!("{p}{suffix}"),
            (None, None) => "not requested".to_string(),
        };
        format!("{label:<8}: {text}")
    };
    vec![
        line("screen", png, " (attached inline)"),
        line("raw dump", raw, ""),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::{Registers, SrFlags};
    use crate::ripper::image::ScreenRes;
    use crate::tools::machine_wire::{build_output, VideoRegs};
    use crate::types::Addr;

    fn probe_video() -> VideoRegs {
        VideoRegs {
            base: Addr::new(0x032200),
            sync_hz: 50,
            res: Some(ScreenRes::Low),
            words: [
                0x0000, 0x0777, 0x0555, 0x0002, 0x0004, 0x0006, 0x0017, 0x0027, 0x0047, 0x0057,
                0x0103, 0x0204, 0x0305, 0x0407, 0x0427, 0x0001,
            ],
        }
    }

    fn fixture_regs() -> Registers {
        Registers {
            d: [0x03FF_0052, 0x0003_5CF0, 0, 0, 0, 0, 0, 0x0000_005D],
            a: [0x00FF_8A00, 0, 0, 0, 0, 0, 0, 0x000F_7FE2],
            usp: 0x000F_7FEE,
            isp: 0x000F_7FE2,
            sr: SrFlags {
                trace: 0,
                supervisor: true,
                x: false,
                n: true,
                z: false,
                v: false,
                c: false,
                imask: 6,
            },
            pc: Addr::new(0x00DC74),
        }
    }

    fn fixture_output(
        session: &str,
        momentary_freeze: bool,
        mem: Option<VideoMemoryOutput>,
    ) -> MachineStateOutput {
        build_output(
            session,
            momentary_freeze,
            fixture_regs(),
            &probe_video(),
            mem,
        )
    }

    #[test]
    fn pretty_golden_running() {
        let mem = Some(VideoMemoryOutput {
            len: 32000,
            png_path: Some("/tmp/shiraz-mcp/machine-0001.png".to_string()),
            raw_path: None,
            note: None,
        });
        let o = fixture_output("running", true, mem);
        let expected = "=== machine_state — Atari ST snapshot ===\n\
session : running (momentarily frozen)\n\
PC $00DC74   SR T=0 S=1 X=0 N=1 Z=0 V=0 C=0 IMASK=6\n\
D0 $03FF0052 D1 $00035CF0 D2 $00000000 D3 $00000000\n\
D4 $00000000 D5 $00000000 D6 $00000000 D7 $0000005D\n\
A0 $00FF8A00 A1 $00000000 A2 $00000000 A3 $00000000\n\
A4 $00000000 A5 $00000000 A6 $00000000 A7 $000F7FE2\n\
USP $000F7FEE  ISP $000F7FE2\n\
video   : low (320x200), 50 Hz, base $032200, st palette\n\
palette : $0000 $0777 $0555 $0002 $0004 $0006 $0017 $0027\n          $0047 $0057 $0103 $0204 $0305 $0407 $0427 $0001\n\
screen  : /tmp/shiraz-mcp/machine-0001.png (attached inline)\n\
raw dump: not requested";
        assert_eq!(pretty(&o), expected);
    }

    #[test]
    fn pretty_paused_note_and_rawdump() {
        let note = "resolution register $FF8260 reads 3 (invalid) — screen skipped".to_string();
        let mem = Some(VideoMemoryOutput {
            len: 0,
            png_path: None,
            raw_path: None,
            note: Some(note.clone()),
        });
        let o = fixture_output("paused", false, mem);
        let text = pretty(&o);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[1].ends_with(" (already frozen)"));
        assert_eq!(lines[11], format!("screen  : {note}"));
        assert_eq!(lines[12], format!("raw dump: {note}"));
    }

    #[test]
    fn pretty_not_requested() {
        let o = fixture_output("paused", false, None);
        let text = pretty(&o);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[11], "screen  : not requested");
        assert_eq!(lines[12], "raw dump: not requested");
    }
}
