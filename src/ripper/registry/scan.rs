//! Scan engine internals (ADR-2): the recursive detect → attach → depack
//! machinery behind `Registry::scan`. Split out of registry.rs for the
//! 200-line budget (P4) — `scan_buffer` is `pub(super)`; `registry.rs`'s
//! public `scan()` is the only external entry point, so this split does not
//! touch the ADR-2 registration-invariant surface.

use std::sync::Arc;

use rayon::prelude::*;

use crate::ripper::finding::Finding;
use crate::ripper::RipKind;
use crate::types::{Addr, MemImage, PluginId};

use super::{Registry, MAX_UNPACKED};

impl Registry {
    pub(super) fn scan_buffer(
        &self,
        buf: Arc<MemImage>,
        kind: Option<RipKind>,
        depth: u8,
        via: &[PluginId],
    ) -> Vec<Finding> {
        let mut out = self.run_detectors(&buf, kind);
        if kind.is_none() {
            self.report_deprotect(&buf, &mut out);
        }
        out.retain_mut(|f| Self::attach_or_drop(f, &buf, via));
        if depth > 0 {
            out.extend(self.depack_recurse(&buf, kind, depth, via));
        }
        out
    }

    /// Step A: detectors, parallel (`rayon`'s ordered collect keeps order).
    fn run_detectors(&self, buf: &Arc<MemImage>, kind: Option<RipKind>) -> Vec<Finding> {
        self.detectors
            .par_iter()
            .filter(|p| kind.is_none_or(|k| p.kind() == k))
            .flat_map_iter(|p| p.scan(buf))
            .collect()
    }

    /// Step B: deprotect reporting, sequential — only when `kind.is_none()`
    /// (a kind-filtered scan asked for that kind; reports would be noise).
    fn report_deprotect(&self, buf: &MemImage, out: &mut Vec<Finding>) {
        for dp in &self.deprotectors {
            if let Some(info) = dp.detect(buf.bytes()) {
                out.push(Finding::new(
                    dp.id(),
                    RipKind::Raw,
                    info.offset,
                    info.len,
                    format!("protection detected: {}", info.name),
                ));
            }
        }
    }

    /// Step C: attach source + provenance, dropping anything out of bounds.
    fn attach_or_drop(f: &mut Finding, buf: &Arc<MemImage>, via: &[PluginId]) -> bool {
        let ok = f.attach(buf, via);
        if !ok {
            tracing::warn!(plugin = %f.plugin, "dropping out-of-bounds finding");
        }
        ok
    }

    /// Step D: depack→rescan, regardless of the kind filter.
    fn depack_recurse(
        &self,
        buf: &Arc<MemImage>,
        kind: Option<RipKind>,
        depth: u8,
        via: &[PluginId],
    ) -> Vec<Finding> {
        let mut out = Vec::new();
        for d in &self.depackers {
            let Some(bytes) = d.try_unpack(buf.bytes()) else {
                continue;
            };
            if bytes.len() > MAX_UNPACKED || bytes.is_empty() {
                tracing::warn!(
                    plugin = %d.id(),
                    len = bytes.len(),
                    "skipping depack bomb / degenerate output"
                );
                continue;
            }
            let child = Arc::new(MemImage::new(bytes, Addr::new(0)));
            let child_via: Vec<PluginId> = via.iter().copied().chain([d.id()]).collect();
            out.extend(self.scan_buffer(child, kind, depth - 1, &child_via));
        }
        out
    }
}
