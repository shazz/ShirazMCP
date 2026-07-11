//! `symbols(action="load")` choreography helpers, split out of `symbols.rs`
//! for the file-size ceiling (P26 §7.2): source classification (prg / PRG
//! file / ASCII file), section-base determination (explicit / basepage
//! auto-detect), the Hatari wire send, and the pure listing-mirror build.
//! `symbols.rs` composes these into `finish_load`.

use std::path::{Path, PathBuf};

use rmcp::ErrorData as McpError;

use super::mem::scan::parse_addr;
use super::symbols_wire::{self, SymEntry};
use super::to_mcp;
use crate::debug::{basepage, symbols as dbg_symbols};
use crate::error::Error;
use crate::ripper::prg_symbols::DriError;
use crate::ripper::{prg_file, prg_symbols};
use crate::session::HatariProcess;

/// What `resolve_source` determined from `path` (or its absence).
pub(super) enum SourceKind {
    /// No path: `symbols prg`, the running GEMDOS-HD program's own table.
    Prg,
    /// path given, file starts with the GEMDOS `$601A` magic.
    PrgFile(PathBuf),
    /// path given, anything else — treated as an nm-style ASCII map.
    AsciiFile(PathBuf),
}

pub(super) async fn resolve_source(
    path: Option<&str>,
    text_offset_given: bool,
) -> Result<SourceKind, McpError> {
    let Some(raw) = path else {
        return Ok(SourceKind::Prg);
    };
    let canon = symbols_wire::validate_symbols_path(raw)?;
    let is_prg = is_prg_binary(&canon).await?;
    if is_prg && text_offset_given {
        return Err(McpError::invalid_params(
            "this file is a GEMDOS program — Hatari relocates its symbol table to the current \
             basepage; offsets apply to ASCII maps only",
            None,
        ));
    }
    Ok(if is_prg {
        SourceKind::PrgFile(canon)
    } else {
        SourceKind::AsciiFile(canon)
    })
}

async fn is_prg_binary(path: &Path) -> Result<bool, McpError> {
    use tokio::io::AsyncReadExt;
    let mut f = tokio::fs::File::open(path)
        .await
        .map_err(|e| to_mcp(Error::Io(e)))?;
    let mut buf = [0u8; 2];
    let n = f.read(&mut buf).await.map_err(|e| to_mcp(Error::Io(e)))?;
    Ok(n == 2 && buf == [0x60, 0x1A])
}

pub(super) fn explicit_bases(
    text_offset: Option<&str>,
    data_offset: Option<&str>,
    bss_offset: Option<&str>,
) -> Result<Option<[u32; 3]>, McpError> {
    let Some(t) = text_offset else {
        return Ok(None);
    };
    let text = parse_addr(t)?.raw();
    let data = data_offset.map_or(Ok(text), |d| parse_addr(d).map(|a| a.raw()))?;
    let bss = bss_offset.map_or(Ok(text), |b| parse_addr(b).map(|a| a.raw()))?;
    Ok(Some([text, data, bss]))
}

pub(super) struct BasesResult {
    pub(super) bases: [u32; 3],
    pub(super) base_source: &'static str,
    /// Non-fatal note for PRG sources whose basepage discovery failed —
    /// Hatari's own load still runs and its error wins if there truly is
    /// no basepage (§7.2 step 3).
    pub(super) mirror_note: Option<String>,
}

pub(super) async fn determine_bases(
    proc: &mut HatariProcess,
    explicit: Option<[u32; 3]>,
    is_ascii: bool,
) -> Result<BasesResult, McpError> {
    if let Some(bases) = explicit {
        return Ok(BasesResult {
            bases,
            base_source: "explicit",
            mirror_note: None,
        });
    }
    match basepage::current_basepage(proc, proc.config().ram_len()).await {
        Ok(bp) => Ok(BasesResult {
            bases: [bp.text_base.raw(), bp.data_base.raw(), bp.bss_base.raw()],
            base_source: if is_ascii { "basepage" } else { "hatari" },
            mirror_note: None,
        }),
        Err(e) if is_ascii => Err(no_basepage_hint(e)),
        Err(e) => Ok(BasesResult {
            bases: [0, 0, 0],
            base_source: "hatari",
            mirror_note: Some(format!(
                "could not determine section bases for the listing mirror ({e}); Hatari's own \
                 load error, if any, is authoritative"
            )),
        }),
    }
}

pub(super) fn no_basepage_hint(e: Error) -> McpError {
    if matches!(&e, Error::Session(m) if m.contains("no GEMDOS process")) {
        McpError::invalid_params(
            "no GEMDOS process is running — pass text_offset explicitly, or start the program \
             first",
            None,
        )
    } else {
        to_mcp(e)
    }
}

