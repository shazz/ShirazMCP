//! Typed wrappers for Hatari's `symbols` debugger command family (P26,
//! §5): `symbols prg` / `symbols <path> [$t $d $b]` / `symbols <path>` /
//! `symbols free`. Mirrors `debug/breakpoint.rs`'s shape: generic over
//! `DebugChannel`, wire strings pinned in one `format!` each. Wire facts
//! cited from the Hatari `v2.4.1` git tag (`symbols.c`), NOT the
//! tattlemuss/hrdb fork checkout at /home/matt/projects/hatari HEAD (see
//! docs/plan/P26.md's Hatari-source-trap warning).
//!
//! `symbols name`/`symbols code`/`symbols data` (Hatari's own LISTING
//! command) are never sent from anywhere in this crate — `Symbols_Show`
//! (v2.4.1 symbols.c:649-695) pages every ~20 rows via `fgets(stdin)`,
//! which would either deadlock Hatari's main loop (Running: the fifo goes
//! unserviced until we write) or corrupt our own stdin control plane
//! (Paused: the pager eats the next sentinel line). Listing is served
//! entirely from the Rust-side mirror (`tools::symbols_wire::SymbolStore`).

use crate::debug::{parse, DebugChannel, SymbolLoad};
use crate::error::{Error, Result};

/// Outcome of a file/prg load incl. the host path Hatari read (from the
/// "Loaded … from '<path>'." line; None when no symbols were loaded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolsLoaded {
    pub load: SymbolLoad,
    pub source_path: Option<String>,
}

/// `symbols prg` — load the GEMDOS-HD program's own table.
pub async fn load_prg(chan: &mut impl DebugChannel) -> Result<SymbolsLoaded> {
    parse_load(&chan.debug("symbols prg").await?)
}

/// `symbols {path} $<t> $<d> $<b>` — ASCII map with explicit per-section
/// offsets (always all three; Hatari would default D/B to T otherwise —
/// v2.4.1 symbols.c:909 — and we never rely on that).
pub async fn load_file(
    chan: &mut impl DebugChannel,
    path: &std::path::Path,
    offsets: [u32; 3],
) -> Result<SymbolsLoaded> {
    let cmd = format!(
        "symbols {} ${:x} ${:x} ${:x}",
        path.display(),
        offsets[0],
        offsets[1],
        offsets[2]
    );
    parse_load(&chan.debug(&cmd).await?)
}

/// `symbols {path}` — PRG-binary file: Hatari relocates to the current
/// basepage itself; offsets are meaningless and never sent.
pub async fn load_prg_file(
    chan: &mut impl DebugChannel,
    path: &std::path::Path,
) -> Result<SymbolsLoaded> {
    parse_load(&chan.debug(&format!("symbols {}", path.display())).await?)
}

/// `symbols free` — v2.4.1 prints nothing on success: any line containing
/// "ERROR" (case-insensitive, the `write_bytes`/memwrite precedent) fails,
/// else Ok.
pub async fn free(chan: &mut impl DebugChannel) -> Result<()> {
    let out = chan.debug("symbols free").await?;
    match out
        .lines()
        .find(|l| l.to_ascii_uppercase().contains("ERROR"))
    {
        Some(line) => Err(Error::Parse {
            context: "symbols free".to_string(),
            input: line.to_string(),
        }),
        None => Ok(()),
    }
}

