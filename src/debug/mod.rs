//! Typed wrappers over the raw sentinel-synced `debug(cmd)` primitive
//! (`session::fifo`). This module must not import `tokio::process` or
//! `session::fifo` internals — only the `HatariProcess` public surface
//! (layering: `session` → `debug`).

pub mod basepage;
pub mod breakpoint;
pub mod parse;
pub mod profile;

use crate::error::Result;
use crate::session::spawn::HatariProcess;
use crate::types::Addr;

/// The testability seam: anything that can run a sentinel-synced debugger
/// command. Production impl is `HatariProcess`; unit tests use a canned-
/// response fake. Native async-fn-in-trait — not object-safe and doesn't
/// need to be, all users are generic.
pub trait DebugChannel: Send {
    fn debug(&mut self, cmd: &str) -> impl std::future::Future<Output = Result<String>> + Send;
}

impl DebugChannel for HatariProcess {
    async fn debug(&mut self, cmd: &str) -> Result<String> {
        HatariProcess::debug(self, cmd).await
    }
}

/// CPU registers as parsed from `cpureg` (fixture-pinned, §6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registers {
    pub d: [u32; 8],
    pub a: [u32; 8],
    pub usp: u32,
    pub isp: u32,
    pub sr: SrFlags,
    /// Current PC = the address of the pending instruction (the disasm
    /// line in `cpureg` output), NOT "Next PC".
    pub pc: Addr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SrFlags {
    pub trace: u8,
    pub supervisor: bool,
    pub x: bool,
    pub n: bool,
    pub z: bool,
    pub v: bool,
    pub c: bool,
    pub imask: u8,
}

/// Outcome of `symbols prg` — success and no-symbols are BOTH normal (the
/// committed MJJ build has no symbol table; ADR-8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolLoad {
    Loaded { total: u32, text: u32 },
    NoSymbols,
}

pub async fn registers(chan: &mut impl DebugChannel) -> Result<Registers> {
    parse::parse_cpureg(&chan.debug("cpureg").await?)
}

/// The 16 emulated YM2149 (PSG) registers via `info ym` — a pure dump of
/// Hatari's `PSGRegisters[]` array (psg.c `PSG_Info`). Never touches the
/// $FF8800 select port: the running program's PSG state is not disturbed
/// (P17 / the ADR-33 no-freeze decision).
pub async fn ym_registers(chan: &mut impl DebugChannel) -> Result<[u8; 16]> {
    parse::parse_info_ym(&chan.debug("info ym").await?)
}

/// The MFP 68901 register set via `info mfp` — Hatari's internal MFP
/// struct (mfp.c `MFP_Show`), NOT IoMem: timer data lines carry the
/// last-written RELOAD values, which is what a restore stub must
/// reinstall. `// verify live: TADR line semantics on a running timer.`
pub async fn mfp_registers(chan: &mut impl DebugChannel) -> Result<parse::MfpRegs> {
    parse::parse_info_mfp(&chan.debug("info mfp").await?)
}

pub async fn evaluate(chan: &mut impl DebugChannel, expr: &str) -> Result<u32> {
    parse::parse_evaluate(&chan.debug(&format!("evaluate {expr}")).await?)
}

/// `count` is bytes; Hatari's `m` rounds output up to whole 16-byte lines,
/// so the parsed buffer is truncated to the requested count.
pub async fn read_bytes(chan: &mut impl DebugChannel, addr: Addr, count: usize) -> Result<Vec<u8>> {
    let out = chan.debug(&format!("m ${:x} {count}", addr.raw())).await?;
    let mut bytes = parse::parse_memdump(&out)?;
    bytes.truncate(count);
    Ok(bytes)
}

/// Max data-byte tokens safely embeddable in ONE `memwrite` debugger line.
/// Hatari's `DebugUI_ParseCommand` (`hatari/src/debug/debugui.c`) tokenizes
/// the whole line with `strtok` into a fixed `char *psArgs[64]`; a
/// `memwrite $addr $b1 $b2 …` line spends 2 slots on `memwrite`+`$addr`, so
/// more than 61 byte tokens overflows the array and Hatari rejects the
/// WHOLE line — not a partial write. Confirmed live against a real Hatari
/// 2.4.1 session: N=61 always succeeds, N=62 always fails with `"Error: too
/// many arguments (currently up to 64 supported)"` — deterministic (a hard
/// token-count ceiling, not a timing race), reproduced on every attempt.
/// That message's lowercase "rror" also slipped past the old
/// `.contains("ERROR")` check below, so a fully-rejected write used to come
/// back as `Ok(())`. 32 keeps ~2x margin under the 61-byte hard ceiling.
const MEMWRITE_CHUNK_BYTES: usize = 32;

