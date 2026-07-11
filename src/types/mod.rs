//! `Offset`, `PluginId`, `MemImage`, split out from the pre-§3 `types.rs`
//! (`Addr` moved to `addr.rs` — same file-size cleanup, no behavior
//! change); this module re-exports `Addr` so `crate::types::Addr` is
//! unchanged for every external caller.

mod addr;

pub use addr::Addr;

/// A byte offset into a `MemImage` (or any buffer) — distinct from `Addr`
/// so "position in snapshot" and "bus address" cannot be confused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Offset(usize);

impl Offset {
    pub const fn new(raw: usize) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> usize {
        self.0
    }
}

/// Stable identifier of a ripper plugin ("degas-pi1", "pack-ice"). Static
/// because plugins are compiled in (ADR-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PluginId(&'static str);

impl PluginId {
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for PluginId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An acquired snapshot of ST RAM (≤ 4 MB). The ripper operates on this,
/// never on the live emulator (ADR-2). P0 ships the shape only; acquisition
/// arrives in P1 (`session/memory.rs`).
#[derive(Debug, Clone)]
pub struct MemImage {
    bytes: Vec<u8>,
    base: Addr,
}

impl MemImage {
    /// Snapshot of `bytes` as seen at bus address `base`.
    pub fn new(bytes: Vec<u8>, base: Addr) -> Self {
        Self { bytes, base }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn base(&self) -> Addr {
        self.base
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_raw_roundtrip() {
        assert_eq!(Offset::new(7).raw(), 7);
    }

    #[test]
    fn plugin_id_display_and_as_str() {
        let p = PluginId::new("degas-pi1");
        assert_eq!(p.to_string(), "degas-pi1");
        assert_eq!(p.as_str(), "degas-pi1");
    }

    #[test]
    fn mem_image_accessors() {
        let m = MemImage::new(vec![1, 2, 3], Addr::new(0x1000));
        assert_eq!(m.bytes(), [1, 2, 3]);
        assert_eq!(m.base(), Addr::new(0x1000));
        assert_eq!(m.len(), 3);
        assert!(!m.is_empty());
    }

    #[test]
    fn mem_image_empty() {
        let m = MemImage::new(vec![], Addr::new(0));
        assert_eq!(m.len(), 0);
        assert!(m.is_empty());
    }
}
