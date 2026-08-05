//! Non-destructive filter-layer registry.
//!
//! A "filter" is a layer-tree adjustment node: it transforms the composite of
//! everything below it (`1 - color` for invert, a per-channel tone map for
//! curves). Each filter owns a [`FilterEffect`] — its render pipeline plus any
//! parameter-derived GPU resources (a curves LUT, say), held in a reused
//! [`EffectCache`](super::effect::EffectCache). Parameter-free filters like
//! invert are a trivial wrapper over the shared
//! [`MaskedFilterPipeline`](super::effect::MaskedFilterPipeline).
//!
//! Auto-discovered from `gpu/filters/*.rs` (each exports `pub fn register() ->
//! FilterPipelineRegistration`), with lazy per-type effect caching. New filters
//! slot in by dropping a file in the directory; nothing here is edited.
//!
//! `FilterEffect` is deliberately distinct from [`Veil`](super::veil::Veil):
//! veils are fullscreen viewport post-process passes over ping-pong buffers
//! driven by `VeilChain`; filters run over a group accumulator at a fixed tree
//! position. The two share the [`EffectCache`](super::effect::EffectCache) and
//! the [`ParamDef`]/[`ParamValue`](super::params::ParamValue) schema — where the
//! real reuse lives — not the invocation contract.

use std::collections::HashMap;
use std::sync::Arc;

use super::effect::EffectCache;
use super::params::{ParamDef, ParamValue};
use crate::catalog::{Catalog, CatalogEntry};

/// A filter's GPU realization: a render pipeline plus optional param-derived
/// resources built into an [`EffectCache`]. One instance is shared (Arc'd)
/// across every filter layer of the same type — the per-layer state (the built
/// LUT, the change-detection fingerprint) lives in the compositor's cache map,
/// not on the effect.
pub trait FilterEffect: Send + Sync {
    /// Build or refresh any parameter-derived GPU resources (e.g. a curves LUT)
    /// into `cache`. Called from the compositor's pre-compose ensure phase
    /// whenever a layer's params change — never in the render loop. A genuine
    /// no-op for parameter-free filters, so a transient empty `EffectCache`
    /// satisfies the destructive path.
    fn ensure(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &[ParamValue],
        cache: &mut EffectCache,
    );

    /// Run the filter over `src`, writing the result to `out` (same dims). With
    /// `mask` present, only mask-selected texels take the filtered value;
    /// elsewhere the original passes through. `format` selects the RGBA8 (layer)
    /// or R8 (mask) pipeline. `cache` holds the resources built by [`ensure`].
    ///
    /// [`ensure`]: FilterEffect::ensure
    fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        mask: Option<&wgpu::TextureView>,
        out: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        cache: &EffectCache,
    );
}

/// What each filter module returns from its `register()` function. `params`
/// declares the filter's schema (empty for parameter-free filters); the
/// `create_pipeline` factory builds the shared [`FilterEffect`].
pub struct FilterPipelineRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    /// Iconify name (e.g. `"fa6-solid:chart-line"`) shown wherever the filter
    /// surfaces — the Colors menu action and the filter-layer picker/tree row.
    pub icon: &'static str,
    /// One-sentence summary shown as a picker tooltip and folded into the
    /// Colors-menu action description, where the command palette's substring
    /// search indexes it — include the terms users would search for.
    pub description: &'static str,
    pub params: &'static [ParamDef],
    pub create_pipeline: fn(&wgpu::Device) -> Arc<dyn FilterEffect>,
}

/// Id of the catalog this registry projects into. Distinct from the
/// `layerFilters` catalog of `crate::document::filter`, which registers mask
/// and selection modifiers rather than colour adjustments.
pub const CATALOG_ID: &str = "filters";

impl FilterPipelineRegistration {
    pub fn catalog_entry(&self) -> CatalogEntry {
        CatalogEntry::new(self.type_id, self.display_name)
            .with_icon(self.icon)
            .with_description(self.description)
            .with_params(self.params)
    }
}

/// The filter catalog — every registered filter, sorted by `type_id`.
pub fn catalog() -> Catalog {
    Catalog::new(
        CATALOG_ID,
        "Filters",
        FilterPipelineRegistry::new()
            .types()
            .into_iter()
            .map(FilterPipelineRegistration::catalog_entry)
            .collect(),
    )
    .with_description("Colour adjustments applied to everything beneath them in the layer tree.")
}

/// Auto-discovered filter registry with lazy effect caching.
pub struct FilterPipelineRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    /// The full registration this entry was built from. All metadata accessors
    /// read straight off this, so a new `FilterPipelineRegistration` field is
    /// exposed without widening any tuple or touching the registry.
    reg: FilterPipelineRegistration,
    cached_pipeline: Option<Arc<dyn FilterEffect>>,
}

impl Default for FilterPipelineRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FilterPipelineRegistry {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        for reg in super::filters::registrations() {
            entries.insert(
                reg.type_id,
                RegistryEntry {
                    reg,
                    cached_pipeline: None,
                },
            );
        }
        FilterPipelineRegistry { entries }
    }

    /// Return every registered filter's full [`FilterPipelineRegistration`],
    /// sorted by `type_id` for a stable menu order. Callers read whatever
    /// fields they need off the registration — a new field is free here.
    pub fn types(&self) -> Vec<&FilterPipelineRegistration> {
        let mut types: Vec<&FilterPipelineRegistration> =
            self.entries.values().map(|e| &e.reg).collect();
        types.sort_by_key(|reg| reg.type_id);
        types
    }

    /// Parameter schema for a filter type, or an empty slice for an unknown
    /// type (or a parameter-free filter). Drives both the `filter_types()`
    /// protocol emission and JSON→`ParamValue` conversion when a layer is added.
    pub fn params(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries
            .get(type_id)
            .map(|e| e.reg.params)
            .unwrap_or(&[])
    }

    /// True when this registry knows the given `type_id`.
    pub fn has(&self, type_id: &str) -> bool {
        self.entries.contains_key(type_id)
    }

    /// Human-friendly display name for a filter type, falling back to the
    /// empty string when the type is unknown.
    pub fn display_name(&self, type_id: &str) -> &'static str {
        self.entries
            .get(type_id)
            .map(|e| e.reg.display_name)
            .unwrap_or("")
    }

    /// Iconify name for a filter type, falling back to the empty string when
    /// the type is unknown (callers substitute the generic layer-kind icon).
    pub fn icon(&self, type_id: &str) -> &'static str {
        self.entries.get(type_id).map(|e| e.reg.icon).unwrap_or("")
    }

    /// Get or create the shared effect for a filter type. Returns `None`
    /// for an unknown type rather than panicking — the caller (a protocol
    /// request carrying an arbitrary string) decides how to fail.
    pub fn pipeline(
        &mut self,
        type_id: &str,
        device: &wgpu::Device,
    ) -> Option<Arc<dyn FilterEffect>> {
        let entry = self.entries.get_mut(type_id)?;
        Some(
            entry
                .cached_pipeline
                .get_or_insert_with(|| (entry.reg.create_pipeline)(device))
                .clone(),
        )
    }
}

// Icon well-formedness and per-catalog uniqueness are asserted generically for
// every registry by `icons_are_wellformed_and_unique_within_a_catalog` in
// `crate::catalog`.