/// Writes `bytes` as however many `memwrite` lines `MEMWRITE_CHUNK_BYTES`
/// requires — the only way callers reach Hatari's `memwrite`, so every
/// write path (the `write_memory` tool, `patch`) inherits the chunking.
pub async fn write_bytes(chan: &mut impl DebugChannel, addr: Addr, bytes: &[u8]) -> Result<()> {
    for (i, chunk) in bytes.chunks(MEMWRITE_CHUNK_BYTES).enumerate() {
        let at = Addr::new(addr.raw() + (i * MEMWRITE_CHUNK_BYTES) as u32);
        write_memwrite_line(chan, at, chunk).await?;
    }
    Ok(())
}

/// One `memwrite` debugger line, ≤ `MEMWRITE_CHUNK_BYTES` bytes.
async fn write_memwrite_line(chan: &mut impl DebugChannel, addr: Addr, bytes: &[u8]) -> Result<()> {
    // Each value token MUST carry its own `$` prefix: Hatari's `memwrite`
    // parses bare tokens in the debugger's configured `nNumberBase`
    // (`DebugCpu_MemWrite` -> `Eval_Number(.., NUM_TYPE_NORMAL)` in
    // hatari/src/debug/{debugcpu,evaluate}.c), which defaults to DECIMAL
    // (`configuration.c`). Without `$`, a byte like `0x82` silently writes
    // decimal 82 (wrong, no error) and a byte like `0xfc` fails outright
    // ("Bad value 'fc'!") — verified live against a real Hatari session.
    let hex = bytes
        .iter()
        .map(|b| format!("${b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    let out = chan
        .debug(&format!("memwrite ${:x} {hex}", addr.raw()))
        .await?;
    // Case-insensitive: Hatari's own "too many arguments" rejection prints
    // `"Error: …"` (title case), which a bare `.contains("ERROR")` check
    // misses entirely — the exact way a fully-rejected write used to come
    // back as success (see `MEMWRITE_CHUNK_BYTES`).
    match out
        .lines()
        .find(|l| l.to_ascii_uppercase().contains("ERROR"))
    {
        Some(line) => Err(crate::error::Error::Parse {
            context: "memwrite".to_string(),
            input: line.to_string(),
        }),
        None => Ok(()),
    }
}

pub async fn load_symbols(chan: &mut impl DebugChannel) -> Result<SymbolLoad> {
    parse::parse_symbols(&chan.debug("symbols prg").await?)
}

pub async fn screenshot(chan: &mut impl DebugChannel, path: &std::path::Path) -> Result<()> {
    let out = chan
        .debug(&format!("screenshot {}", path.display()))
        .await?;
    parse::parse_screenshot(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        responses: Vec<String>,
        sent: Vec<String>,
    }

    impl DebugChannel for Fake {
        async fn debug(&mut self, cmd: &str) -> Result<String> {
            self.sent.push(cmd.to_string());
            Ok(self.responses.remove(0))
        }
    }

    const FIXTURE: &str = include_str!("../../tests/fixtures/hatari_session.log");

    #[tokio::test]
    async fn registers_roundtrip_via_channel() {
        let block = parse::fixture_block(FIXTURE, "cpureg");
        let mut fake = Fake {
            responses: vec![block],
            sent: vec![],
        };
        let regs = registers(&mut fake).await.unwrap();
        assert_eq!(regs.d[0], 0x03FF_0052);
        assert_eq!(regs.pc, Addr::new(0xdc74));
        assert_eq!(fake.sent, vec!["cpureg".to_string()]);
    }

    /// §2.2 probe 1, verbatim.
    fn probe_ym_regs() -> [u8; 16] {
        [
            0x59, 0x00, 0xEE, 0x0E, 0xCD, 0x02, 0x07, 0x2A, 0x0B, 0x0B, 0x0E, 0x00, 0x00, 0x00,
            0x25, 0x00,
        ]
    }

    #[tokio::test]
    async fn ym_registers_sends_info_ym() {
        let block = "Reg $00 : $59\nReg $01 : $00\nReg $02 : $EE\nReg $03 : $0E\n\
Reg $04 : $CD\nReg $05 : $02\nReg $06 : $07\nReg $07 : $2A\nReg $08 : $0B\n\
Reg $09 : $0B\nReg $0A : $0E\nReg $0B : $00\nReg $0C : $00\nReg $0D : $00\n\
Reg $0E : $25\nReg $0F : $00"
            .to_string();
        let mut fake = Fake {
            responses: vec![block],
            sent: vec![],
        };
        let regs = ym_registers(&mut fake).await.unwrap();
        assert_eq!(regs, probe_ym_regs());
        assert_eq!(fake.sent, vec!["info ym".to_string()]);
    }

    #[tokio::test]
    async fn mfp_registers_sends_info_mfp() {
        let block = "General Purpose Pins:    0x12\n\
Active Edge:             0x00\n\
Data Direction:          0x00\n\
Interrupt A Enable:      0x00\n\
Interrupt B Enable:      0x00\n\
Interrupt A Pending:     0x00\n\
Interrupt B Pending:     0x00\n\
Interrupt A In-Service:  0x00\n\
Interrupt B In-Service:  0x00\n\
Interrupt A Mask:        0x00\n\
Interrupt B Mask:        0x00\n\
Vector:                  0x40\n\
Timer A Control:         0x00\n\
Timer B Control:         0x00\n\
Timer C/D Control:       0x00\n\
Timer A Data:            0x00\n\
Timer B Data:            0x00\n\
Timer C Data:            0x00\n\
Timer D Data:            0x00"
            .to_string();
        let mut fake = Fake {
            responses: vec![block],
            sent: vec![],
        };
        let regs = mfp_registers(&mut fake).await.unwrap();
        assert_eq!(regs.gpip, 0x12);
        assert_eq!(regs.vr, 0x40);
        assert_eq!(fake.sent, vec!["info mfp".to_string()]);
    }

    #[tokio::test]
    async fn write_bytes_formats_memwrite() {
        let mut fake = Fake {
            responses: vec!["ok".to_string()],
            sent: vec![],
        };
        write_bytes(&mut fake, Addr::new(0x1000), &[0xDE, 0xAD])
            .await
            .unwrap();
        assert_eq!(fake.sent, vec!["memwrite $1000 $de $ad".to_string()]);
    }

    #[tokio::test]
    async fn write_bytes_error_line_fails() {
        let mut fake = Fake {
            responses: vec!["ERROR: bad address".to_string()],
            sent: vec![],
        };
        let err = write_bytes(&mut fake, Addr::new(0x1000), &[0xDE])
            .await
            .unwrap_err();
        assert!(matches!(err, crate::error::Error::Parse { .. }));
    }

    /// Lowercase "rror" (Hatari's real "too many arguments" rejection text,
    /// confirmed live) must still be caught — the exact case the pre-fix
    /// `.contains("ERROR")` check missed, turning a rejected write into a
    /// false `Ok(())`.
    #[tokio::test]
    async fn write_bytes_lowercase_error_line_fails() {
        let mut fake = Fake {
            responses: vec!["Error: too many arguments (currently up to 64 supported)".into()],
            sent: vec![],
        };
        let err = write_bytes(&mut fake, Addr::new(0x1000), &[0xDE])
            .await
            .unwrap_err();
        assert!(matches!(err, crate::error::Error::Parse { .. }));
    }

    /// A write above `MEMWRITE_CHUNK_BYTES` must become multiple `memwrite`
    /// lines, each within the token budget Hatari's `psArgs[64]` allows
    /// (confirmed live: 61 data bytes succeed, 62 unconditionally fail) —
    /// this is the actual bug fix under test.
    #[tokio::test]
    async fn write_bytes_chunks_above_line_limit() {
        let bytes = vec![0xABu8; MEMWRITE_CHUNK_BYTES * 2 + 5];
        let mut fake = Fake {
            responses: vec!["ok".into(), "ok".into(), "ok".into()],
            sent: vec![],
        };
        write_bytes(&mut fake, Addr::new(0x1000), &bytes)
            .await
            .unwrap();
        assert_eq!(fake.sent.len(), 3);
        for line in &fake.sent {
            let data_tokens = line.split(' ').count() - 2; // "memwrite" + addr
            assert!(
                data_tokens <= MEMWRITE_CHUNK_BYTES,
                "line exceeds the safe token budget: {line}"
            );
        }
        // Each chunk targets the next address after the previous one.
        assert!(fake.sent[0].starts_with("memwrite $1000 "));
        assert!(
            fake.sent[1].starts_with(&format!("memwrite ${:x} ", 0x1000 + MEMWRITE_CHUNK_BYTES))
        );
        assert!(fake.sent[2].starts_with(&format!(
            "memwrite ${:x} ",
            0x1000 + MEMWRITE_CHUNK_BYTES * 2
        )));
    }

    /// A single-chunk write (the common case) must still be exactly one
    /// `memwrite` line — no regression in the happy path.
    #[tokio::test]
    async fn write_bytes_single_chunk_stays_one_line() {
        let bytes = vec![0x11u8; MEMWRITE_CHUNK_BYTES];
        let mut fake = Fake {
            responses: vec!["ok".into()],
            sent: vec![],
        };
        write_bytes(&mut fake, Addr::new(0x2000), &bytes)
            .await
            .unwrap();
        assert_eq!(fake.sent.len(), 1);
    }
}
