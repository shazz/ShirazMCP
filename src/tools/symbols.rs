//! `symbols`: register a label map (P26). Hatari is the sole name→address
//! authority (`resolve_addr`, ADR-45) — this tool's job is choreography:
//! send the right `symbols` wire command, auto-detect section bases from
//! the GEMDOS basepage when the agent doesn't supply them, and mirror the
//! same source file into a server-side store so `action=list` can browse
//! it without ever sending Hatari's own `symbols name|code|data` (the
//! pager hazard — see `debug/symbols.rs`'s module doc). The load
//! choreography itself (source classification, base determination, mirror
//! build) lives in `symbols_load.rs`, split out for the file-size ceiling.

use std::sync::PoisonError;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::symbols_load::{self, BasesResult, SourceKind};
use super::symbols_wire::{self, SymEntry, SymbolStore, LIST_DEFAULT, LIST_MAX};
use super::{to_mcp, wrong_state, HatariServer};
use crate::debug::{symbols as dbg_symbols, SymbolLoad};
use crate::types::Addr;

// Re-exported for `tests/symbols_live.rs` (see `tools/analyze.rs`'s
// re-export note) — `symbols_wire` itself stays `pub(crate)`.
pub use super::symbols_wire::{
    LoadedOut, SymKind, SymListKind, SymbolOut, SymbolsAction, SymbolsArgs, SymbolsOutput,
};

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = symbols_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "symbols",
        description = "Register a symbol map so profile/disasm/breakpoint accept label names \
(BlitSphere) instead of addresses. action=load with no path loads the running GEMDOS-HD \
program's own symbol table (vasm -Ftos embeds one unless -nosym) — Hatari relocates it to the \
real load address automatically. action=load with path loads an nm-style ASCII map (\"70000 T \
main\" lines) — the TEXT/DATA/BSS offsets are auto-detected from the GEMDOS basepage, or given \
explicitly via text_offset/data_offset/bss_offset — or a symbol-bearing .PRG file. action=list \
browses the loaded map server-side (find= substring filter, capped — narrow with find rather \
than paging); action=free unloads. Symbol names in expressions are [A-Za-z0-9_] (names with \
'.'/'-' load but cannot be referenced). Registers/variables (a0, pc, VBL, TEXT) shadow \
same-named symbols."
    )]
    pub async fn symbols(
        &self,
        Parameters(a): Parameters<SymbolsArgs>,
    ) -> Result<Json<SymbolsOutput>, McpError> {
        match a.action {
            SymbolsAction::Load => self.symbols_load(a).await,
            SymbolsAction::List => self.symbols_list(a).await,
            SymbolsAction::Free => self.symbols_free().await,
        }
        .map(Json)
    }
}

impl HatariServer {
    async fn symbols_load(&self, a: SymbolsArgs) -> Result<SymbolsOutput, McpError> {
        symbols_wire::validate_offset_args(&a)?;
        let explicit = symbols_load::explicit_bases(
            a.text_offset.as_deref(),
            a.data_offset.as_deref(),
            a.bss_offset.as_deref(),
        )?;
        let source =
            symbols_load::resolve_source(a.path.as_deref(), a.text_offset.is_some()).await?;
        let is_ascii = matches!(source, SourceKind::AsciiFile(_));
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        let br = symbols_load::determine_bases(proc, explicit, is_ascii).await?;
        let loaded = symbols_load::send_load(proc, &source, br.bases).await?;
        let state = name.to_string();
        drop(guard);
        Ok(self.finish_load(source, loaded, br, state).await)
    }

    async fn symbols_list(&self, a: SymbolsArgs) -> Result<SymbolsOutput, McpError> {
        let kind = a.kind.unwrap_or(SymListKind::All);
        let limit = a.limit.unwrap_or(LIST_DEFAULT).min(LIST_MAX);
        let offset = a.offset.unwrap_or(0);
        let state = self.session.lock().await.state_name().to_string();
        let store = self.symbols.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(store) = store.as_ref() else {
            return Err(McpError::invalid_request(
                "no symbol map registered — run symbols(action=\"load\") first; \
                 Hatari-autoloaded symbols are usable in expressions but not listable",
                None,
            ));
        };
        let (symbols, total) =
            symbols_wire::list_symbols(store, kind, a.find.as_deref(), limit, offset);
        Ok(SymbolsOutput {
            action: "list".to_string(),
            loaded: None,
            symbols: Some(symbols),
            total_matches: Some(total),
            state,
        })
    }

