use std::sync::Arc;

use crate::types::{MemImage, PluginId};

use super::deprotect::{Deprotect, NoopDeprotect};
use super::finding::Finding;
use super::{Depacker, RipKind, RipPlugin};

// The recursive detect->attach->depack machinery lives in `scan.rs` (split
// for the 200-line budget, P4) — `scan_buffer` is the only item it exposes
// back to this module.
mod scan;

/// Depack-bomb guard: a depacked buffer larger than this is skipped with a
/// `tracing::warn` (ADR-2 consequence). 4× the biggest ST RAM, generous.
pub const MAX_UNPACKED: usize = 16 << 20;

/// Default recursion depth for depack→rescan (levels of unpacking).
pub const DEPTH_DEFAULT: u8 = 2;

pub struct Registry {
    detectors: Vec<Box<dyn RipPlugin>>,
    depackers: Vec<Box<dyn Depacker>>,
    deprotectors: Vec<Box<dyn Deprotect>>,
}

/// Discovery row for `list_ripper_plugins` (progressive disclosure, ADR-5).
#[derive(Debug, Clone, Copy)]
pub struct PluginSummary {
    pub id: PluginId,
    pub role: PluginRole,
    pub kind: Option<RipKind>,
    pub describe: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginRole {
    Detector,
    Depacker,
    Deprotect,
}

impl Registry {
    /// Empty registry — tests build custom ones from fixture plugins.
    pub fn new() -> Self {
        Self {
            detectors: Vec::new(),
            depackers: Vec::new(),
            deprotectors: Vec::new(),
        }
    }

    pub fn with_detector(mut self, p: Box<dyn RipPlugin>) -> Self {
        self.detectors.push(p);
        self
    }

    pub fn with_depacker(mut self, d: Box<dyn Depacker>) -> Self {
        self.depackers.push(d);
        self
    }

    pub fn with_deprotect(mut self, d: Box<dyn Deprotect>) -> Self {
        self.deprotectors.push(d);
        self
    }

    /// All registered plugins, in listing order: detectors, then depackers,
    /// then deprotectors, each in registration order.
    pub fn summaries(&self) -> Vec<PluginSummary> {
        let detectors = self.detectors.iter().map(|p| PluginSummary {
            id: p.id(),
            role: PluginRole::Detector,
            kind: Some(p.kind()),
            describe: p.describe(),
        });
        let depackers = self.depackers.iter().map(|d| PluginSummary {
            id: d.id(),
            role: PluginRole::Depacker,
            kind: None,
            describe: d.describe(),
        });
        let deprotectors = self.deprotectors.iter().map(|d| PluginSummary {
            id: d.id(),
            role: PluginRole::Deprotect,
            kind: None,
            describe: d.describe(),
        });
        detectors.chain(depackers).chain(deprotectors).collect()
    }

    /// Every plugin id (all three roles) — used by the tool layer to
    /// validate a `plugin` filter argument.
    pub fn plugin_ids(&self) -> Vec<PluginId> {
        self.summaries().into_iter().map(|s| s.id).collect()
    }

    /// Render a finding's bytes via the detector that produced it (ADR-12).
    /// None = unknown plugin id, non-rendering plugin, or corrupt bytes.
    pub fn render_png(&self, plugin: PluginId, bytes: &[u8]) -> Option<Vec<u8>> {
        self.detectors
            .iter()
            .find(|p| p.id() == plugin)?
            .render_png(bytes)
    }

    /// THE scan. `kind = None` scans everything. `depth` = remaining depack
    /// levels; 0 = detectors only. Ordering guarantee (what makes
    /// `FindingId` indices stable): per buffer — detectors in registration
    /// order, then deprotect reports, then depack-recursion findings
    /// depth-first in depacker registration order. Within one detector the
    /// order is plugin-defined (picture detectors return confidence-
    /// descending, capped — ADR-37).
    pub fn scan(&self, mem: Arc<MemImage>, kind: Option<RipKind>, depth: u8) -> Vec<Finding> {
        self.scan_buffer(mem, kind, depth, &[])
    }

