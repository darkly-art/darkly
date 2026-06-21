//! Destructive color-adjustment registry.
//!
//! An "adjustment" applies a `MaskedFilterPipeline` to a node's pixels in
//! place (`1 - color` for invert, more to come), respecting an active
//! selection and working on both raster layers (RGBA8) and masks (R8). It is
//! the destructive cousin of a veil: a veil is ephemeral viewport post-process
//! state, an adjustment bakes into the document and is undoable.
//!
//! Mirrors [`VeilRegistry`](super::veil::VeilRegistry)'s *registry shape* —
//! auto-discovered from `gpu/adjustments/*.rs` (each exports `pub fn register()
//! -> AdjustmentRegistration`), with lazy per-type pipeline caching — but the
//! cached GPU object is the shared [`MaskedFilterPipeline`](super::effect::MaskedFilterPipeline),
//! not a bespoke pass. New adjustments slot in by dropping a file in the
//! directory; nothing here is edited.

use std::collections::HashMap;
use std::sync::Arc;

use super::effect::MaskedFilterPipeline;

/// What each adjustment module returns from its `register()` function. Unlike
/// a veil, an adjustment has no parameters and its pipeline holds every target
/// format, so `create_pipeline` needs only the device.
pub struct AdjustmentRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub create_pipeline: fn(&wgpu::Device) -> MaskedFilterPipeline,
}

/// Auto-discovered adjustment registry with lazy pipeline caching.
pub struct AdjustmentRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    display_name: &'static str,
    create_pipeline: fn(&wgpu::Device) -> MaskedFilterPipeline,
    cached_pipeline: Option<Arc<MaskedFilterPipeline>>,
}

impl Default for AdjustmentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AdjustmentRegistry {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        for reg in super::adjustments::registrations() {
            entries.insert(
                reg.type_id,
                RegistryEntry {
                    display_name: reg.display_name,
                    create_pipeline: reg.create_pipeline,
                    cached_pipeline: None,
                },
            );
        }
        AdjustmentRegistry { entries }
    }

    /// All registered adjustment type IDs with their display names, sorted by
    /// id for a stable menu order.
    pub fn types(&self) -> Vec<(&'static str, &'static str)> {
        let mut types: Vec<_> = self
            .entries
            .iter()
            .map(|(&id, e)| (id, e.display_name))
            .collect();
        types.sort_by_key(|(id, _)| *id);
        types
    }

    /// True when this registry knows the given `type_id`.
    pub fn has(&self, type_id: &str) -> bool {
        self.entries.contains_key(type_id)
    }

    /// Human-friendly display name for an adjustment type, falling back to the
    /// empty string when the type is unknown.
    pub fn display_name(&self, type_id: &str) -> &'static str {
        self.entries
            .get(type_id)
            .map(|e| e.display_name)
            .unwrap_or("")
    }

    /// Get or create the shared pipeline for an adjustment type. Returns `None`
    /// for an unknown type rather than panicking — the caller (a protocol
    /// request carrying an arbitrary string) decides how to fail.
    pub fn pipeline(
        &mut self,
        type_id: &str,
        device: &wgpu::Device,
    ) -> Option<Arc<MaskedFilterPipeline>> {
        let entry = self.entries.get_mut(type_id)?;
        Some(
            entry
                .cached_pipeline
                .get_or_insert_with(|| Arc::new((entry.create_pipeline)(device)))
                .clone(),
        )
    }
}
