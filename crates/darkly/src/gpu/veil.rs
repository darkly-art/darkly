use std::collections::HashMap;
use std::sync::Arc;

pub use super::effect::{EffectCache, EffectPipeline};
pub use super::params::{ParamDef, ParamValue};
use super::preview_recipe::{PreviewRecipe, PreviewSpec};
use crate::catalog::{Catalog, CatalogEntry};

/// Viewport-level post-processing effect ("veil").
/// Veils run on the fully-presented image at screen resolution,
/// after the view transform has been applied. They are ephemeral
/// editor state — they don't serialize with the document.
///
/// Unlike filters (which the compositor drives pass-by-pass),
/// veils get full control over their render passes via `encode()`.
/// This allows multi-resolution intermediate passes (e.g., downscale+upscale).
pub trait Veil: std::fmt::Debug {
    fn type_id(&self) -> &'static str;
    fn clone_boxed(&self) -> Box<dyn Veil>;

    /// Return the current parameter values, in the same order as the
    /// type's `ParamDef` array in `VeilRegistration`.
    fn param_values(&self) -> Vec<ParamValue>;

    /// Create GPU resources for this veil instance.
    /// `ping_pong_views` are the veil chain's render textures — veils read
    /// from and write to these at whatever resolution the chain provides.
    /// When `rendering.veil_scale` is below 1.0 the chain passes smaller
    /// textures automatically; veils never need to know about the distinction.
    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ping_pong_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        render_width: u32,
        render_height: u32,
    ) -> EffectCache;

    /// Per-veil resolution scale, applied on top of the global
    /// `rendering.veil_scale`. Veils whose per-pixel cost is too high at
    /// full viewport resolution override this to a value below 1.0; the
    /// chain renders the veil at the reduced resolution and bilinearly
    /// upscales the result. Effective scale is
    /// `global_scale * perf_scale_factor`. Default `1.0` (no extra scaling).
    fn perf_scale_factor(&self) -> f32 {
        1.0
    }

    /// Whether this veil uses time-based animation.
    /// When true (and speed > 0 and visible), the compositor drives
    /// continuous re-rendering via `needs_present`.
    fn needs_animation(&self) -> bool {
        false
    }

    /// Called each frame with the delta time (seconds since last frame).
    /// Animated veils should multiply `dt` by their speed param,
    /// accumulate into their internal time, and write to the uniform buffer.
    /// Default is a no-op for non-animated veils.
    fn update_time(&mut self, _queue: &wgpu::Queue, _cache: &EffectCache, _dt: f32) {}

    /// Encode all render passes into the command encoder.
    /// The veil reads from `ping_pong[src_idx]` (via pre-built bind groups)
    /// and must write its final output to `dst_view`.
    /// Internal intermediate passes (e.g., to aux textures) are the veil's concern.
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        src_idx: usize,
        dst_view: &wgpu::TextureView,
    );
}

/// What each veil module returns from its `register()` function.
pub struct VeilRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    /// One-sentence summary shown as a tooltip in the Add Veil picker —
    /// include the terms users would search for.
    pub description: &'static str,
    pub params: &'static [ParamDef],
    /// How this veil's documentation preview moves, or `None` for a veil with
    /// nothing worth showing. Declaring a recipe is what makes a veil
    /// previewable — the two facts are one.
    pub preview: Option<&'static PreviewRecipe>,
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    pub from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Veil>,
}

/// Id of the catalog this registry projects into.
pub const CATALOG_ID: &str = "veils";

/// Document properties a veil's preview may drive — none. Veils are previewed
/// offscreen, over a source texture rather than through a layer tree, so there
/// is no compositing layer whose properties a track could name.
pub const LAYER_KNOBS: &[ParamDef] = &[];

impl VeilRegistration {
    pub fn catalog_entry(&self) -> CatalogEntry {
        // Veils render a live preview in their picker, so no icon.
        CatalogEntry::new(self.type_id, self.display_name)
            .with_description(self.description)
            .with_params(self.params)
            .with_supports_preview(self.preview.is_some())
    }
}