    /// Run one depacker by id, prefix-tolerantly (the depack tool's seam,
    /// ADR-25). None = unknown id or the stream does not decode.
    pub fn unpack_prefix(&self, plugin: PluginId, data: &[u8]) -> Option<(Vec<u8>, usize)> {
        self.depackers
            .iter()
            .find(|d| d.id() == plugin)?
            .try_unpack_prefix(data)
    }

    /// `scan` with an explicit provenance prefix: findings carry `via`
    /// (hence no bus address). For buffers unpacked OUTSIDE the recursion
    /// engine — the depack tool (ADR-25).
    pub fn scan_via(
        &self,
        mem: Arc<MemImage>,
        kind: Option<RipKind>,
        depth: u8,
        via: &[PluginId],
    ) -> Vec<Finding> {
        self.scan_buffer(mem, kind, depth, via)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// THE single registration point (ADR-2 invariant: add a format = one plugin
/// file + one line HERE). P4 registered the two picture detectors; P5 adds
/// the Pack-Ice region detector and the first real depacker; P6 adds the two
/// music detectors (`sndh`, `ym-player`); the deprotect dummy remains the
/// only deprotector. P7 adds the boot-sector detector (`bootsector`). P9
/// adds three more packer pairs (atomik, powerpacker, rnc — each a
/// Packer-kind region detector + a depacker). P12 adds the first headerless
/// depacker (zx0 — explicit-path via the depack tool; the auto-engine only
/// unpacks whole-buffer-exact streams, ADR-25). A phase that edits anything
/// else in this module to add a plugin is an architecture bug — stop and
/// flag it. P13 adds the MPP picture detector (`mpp`).
pub fn default_registry() -> Registry {
    Registry::new()
        .with_detector(Box::new(super::plugins::degas::Degas))
        .with_detector(Box::new(super::plugins::neochrome::NeoChrome))
        .with_detector(Box::new(super::plugins::mpp::Mpp))
        .with_detector(Box::new(super::plugins::pack_ice::PackIceRegion))
        .with_detector(Box::new(super::plugins::sndh::Sndh))
        .with_detector(Box::new(super::plugins::ym_player::YmPlayer))
        .with_detector(Box::new(super::plugins::bootsector::BootSector))
        .with_detector(Box::new(super::plugins::atomik::AtomikRegion))
        .with_detector(Box::new(super::plugins::powerpacker::PowerPackerRegion))
        .with_detector(Box::new(super::plugins::rnc::RncRegion))
        .with_depacker(Box::new(super::plugins::pack_ice::PackIce))
        .with_depacker(Box::new(super::plugins::atomik::Atomik))
        .with_depacker(Box::new(super::plugins::powerpacker::PowerPacker))
        .with_depacker(Box::new(super::plugins::rnc::Rnc))
        .with_depacker(Box::new(super::plugins::zx0::Zx0))
        .with_deprotect(Box::new(NoopDeprotect))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Addr;

    #[test]
    fn empty_registry_scan_is_empty() {
        let mem = Arc::new(MemImage::new(vec![0; 4096], Addr::new(0)));
        assert!(Registry::new().scan(mem, None, 2).is_empty());
    }

    #[test]
    fn default_registry_summaries() {
        let summaries = default_registry().summaries();
        assert_eq!(summaries.len(), 16);
        assert_eq!(summaries[0].id, PluginId::new("degas"));
        assert_eq!(summaries[0].role, PluginRole::Detector);
        assert_eq!(summaries[0].kind, Some(RipKind::Picture));
        assert_eq!(summaries[1].id, PluginId::new("neochrome"));
        assert_eq!(summaries[1].role, PluginRole::Detector);
        assert_eq!(summaries[1].kind, Some(RipKind::Picture));
        assert_eq!(summaries[2].id, PluginId::new("mpp"));
        assert_eq!(summaries[2].role, PluginRole::Detector);
        assert_eq!(summaries[2].kind, Some(RipKind::Picture));
        assert_eq!(summaries[3].id, PluginId::new("pack-ice-region"));
        assert_eq!(summaries[3].role, PluginRole::Detector);
        assert_eq!(summaries[3].kind, Some(RipKind::Packer));
        assert_eq!(summaries[4].id, PluginId::new("sndh"));
        assert_eq!(summaries[4].role, PluginRole::Detector);
        assert_eq!(summaries[4].kind, Some(RipKind::Music));
        assert_eq!(summaries[5].id, PluginId::new("ym-player"));
        assert_eq!(summaries[5].role, PluginRole::Detector);
        assert_eq!(summaries[5].kind, Some(RipKind::Music));
        assert_eq!(summaries[6].id, PluginId::new("bootsector"));
        assert_eq!(summaries[6].role, PluginRole::Detector);
        assert_eq!(summaries[6].kind, Some(RipKind::Boot));
        assert_eq!(summaries[7].id, PluginId::new("atomik-region"));
        assert_eq!(summaries[7].role, PluginRole::Detector);
        assert_eq!(summaries[7].kind, Some(RipKind::Packer));
        assert_eq!(summaries[8].id, PluginId::new("powerpacker-region"));
        assert_eq!(summaries[8].role, PluginRole::Detector);
        assert_eq!(summaries[8].kind, Some(RipKind::Packer));
        assert_eq!(summaries[9].id, PluginId::new("rnc-region"));
        assert_eq!(summaries[9].role, PluginRole::Detector);
        assert_eq!(summaries[9].kind, Some(RipKind::Packer));
        assert_eq!(summaries[10].id, PluginId::new("pack-ice"));
        assert_eq!(summaries[10].role, PluginRole::Depacker);
        assert!(summaries[10].kind.is_none());
        assert_eq!(summaries[11].id, PluginId::new("atomik"));
        assert_eq!(summaries[11].role, PluginRole::Depacker);
        assert!(summaries[11].kind.is_none());
        assert_eq!(summaries[12].id, PluginId::new("powerpacker"));
        assert_eq!(summaries[12].role, PluginRole::Depacker);
        assert!(summaries[12].kind.is_none());
        assert_eq!(summaries[13].id, PluginId::new("rnc"));
        assert_eq!(summaries[13].role, PluginRole::Depacker);
        assert!(summaries[13].kind.is_none());
        assert_eq!(summaries[14].id, PluginId::new("zx0"));
        assert_eq!(summaries[14].role, PluginRole::Depacker);
        assert!(summaries[14].kind.is_none());
        assert_eq!(summaries[15].id, PluginId::new("noop-deprotect"));
        assert_eq!(summaries[15].role, PluginRole::Deprotect);
        assert!(summaries[15].kind.is_none());
    }

    #[test]
    fn render_png_unknown_plugin_is_none() {
        assert!(default_registry()
            .render_png(PluginId::new("nope"), &[])
            .is_none());
    }

    // §6.2's Z2 vector (5 bytes), duplicated locally per the spec.
    const Z2: &[u8] = &[0x87, 0x41, 0x55, 0x55, 0x80];

    #[test]
    fn unpack_prefix_happy_and_unknown_id() {
        let reg = default_registry();
        assert_eq!(
            reg.unpack_prefix(PluginId::new("zx0"), Z2),
            Some((b"AAAAAA".to_vec(), 5))
        );
        assert!(reg.unpack_prefix(PluginId::new("nope"), Z2).is_none());
    }

    struct Tagger;
    impl RipPlugin for Tagger {
        fn id(&self) -> PluginId {
            PluginId::new("tagger")
        }
        fn kind(&self) -> RipKind {
            RipKind::Raw
        }
        fn describe(&self) -> &'static str {
            "test-only tagger"
        }
        fn scan(&self, mem: &MemImage) -> Vec<Finding> {
            vec![Finding::new(
                self.id(),
                RipKind::Raw,
                crate::types::Offset::new(0),
                mem.len(),
                "tag".into(),
            )]
        }
    }

    #[test]
    fn scan_via_empty_registry_is_empty() {
        let mem = Arc::new(MemImage::new(vec![1, 2, 3, 4], Addr::new(0)));
        let via = [PluginId::new("zx0")];
        assert!(Registry::new().scan_via(mem, None, 2, &via).is_empty());
    }

    #[test]
    fn scan_via_threads_provenance_and_drops_addr() {
        let mem = Arc::new(MemImage::new(vec![1, 2, 3, 4], Addr::new(0x2000)));
        let via = [PluginId::new("zx0")];
        let findings = Registry::new()
            .with_detector(Box::new(Tagger))
            .scan_via(mem, None, 2, &via);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].via(), via);
        assert!(findings[0].addr().is_none());
    }
}