/// Reuses `parse::parse_symbols`'s own precedence (it already recognizes
/// "symbol table missing" as `SymbolLoad::NoSymbols` BEFORE any generic
/// error handling — ADR-8: a symbol-less program is normal, not an error,
/// even though Hatari reports it via an `ERROR:`-prefixed line, verified
/// live in `tests/fixtures/hatari_session.log`). Only on a genuine parse
/// failure do we pick the most specific `ERROR: …` line (§5's error table:
/// file missing, no basepage, TEXT size mismatch, no GEMDOS-HD program,
/// bad DRI table size…) as the actionable `input`.
fn parse_load(out: &str) -> Result<SymbolsLoaded> {
    match parse::parse_symbols(out) {
        Ok(load) => Ok(SymbolsLoaded {
            load,
            source_path: parse::parse_loaded_path(out),
        }),
        Err(_) => {
            let line = out.lines().find(|l| l.contains("ERROR")).unwrap_or(out);
            Err(Error::Parse {
                context: "symbols load".to_string(),
                input: line.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

    #[tokio::test]
    async fn load_prg_sends_wire_string_and_parses() {
        let mut fake = Fake {
            responses: vec!["Loaded 12 symbols (3 TEXT) from '/c/x.prg'.".to_string()],
            sent: vec![],
        };
        let out = load_prg(&mut fake).await.unwrap();
        assert_eq!(fake.sent, vec!["symbols prg".to_string()]);
        assert_eq!(out.load, SymbolLoad::Loaded { total: 12, text: 3 });
        assert_eq!(out.source_path.as_deref(), Some("/c/x.prg"));
    }

    #[tokio::test]
    async fn load_file_sends_offsets_wire_string() {
        let mut fake = Fake {
            responses: vec!["Loaded 2 symbols (1 TEXT) from '/a/b.sym'.".to_string()],
            sent: vec![],
        };
        load_file(
            &mut fake,
            Path::new("/a/b.sym"),
            [0x70000, 0x71000, 0x71200],
        )
        .await
        .unwrap();
        assert_eq!(
            fake.sent,
            vec!["symbols /a/b.sym $70000 $71000 $71200".to_string()]
        );
    }

    #[tokio::test]
    async fn load_prg_file_sends_bare_path() {
        let mut fake = Fake {
            responses: vec!["Loaded 2 symbols (1 TEXT) from '/a/b.prg'.".to_string()],
            sent: vec![],
        };
        load_prg_file(&mut fake, Path::new("/a/b.prg"))
            .await
            .unwrap();
        assert_eq!(fake.sent, vec!["symbols /a/b.prg".to_string()]);
    }

    /// The live-verified fixture shape (ADR-8): Hatari reports the
    /// no-symbols case via TWO `ERROR:`-prefixed lines, the first of which
    /// contains "symbol table missing" — must still degrade to `NoSymbols`,
    /// never a hard `Error::Parse`.
    #[tokio::test]
    async fn load_no_symbols_is_ok_not_error() {
        let mut fake = Fake {
            responses: vec![concat!(
                "Reading symbols from program 'x.prg' symbol table...\n",
                "TOS executable, DRI / GST symbol table, reloc=0, program flags: PRIVATE (0x0)\n",
                "ERROR: symbol table missing from the program!\n",
                "ERROR: reading symbols from 'x.prg' failed!"
            )
            .to_string()],
            sent: vec![],
        };
        let out = load_prg(&mut fake).await.unwrap();
        assert_eq!(out.load, SymbolLoad::NoSymbols);
        assert_eq!(out.source_path, None);
    }

    #[tokio::test]
    async fn load_error_line_surfaces_as_parse_error() {
        let mut fake = Fake {
            responses: vec!["ERROR: no valid program basepage!".to_string()],
            sent: vec![],
        };
        let err = load_prg(&mut fake).await.unwrap_err();
        match err {
            Error::Parse { context, input } => {
                assert_eq!(context, "symbols load");
                assert!(input.contains("no valid program basepage"));
            }
            other => panic!("expected Error::Parse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn free_empty_output_is_ok() {
        let mut fake = Fake {
            responses: vec![String::new()],
            sent: vec![],
        };
        free(&mut fake).await.unwrap();
        assert_eq!(fake.sent, vec!["symbols free".to_string()]);
    }

    #[tokio::test]
    async fn free_error_line_fails() {
        let mut fake = Fake {
            responses: vec!["ERROR: no symbols loaded".to_string()],
            sent: vec![],
        };
        let err = free(&mut fake).await.unwrap_err();
        assert!(matches!(err, Error::Parse { .. }));
    }
}
