use crate::error::{Error, Result};
use crate::types::{Offset, PluginId};

/// What a deprotector recognized. `name` is the protection scheme
/// ("Copylock series 1", …); offset/len bound the protected region within
/// the scanned buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectionInfo {
    pub name: &'static str,
    pub offset: Offset,
    pub len: usize,
}

/// Deprotection seam (ADR-6): interface only in this plan. The engine may
/// REPORT detections as findings; it never calls `strip` (§5.4 step D,
/// test-asserted §7.2). Real unprotectors (dec0de heritage —
/// refs/orionfuzion/dec0de/src/dec0de.c, background only, do not port) are
/// post-P7 backlog and will register here as one file + one line.
pub trait Deprotect: Send + Sync {
    fn id(&self) -> PluginId;
    fn describe(&self) -> &'static str;
    fn detect(&self, data: &[u8]) -> Option<ProtectionInfo>;
    fn strip(&self, data: &[u8]) -> Result<Vec<u8>>;
}

/// The single shipped implementation: detects nothing, strips nothing.
/// Exists so the seam is compile-tested and visible in
/// `list_ripper_plugins` from day one.
#[derive(Debug)]
pub struct NoopDeprotect;

impl Deprotect for NoopDeprotect {
    fn id(&self) -> PluginId {
        PluginId::new("noop-deprotect")
    }

    fn describe(&self) -> &'static str {
        "placeholder deprotector: detects nothing, strips nothing (ADR-6)"
    }

    fn detect(&self, _data: &[u8]) -> Option<ProtectionInfo> {
        None
    }

    fn strip(&self, _data: &[u8]) -> Result<Vec<u8>> {
        Err(Error::Ripper {
            plugin: self.id(),
            message: "no deprotectors implemented (ADR-6)".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_detects_nothing() {
        assert!(NoopDeprotect.detect(&[0u8; 64]).is_none());
    }

    #[test]
    fn noop_strip_is_err() {
        let e = NoopDeprotect.strip(&[]).unwrap_err();
        assert_eq!(
            e.to_string(),
            "ripper plugin 'noop-deprotect': no deprotectors implemented (ADR-6)"
        );
    }
}
