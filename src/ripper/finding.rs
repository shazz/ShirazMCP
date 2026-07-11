use std::sync::Arc;

use crate::types::{Addr, MemImage, Offset, PluginId};

use super::RipKind;

/// A located artifact (ADR-5): a compact handle, never the bytes on the
/// wire. Plugins construct it with `new`; the engine attaches the source
/// buffer and the depack provenance chain before returning it (§5.4), so
/// `extract` can slice the exact bytes later without re-running depackers.
#[derive(Debug, Clone)]
pub struct Finding {
    pub plugin: PluginId,
    pub kind: RipKind,
    /// Byte offset into the buffer this finding was found in (the root RAM
    /// image when `via()` is empty, a depacked buffer otherwise).
    pub offset: Offset,
    pub len: usize,
    pub label: String,
    /// Structural confidence 0-100 (picture detectors, P21). None = unscored.
    pub confidence: Option<u8>,
    via: Vec<PluginId>,
    source: Option<Arc<MemImage>>,
}

impl Finding {
    /// What plugins call. `via` empty, `source` unattached.
    pub fn new(plugin: PluginId, kind: RipKind, offset: Offset, len: usize, label: String) -> Self {
        Self {
            plugin,
            kind,
            offset,
            len,
            label,
            confidence: None,
            via: Vec::new(),
            source: None,
        }
    }

    /// Attach a structure score (picture detectors). Chainable off `new`.
    pub fn with_confidence(mut self, confidence: u8) -> Self {
        self.confidence = Some(confidence);
        self
    }

    /// Depack provenance: `["pack-ice"]` = found inside one pack-ice unpack
    /// of the root image. Empty = found directly in RAM.
    pub fn via(&self) -> &[PluginId] {
        &self.via
    }

    /// Bus address of the artifact — only meaningful for root-image findings
    /// (a depacked buffer has no bus address). Some iff attached AND via is
    /// empty.
    pub fn addr(&self) -> Option<Addr> {
        match &self.source {
            Some(s) if self.via.is_empty() => {
                Some(Addr::new(s.base().raw() + self.offset.raw() as u32))
            }
            _ => None,
        }
    }

    /// The artifact bytes, sliced from the attached source buffer.
    /// None iff not yet attached (a raw plugin return that never went
    /// through `Registry::scan` — the engine always attaches or drops).
    pub fn bytes(&self) -> Option<&[u8]> {
        let s = self.source.as_ref()?;
        s.bytes()
            .get(self.offset.raw()..self.offset.raw() + self.len)
    }

    /// Engine and `Finding::whole_buffer` only. Attach source + provenance;
    /// returns false when offset+len does not fit the buffer (the engine
    /// drops such findings with a `tracing::warn` — a buggy plugin must not
    /// poison extract).
    pub(crate) fn attach(&mut self, source: &Arc<MemImage>, via: &[PluginId]) -> bool {
        if self
            .offset
            .raw()
            .checked_add(self.len)
            .is_none_or(|end| end > source.len())
        {
            return false;
        }
        self.source = Some(Arc::clone(source));
        self.via = via.to_vec();
        true
    }

    /// The entire `source` as one attached Raw finding — the `depack`
    /// tool's handle to a depacked blob (ADR-25). `plugin` names the
    /// depacker that produced the buffer; `via` gives it depack
    /// provenance, so `addr()` is None (a depacked buffer has no bus
    /// address).
    pub fn whole_buffer(
        source: &Arc<MemImage>,
        plugin: PluginId,
        via: &[PluginId],
        label: String,
    ) -> Self {
        let mut f = Self::new(plugin, RipKind::Raw, Offset::new(0), source.len(), label);
        let attached = f.attach(source, via);
        debug_assert!(attached, "0..len always fits its own buffer");
        f
    }
}

/// Order findings best-first (confidence desc — `None` sorts last via
/// `Option`'s Ord — then offset asc for determinism) and truncate to `cap`,
/// with a tracing::debug of the dropped count. Plugins call this at the end
/// of `scan` (ADR-37): ranking is per-plugin so the engine's registration-
/// order guarantee and FindingId stability are untouched.
pub fn rank_and_cap(mut findings: Vec<Finding>, cap: usize) -> Vec<Finding> {
    findings.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then(a.offset.raw().cmp(&b.offset.raw()))
    });
    if findings.len() > cap {
        tracing::debug!(dropped = findings.len() - cap, cap, "rank_and_cap: capped");
        findings.truncate(cap);
    }
    findings
}

/// Stable handle to one finding of one scan (ADR-5). Wire format
/// "<scan>:<index>", e.g. "3:0". The scan component makes staleness
/// detectable: ids from an older scan are rejected, never silently remapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindingId {
    scan: u64,
    index: u32,
}