    async fn symbols_free(&self) -> Result<SymbolsOutput, McpError> {
        let mut guard = self.session.lock().await;
        let name = guard.state_name();
        let proc = guard
            .process_mut()
            .ok_or_else(|| wrong_state(name, "running or paused"))?;
        dbg_symbols::free(proc).await.map_err(to_mcp)?;
        let state = name.to_string();
        drop(guard);
        self.clear_symbol_store();
        Ok(SymbolsOutput {
            action: "free".to_string(),
            loaded: None,
            symbols: None,
            total_matches: None,
            state,
        })
    }

    async fn finish_load(
        &self,
        source: SourceKind,
        loaded: dbg_symbols::SymbolsLoaded,
        br: BasesResult,
        state: String,
    ) -> SymbolsOutput {
        if matches!(loaded.load, SymbolLoad::NoSymbols) {
            self.clear_symbol_store();
            let note = "program has no symbol table (build without -nosym, or supply an ASCII \
                         map via path)"
                .to_string();
            return load_output(&source, &loaded, &br, false, Some(note), state);
        }
        let mirror = symbols_load::build_mirror(&source, &loaded, br.bases).await;
        let note = mirror.note.or_else(|| br.mirror_note.clone());
        self.store_mirror(
            mirror.listable,
            &source,
            &loaded,
            &br,
            mirror.entries,
            mirror.skipped,
        );
        load_output(&source, &loaded, &br, mirror.listable, note, state)
    }

    fn store_mirror(
        &self,
        listable: bool,
        source: &SourceKind,
        loaded: &dbg_symbols::SymbolsLoaded,
        br: &BasesResult,
        entries: Vec<SymEntry>,
        skipped: usize,
    ) {
        let store = listable.then(|| SymbolStore {
            source: symbols_load::source_label(source, loaded),
            text_base: br.bases[0],
            data_base: br.bases[1],
            bss_base: br.bases[2],
            entries,
            skipped,
        });
        *self.symbols.lock().unwrap_or_else(PoisonError::into_inner) = store;
    }

    /// Cleared on `action=free` and on session `stop` (§7.5) — a stale
    /// mirror must not answer `list` once Hatari's own table is gone.
    pub(crate) fn clear_symbol_store(&self) {
        *self.symbols.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

fn load_output(
    source: &SourceKind,
    loaded: &dbg_symbols::SymbolsLoaded,
    br: &BasesResult,
    listable: bool,
    note: Option<String>,
    state: String,
) -> SymbolsOutput {
    let (total, text_symbols) = match loaded.load {
        SymbolLoad::Loaded { total, text } => (total, text),
        SymbolLoad::NoSymbols => (0, 0),
    };
    SymbolsOutput {
        action: "load".to_string(),
        loaded: Some(LoadedOut {
            source: symbols_load::source_label(source, loaded),
            total,
            text_symbols,
            text_base: Addr::new(br.bases[0]).to_string(),
            data_base: Addr::new(br.bases[1]).to_string(),
            bss_base: Addr::new(br.bases[2]).to_string(),
            base_source: br.base_source.to_string(),
            listable,
            note,
        }),
        symbols: None,
        total_matches: None,
        state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::HatariConfig;

    fn args(action: SymbolsAction) -> SymbolsArgs {
        SymbolsArgs {
            action,
            path: None,
            text_offset: None,
            data_offset: None,
            bss_offset: None,
            kind: None,
            find: None,
            limit: None,
            offset: None,
        }
    }

    /// §9: `symbols(list)` with an empty store is `invalid_request` with a
    /// load hint — works even on a Stopped session (no session gate).
    #[tokio::test]
    async fn list_with_empty_store_is_invalid_request() {
        let server = HatariServer::new(HatariConfig::default());
        let err = server
            .symbols(Parameters(args(SymbolsAction::List)))
            .await
            .err()
            .expect("expected an error");
        assert!(err.message.contains("symbols(action=\"load\")"));
    }

    /// §9: `symbols(load, text_offset w/o path)` is `invalid_params`.
    #[tokio::test]
    async fn load_text_offset_without_path_is_invalid_params() {
        let server = HatariServer::new(HatariConfig::default());
        let mut a = args(SymbolsAction::Load);
        a.text_offset = Some("$70000".to_string());
        let err = server
            .symbols(Parameters(a))
            .await
            .err()
            .expect("expected an error");
        assert!(err.message.contains("pass path"));
    }

    /// §9: `symbols(load, data_offset w/o text_offset)` is `invalid_params`.
    #[tokio::test]
    async fn load_data_offset_without_text_offset_is_invalid_params() {
        let server = HatariServer::new(HatariConfig::default());
        let mut a = args(SymbolsAction::Load);
        a.path = Some("/tmp/does-not-matter.sym".to_string());
        a.data_offset = Some("$1000".to_string());
        let err = server
            .symbols(Parameters(a))
            .await
            .err()
            .expect("expected an error");
        assert!(err.message.contains("require text_offset"));
    }
}
