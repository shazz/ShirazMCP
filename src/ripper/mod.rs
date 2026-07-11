//! Plugin-based ripping engine (ADR-2). Pure over `MemImage`: no emulator,
//! no async — CPU-bound scanning stays synchronous and parallelizes with
//! rayon. Extension contract: add a format = one plugin file + one line in
//! `registry::default_registry()`.

pub mod deprotect;
pub mod disk;
pub mod extract;
pub mod finding;
pub mod image;
pub mod plugins;
pub mod prg_file;
pub mod prg_symbols;
pub mod registry;
pub mod repatch;
pub mod snapshot;

pub use deprotect::{Deprotect, NoopDeprotect, ProtectionInfo};
pub use extract::{extract, extract_png, ExtractFormat, Extracted};
pub use finding::{rank_and_cap, Finding, FindingId};
pub use registry::{default_registry, PluginRole, PluginSummary, Registry};

use crate::types::{MemImage, PluginId};

/// What category of artifact a plugin looks for. `Sprite` is reserved
/// (post-P7 backlog); `Boot` arrives with real plugins in P7; `Raw` is the
/// bucket for kind-less reports (e.g. protection detections, ADR-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RipKind {
    Picture,
    Music,
    Sprite,
    Packer,
    Boot,
    Raw,
}

/// A detector. Scans a memory image, returns zero or more findings; picture
/// plugins additionally render their bytes to PNG (`render_png`).
/// Object-safe, registered as `Box<dyn RipPlugin>` (ADR-2): dispatch cost is
/// negligible next to the scan itself, which monomorphizes internally.
pub trait RipPlugin: Send + Sync {
    /// Stable id, e.g. "degas-pi1". Also the `Finding.plugin` value.
    fn id(&self) -> PluginId;
    fn kind(&self) -> RipKind;
    /// One-line description shown by `list_ripper_plugins`.
    fn describe(&self) -> &'static str;
    /// Pure scan. Construct findings with `Finding::new` — the engine
    /// attaches source-buffer and provenance afterwards (§5.2/§5.4).
    fn scan(&self, mem: &MemImage) -> Vec<Finding>;

    /// Render this plugin's artifact (the exact bytes a `Finding` of this
    /// plugin covers) as encoded PNG bytes. Default `None` = "this plugin
    /// does not render". Implementations also return `None` for corrupt
    /// input (with a `tracing::warn`) — never an error, never a panic;
    /// symmetric with `Depacker::try_unpack`.
    fn render_png(&self, _bytes: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

/// A transformer. If `data` is packed by this scheme, return unpacked bytes;
/// the engine re-runs all detectors over the result (bounded depth, §5.4).
/// `None` means "not mine / corrupt" — never an error, never a panic.
pub trait Depacker: Send + Sync {
    fn id(&self) -> PluginId;
    fn describe(&self) -> &'static str;
    fn try_unpack(&self, data: &[u8]) -> Option<Vec<u8>>;

    /// Prefix-tolerant variant for the EXPLICIT depack path (`depack` tool,
    /// ADR-25): unpack a stream that starts at `data[0]` and may be
    /// followed by unrelated bytes; returns (unpacked, bytes consumed).
    /// The default delegates to `try_unpack` and attributes the whole
    /// buffer — right for magic-scanning schemes; headerless schemes
    /// override it. Same contract: None = not mine / corrupt, never a
    /// panic.
    fn try_unpack_prefix(&self, data: &[u8]) -> Option<(Vec<u8>, usize)> {
        self.try_unpack(data).map(|out| (out, data.len()))
    }
}

// Object-safety is contractual (ADR-2): this must keep compiling.
const _: fn(&dyn RipPlugin, &dyn Depacker, &dyn Deprotect) = |_, _, _| {};