impl FindingId {
    pub const fn new(scan: u64, index: u32) -> Self {
        Self { scan, index }
    }

    pub const fn scan(self) -> u64 {
        self.scan
    }

    pub const fn index(self) -> u32 {
        self.index
    }

    /// Parse "<scan>:<index>". Anything else →
    /// `Error::Parse { context: "finding_id", input: s.into() }`.
    pub fn parse(s: &str) -> crate::error::Result<Self> {
        let bad = || crate::error::Error::Parse {
            context: "finding_id".to_string(),
            input: s.to_string(),
        };
        let (scan, index) = s.split_once(':').ok_or_else(bad)?;
        let scan = scan.parse::<u64>().map_err(|_| bad())?;
        let index = index.parse::<u32>().map_err(|_| bad())?;
        Ok(Self { scan, index })
    }
}

impl std::fmt::Display for FindingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.scan, self.index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn finding_id_display_roundtrip() {
        let id = FindingId::new(3, 7);
        assert_eq!(id.to_string(), "3:7");
        assert_eq!(FindingId::parse("3:7").unwrap(), id);
    }

    #[test]
    fn finding_id_parse_rejects_garbage() {
        for input in ["x", "1:", ":2", "1:2:3", ""] {
            match FindingId::parse(input) {
                Err(Error::Parse { context, .. }) => assert_eq!(context, "finding_id"),
                other => panic!("expected Error::Parse for {input:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn new_finding_is_unattached() {
        let f = Finding::new(
            PluginId::new("toy"),
            RipKind::Picture,
            Offset::new(0),
            4,
            "x".to_string(),
        );
        assert!(f.via().is_empty());
        assert!(f.bytes().is_none());
        assert!(f.addr().is_none());
    }

    #[test]
    fn attach_slices_bytes_and_addr() {
        let img = Arc::new(MemImage::new((0..=15).collect(), Addr::new(0x1000)));
        let mut f = Finding::new(
            PluginId::new("toy"),
            RipKind::Picture,
            Offset::new(4),
            4,
            "x".to_string(),
        );
        assert!(f.attach(&img, &[]));
        assert_eq!(f.bytes(), Some(&[4u8, 5, 6, 7][..]));
        assert_eq!(f.addr(), Some(Addr::new(0x1004)));
    }

    #[test]
    fn attach_with_via_clears_addr() {
        let img = Arc::new(MemImage::new((0..=15).collect(), Addr::new(0x1000)));
        let mut f = Finding::new(
            PluginId::new("toy"),
            RipKind::Picture,
            Offset::new(4),
            4,
            "x".to_string(),
        );
        let via = [PluginId::new("toy-pack")];
        assert!(f.attach(&img, &via));
        assert!(f.addr().is_none());
        assert_eq!(f.via(), via);
        assert!(f.bytes().is_some());
    }

    #[test]
    fn whole_buffer_is_a_raw_finding_with_via() {
        let img = Arc::new(MemImage::new((0..16).collect(), Addr::new(0x2000)));
        let via = [PluginId::new("zx0")];
        let f = Finding::whole_buffer(&img, PluginId::new("zx0"), &via, "blob".to_string());
        assert_eq!(f.bytes(), Some(img.bytes()));
        assert!(f.addr().is_none());
        assert_eq!(f.via(), via);
        assert_eq!(f.kind, RipKind::Raw);
        assert_eq!(f.len, 16);
    }

    #[test]
    fn attach_rejects_out_of_bounds() {
        let img = Arc::new(MemImage::new(vec![0; 16], Addr::new(0)));
        let mut f = Finding::new(
            PluginId::new("toy"),
            RipKind::Picture,
            Offset::new(12),
            8,
            "x".to_string(),
        );
        assert!(!f.attach(&img, &[]));
    }

    #[test]
    fn with_confidence_roundtrip() {
        let f = Finding::new(
            PluginId::new("toy"),
            RipKind::Picture,
            Offset::new(0),
            4,
            "x".to_string(),
        );
        assert_eq!(f.confidence, None);
        assert_eq!(f.with_confidence(87).confidence, Some(87));
    }

    #[test]
    fn rank_and_cap_orders_and_truncates() {
        let mk = |confidence: Option<u8>, offset: usize| {
            let f = Finding::new(
                PluginId::new("toy"),
                RipKind::Picture,
                Offset::new(offset),
                4,
                "x".to_string(),
            );
            match confidence {
                Some(c) => f.with_confidence(c),
                None => f,
            }
        };
        let findings = vec![
            mk(Some(10), 0),
            mk(None, 2),
            mk(Some(90), 8),
            mk(Some(90), 4),
        ];
        let ranked = rank_and_cap(findings, 2);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].offset, Offset::new(4));
        assert_eq!(ranked[1].offset, Offset::new(8));
    }
}
