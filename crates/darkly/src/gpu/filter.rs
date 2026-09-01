//! Non-destructive filter-layer registry.
//!
//! A "filter" is a layer-tree adjustment node: it transforms the composite of
//! everything below it (`1 - color` for invert, a per-channel tone map for
//! curves). Each filter owns a [`FilterEffect`]: its render pipeline plus any
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
//! the [`ParamDef`]/[`ParamValue`](super::params::ParamValue) schema (where the
//! real reuse lives), not the invocation contract.

use std::collections::HashMap;
use std::sync::Arc;

use super::effect::EffectCache;
use super::params::{ParamDef, ParamValue};
use super::preview::{
    PreviewAnim, PreviewEntry, PreviewMechanism, PreviewRegistries, PreviewSession, PreviewTarget,
    PREVIEW_FORMAT,
};
use crate::catalog::{Catalog, CatalogEntry};

/// A filter's GPU realization: a render pipeline plus optional param-derived
/// resources built into an [`EffectCache`]. One instance is shared (Arc'd)
/// across every filter layer of the same type: the per-layer state (the built
/// LUT, the change-detection fingerprint) lives in the compositor's cache map,
/// not on the effect.
pub trait FilterEffect: Send + Sync {
    /// Build or refresh any parameter-derived GPU resources (e.g. a curves LUT)
    /// into `cache`. Called from the compositor's pre-compose ensure phase
    /// whenever a layer's params change - never in the render loop. A genuine
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
    /// surfaces: the Colors menu action and the filter-layer picker/tree row.
    pub icon: &'static str,
    /// One-sentence summary shown as a picker tooltip and folded into the
    /// Colors-menu action description, where the command palette's substring
    /// search indexes it; include the terms artists would search for.
    pub description: &'static str,
    /// Id of the action that applies this filter to the active layer. Bindings
    /// in `presets/*.yaml` name this string; declaring it here rather than
    /// deriving it from `type_id` is what gives those bindings a compile-time
    /// target, the same way `ToolRegistration` does for tool selection.
    pub hotkey_action: &'static str,
    pub params: &'static [ParamDef],
    /// How long this filter's preview runs, or `None` for a filter with nothing
    /// worth showing. Declaring an animation is what makes a filter previewable:
    /// the two facts are one.
    pub preview: Option<PreviewAnim>,
    /// The parameter values this filter's preview shows at `t ∈ [0, 1]`, in
    /// `params` order.
    ///
    /// A function on the registration rather than a method on the effect,
    /// because a [`FilterEffect`] is shared across every filter layer of its
    /// type and holds no parameters of its own; they reach it through
    /// [`ensure`](FilterEffect::ensure), which is what this feeds. `None`
    /// (the default) is a still at the schema defaults, which is the honest answer
    /// for a filter with no parameters to sweep.
    pub preview_at: Option<fn(f32) -> Vec<ParamValue>>,
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
            .with_hotkey_action(self.hotkey_action)
            .with_params(self.params)
            .with_supports_preview(self.preview.is_some())
    }
}

/// The filter catalog: every registered filter, sorted by `type_id`.
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
    .with_description("Color adjustments applied to everything beneath them in the layer tree.")
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
    /// fields they need off the registration; a new field is free here.
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

    /// How long a filter type's preview runs. `None` for an unknown type or
    /// one that declares no preview.
    pub fn preview(&self, type_id: &str) -> Option<PreviewAnim> {
        self.entries.get(type_id)?.reg.preview
    }

    /// The parameter values a filter type's preview shows at `t`, in schema
    /// order. Falls back to the schema defaults for a filter that declares no
    /// sweep, so a caller never has to ask whether one exists.
    pub fn preview_params(&self, type_id: &str, t: f32) -> Vec<ParamValue> {
        let Some(entry) = self.entries.get(type_id) else {
            return Vec::new();
        };
        match entry.reg.preview_at {
            Some(at) => at(t),
            None => entry
                .reg
                .params
                .iter()
                .map(ParamDef::default_value)
                .collect(),
        }
    }

    /// Resolve a runtime `&str` type id to the registry's `&'static str` key,
    /// or `None` if the type is unknown. Callers keying long-lived state by
    /// type id (the preview cache + its readback context) use this to obtain a
    /// `'static` id without leaking. Mirrors `VeilRegistry::static_type_id`.
    pub fn static_type_id(&self, type_id: &str) -> Option<&'static str> {
        self.entries.get_key_value(type_id).map(|(k, _)| *k)
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
    /// for an unknown type rather than panicking; the caller (a protocol
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

// ---------------------------------------------------------------------------
// Preview mechanism
// ---------------------------------------------------------------------------

/// This catalog's answer to [`PreviewMechanism`]. Exported by name so
/// `build.rs` finds it while scanning this module's source and emits a
/// `preview_mechanisms()` row for `filters`.
pub fn preview_mechanism() -> &'static dyn PreviewMechanism {
    &FilterMechanism
}

struct FilterMechanism;

impl PreviewMechanism for FilterMechanism {
    fn resolve(&self, type_id: &str) -> Option<PreviewEntry> {
        let registry = FilterPipelineRegistry::new();
        Some(PreviewEntry {
            type_id: registry.static_type_id(type_id)?,
            anim: registry.preview(type_id)?,
        })
    }

    fn reads_source(&self) -> bool {
        true
    }

    fn open<'a>(
        &self,
        regs: PreviewRegistries<'a>,
        type_id: &str,
    ) -> Option<Box<dyn PreviewSession + 'a>> {
        let type_id = regs.filters.static_type_id(type_id)?;
        Some(Box::new(FilterSession {
            registry: regs.filters,
            type_id,
            effect: None,
            cache: EffectCache::empty(),
        }))
    }
}

/// One open filter preview.
///
/// Unlike a veil or a void there is no per-instance object to drive: a
/// [`FilterEffect`] is shared across every filter layer of its type and holds
/// no parameters, so each frame's values are computed on the registration and
/// pushed through [`ensure`](FilterEffect::ensure) into this session's own
/// cache. That is the same contract the compositor uses per layer.
struct FilterSession<'a> {
    registry: &'a mut FilterPipelineRegistry,
    type_id: &'static str,
    effect: Option<Arc<dyn FilterEffect>>,
    cache: EffectCache,
}

impl<'a> PreviewSession for FilterSession<'a> {
    fn set_t(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _target: &PreviewTarget,
        t: f32,
    ) {
        if self.effect.is_none() {
            self.effect = self.registry.pipeline(self.type_id, device);
        }
        let Some(effect) = self.effect.as_ref() else {
            return;
        };
        let params = self.registry.preview_params(self.type_id, t);
        effect.ensure(device, queue, &params, &mut self.cache);
    }

    fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &PreviewTarget,
    ) {
        let Some(effect) = self.effect.as_ref() else {
            return;
        };
        effect.render(
            device,
            encoder,
            target.source_view(),
            None,
            target.output_view(),
            PREVIEW_FORMAT,
            &self.cache,
        );
    }
}

// Icon well-formedness and per-catalog uniqueness are asserted generically for
// every registry by `icons_are_wellformed_and_unique_within_a_catalog` in
// `crate::catalog`.