pub(super) async fn send_load(
    proc: &mut HatariProcess,
    source: &SourceKind,
    bases: [u32; 3],
) -> Result<dbg_symbols::SymbolsLoaded, McpError> {
    match source {
        SourceKind::Prg => dbg_symbols::load_prg(proc).await,
        SourceKind::PrgFile(p) => dbg_symbols::load_prg_file(proc, p).await,
        SourceKind::AsciiFile(p) => dbg_symbols::load_file(proc, p, bases).await,
    }
    .map_err(to_mcp)
}

/// Pure listing-mirror build result — never fails the tool call: a mirror
/// that couldn't be built degrades to `listable: false` + `note` (§7.2
/// step 5), since Hatari's own load already succeeded.
pub(super) struct MirrorBuild {
    pub(super) entries: Vec<SymEntry>,
    pub(super) skipped: usize,
    pub(super) listable: bool,
    pub(super) note: Option<String>,
}

fn unlistable(note: impl Into<String>) -> MirrorBuild {
    MirrorBuild {
        entries: Vec::new(),
        skipped: 0,
        listable: false,
        note: Some(note.into()),
    }
}

pub(super) async fn build_mirror(
    source: &SourceKind,
    loaded: &dbg_symbols::SymbolsLoaded,
    bases: [u32; 3],
) -> MirrorBuild {
    match source {
        SourceKind::Prg => mirror_from_prg_source_path(loaded, bases),
        SourceKind::PrgFile(path) => mirror_from_prg_file(path, bases),
        SourceKind::AsciiFile(path) => mirror_from_ascii(path, bases).await,
    }
}

fn mirror_from_prg_source_path(
    loaded: &dbg_symbols::SymbolsLoaded,
    bases: [u32; 3],
) -> MirrorBuild {
    let Some(path) = loaded.source_path.as_deref() else {
        return unlistable("no source path reported for the loaded program's symbol table");
    };
    mirror_from_prg_file(Path::new(path), bases)
}

fn mirror_from_prg_file(path: &Path, bases: [u32; 3]) -> MirrorBuild {
    let prg = match prg_file::read_prg_file(path) {
        Ok(p) => p,
        Err(e) => {
            return unlistable(format!(
                "could not re-read the program for the listing mirror: {e}"
            ))
        }
    };
    match prg_symbols::parse_dri(&prg, bases) {
        Ok((syms, skipped)) => MirrorBuild {
            entries: symbols_wire::from_prg_symbols(syms),
            skipped,
            listable: true,
            note: None,
        },
        Err(DriError::NotDri { .. }) => {
            unlistable("non-DRI symbol table (a.out/ELF): names resolve, listing unavailable")
        }
        Err(e) => unlistable(format!(
            "could not parse the DRI symbol table for listing: {e:?}"
        )),
    }
}

async fn mirror_from_ascii(path: &Path, bases: [u32; 3]) -> MirrorBuild {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(t) => t,
        Err(e) => return unlistable(format!("could not re-read {}: {e}", path.display())),
    };
    match symbols_wire::parse_ascii_map(&text, bases) {
        Ok((entries, skipped)) => MirrorBuild {
            entries,
            skipped,
            listable: true,
            note: None,
        },
        Err(msg) => unlistable(format!(
            "listing mirror parse mismatched Hatari's own load: {msg}"
        )),
    }
}

pub(super) fn source_label(source: &SourceKind, loaded: &dbg_symbols::SymbolsLoaded) -> String {
    match source {
        SourceKind::Prg => format!("prg:{}", loaded.source_path.as_deref().unwrap_or("?")),
        SourceKind::PrgFile(p) | SourceKind::AsciiFile(p) => format!("file:{}", p.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_bases_defaults_data_and_bss_to_text() {
        let bases = explicit_bases(Some("$70000"), None, None).unwrap().unwrap();
        assert_eq!(bases, [0x70000, 0x70000, 0x70000]);
    }

    #[test]
    fn explicit_bases_none_without_text_offset() {
        assert_eq!(explicit_bases(None, None, None).unwrap(), None);
    }

    #[test]
    fn explicit_bases_respects_explicit_data_and_bss() {
        let bases = explicit_bases(Some("$70000"), Some("$80000"), Some("$90000"))
            .unwrap()
            .unwrap();
        assert_eq!(bases, [0x70000, 0x80000, 0x90000]);
    }

    #[test]
    fn no_basepage_hint_maps_no_gemdos_process() {
        let e = Error::Session("no GEMDOS process is running (p_run holds 0)".to_string());
        let mcp = no_basepage_hint(e);
        assert!(mcp.message.contains("pass text_offset explicitly"));
    }

    #[test]
    fn no_basepage_hint_passes_other_errors_through() {
        let e = Error::Session("some other failure".to_string());
        let mcp = no_basepage_hint(e);
        assert!(!mcp.message.contains("pass text_offset"));
    }
}