/// The veil catalog — every registered veil, sorted by `type_id`.
pub fn catalog() -> Catalog {
    Catalog::new(
        CATALOG_ID,
        "Veils",
        VeilRegistry::new()
            .types()
            .into_iter()
            .map(VeilRegistration::catalog_entry)
            .collect(),
    )
    .with_description("Non-destructive effects stacked above a layer's pixels.")
}

/// Auto-discovered veil registry with lazy pipeline caching.
pub struct VeilRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    /// The full registration this entry was built from. All metadata accessors
    /// read straight off this, so a new `VeilRegistration` field is exposed
    /// without widening any tuple or touching the registry.
    reg: VeilRegistration,
    cached_pipeline: Option<Arc<EffectPipeline>>,
}

impl Default for VeilRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl VeilRegistry {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        for reg in super::veils::registrations() {
            entries.insert(
                reg.type_id,
                RegistryEntry {
                    reg,
                    cached_pipeline: None,
                },
            );
        }
        VeilRegistry { entries }
    }

    /// Return every registered veil's full [`VeilRegistration`], sorted by
    /// `type_id` for deterministic UI ordering. Callers read whatever fields
    /// they need off the registration — a new field is free here.
    pub fn types(&self) -> Vec<&VeilRegistration> {
        let mut types: Vec<&VeilRegistration> = self.entries.values().map(|e| &e.reg).collect();
        types.sort_by_key(|reg| reg.type_id);
        types
    }

    /// Get the static parameter definitions for a veil type.
    pub fn param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries
            .get(type_id)
            .map(|e| e.reg.params)
            .unwrap_or(&[])
    }

    /// Resolve a runtime `&str` type id to the registry's `&'static str` key,
    /// or `None` if the type is unknown. Callers that need to key long-lived
    /// state by type id (e.g. the veil preview cache + its readback context)
    /// use this to obtain a `'static` id without leaking.
    pub fn static_type_id(&self, type_id: &str) -> Option<&'static str> {
        self.entries.get_key_value(type_id).map(|(k, _)| *k)
    }

    /// True when this registry knows the given `type_id`. Used by the
    /// `.darkly` load pre-check to refuse files that name veils the
    /// binary doesn't ship — see [`crate::format::error::LoadError`].
    pub fn has(&self, type_id: &str) -> bool {
        self.entries.contains_key(type_id)
    }

    /// How a veil type's preview moves, paired with the knob namespace this
    /// catalog exposes to it. `None` for an unknown type or one that declares
    /// no recipe.
    pub fn preview(&self, type_id: &str) -> Option<PreviewSpec> {
        Some(PreviewSpec {
            recipe: self.entries.get(type_id)?.reg.preview?,
            layer_knobs: LAYER_KNOBS,
        })
    }

    /// Get the human-friendly display name for a veil type, falling back to
    /// the `type_id` literal when the type is unknown.
    pub fn display_name(&self, type_id: &str) -> &'static str {
        self.entries
            .get(type_id)
            .map(|e| e.reg.display_name)
            .unwrap_or("")
    }

    /// Get or create the shared pipeline for a veil type.
    pub fn pipeline(
        &mut self,
        type_id: &str,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Arc<EffectPipeline> {
        let entry = self
            .entries
            .get_mut(type_id)
            .unwrap_or_else(|| panic!("Unknown veil type: {type_id}"));
        entry
            .cached_pipeline
            .get_or_insert_with(|| Arc::new((entry.reg.create_pipeline)(device, format)))
            .clone()
    }

    /// Create a veil instance from a type string and parameter values.
    pub fn create_veil(
        &mut self,
        type_id: &str,
        params: &[ParamValue],
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Box<dyn Veil> {
        let entry = self
            .entries
            .get_mut(type_id)
            .unwrap_or_else(|| panic!("Unknown veil type: {type_id}"));
        let pipeline = entry
            .cached_pipeline
            .get_or_insert_with(|| Arc::new((entry.reg.create_pipeline)(device, format)))
            .clone();
        (entry.reg.from_params)(params, pipeline)
    }
}
