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
    pub params: &'static [ParamDef],
    pub create_pipeline: fn(&wgpu::Device) -> Arc<dyn FilterEffect>,
}

/// Auto-discovered filter registry with lazy effect caching.
pub struct FilterPipelineRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    display_name: &'static str,
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
                    params: reg.params,
                    create_pipeline: reg.create_pipeline,
                    cached_pipeline: None,
                },
            );
        }
        FilterPipelineRegistry { entries }
    }

    /// All registered filter type IDs with their display names, sorted by
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
