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
    pub params: &'static [ParamDef],
    pub create_pipeline: fn(&wgpu::Device) -> Arc<dyn FilterEffect>,
}

/// Auto-discovered filter registry with lazy effect caching.
pub struct FilterPipelineRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    display_name: &'static str,
    icon: &'static str,
    params: &'static [ParamDef],
    create_pipeline: fn(&wgpu::Device) -> Arc<dyn FilterEffect>,
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
                    display_name: reg.display_name,
                    icon: reg.icon,
                    params: reg.params,
                    create_pipeline: reg.create_pipeline,
                    cached_pipeline: None,
                },
            );
        }
        FilterPipelineRegistry { entries }
    }

    /// All registered filter type IDs with their display names and icons,
    /// sorted by id for a stable menu order.
    pub fn types(&self) -> Vec<(&'static str, &'static str, &'static str)> {
        let mut types: Vec<_> = self
            .entries
            .iter()
            .map(|(&id, e)| (id, e.display_name, e.icon))
            .collect();
        types.sort_by_key(|(id, _, _)| *id);
        types
    }

    /// Parameter schema for a filter type, or an empty slice for an unknown
    /// type (or a parameter-free filter). Drives both the `filter_types()`
    /// protocol emission and JSON→`ParamValue` conversion when a layer is added.
    pub fn params(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries.get(type_id).map(|e| e.params).unwrap_or(&[])
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
            .map(|e| e.display_name)
            .unwrap_or("")
    }

    /// Iconify name for a filter type, falling back to the empty string when
    /// the type is unknown (callers substitute the generic layer-kind icon).
    pub fn icon(&self, type_id: &str) -> &'static str {
        self.entries.get(type_id).map(|e| e.icon).unwrap_or("")
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
                .get_or_insert_with(|| (entry.create_pipeline)(device))
                .clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every filter declares a well-formed, non-empty Iconify name, and no two
    /// filters share one — so each reads distinctly in the Colors menu and the
    /// Add Filter Layer picker. Guards against copy-pasting a `register()` and
    /// forgetting to change the icon.
    #[test]
    fn every_filter_has_a_unique_icon() {
        let registry = FilterPipelineRegistry::new();
        let mut seen = HashSet::new();
        for (type_id, _display, icon) in registry.types() {
            assert!(!icon.is_empty(), "filter '{type_id}' has no icon");
            assert!(
                icon.contains(':'),
                "filter '{type_id}' icon '{icon}' is not a `prefix:name` Iconify id"
            );
            assert!(
                seen.insert(icon),
                "filter '{type_id}' reuses icon '{icon}' — icons must be unique per filter"
            );
        }
        assert!(!seen.is_empty(), "no filters registered");
    }
}
