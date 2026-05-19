//! Void layer effects — the procedural counterpart to [`Veil`].
//!
//! A void is a GPU effect that *generates* a layer's pixel content from a
//! shader (noise, screenshare, future portals), rather than storing static
//! pixels like a raster layer. Voids live inside the layer stack as a real
//! [`crate::layer::Layer::Void`] variant, participating in normal blending,
//! masking, and undo.
//!
//! The trait shape mirrors [`super::veil::Veil`] but the output contract is
//! different: a void [`Void::encode`] takes no ping-pong source view, because
//! a void has no upstream input — it writes the layer's color content into
//! `dst_view` from scratch. The compositor then composites the void's texture
//! through the normal raster blend pipeline, so opacity / blend mode / mask
//! work uniformly with raster layers.
//!
//! Adding a new void type is one new file under [`super::voids`]: the
//! module's `register()` returns a [`VoidRegistration`] with everything the
//! engine needs — display name, parameter schema, pipeline constructor, and
//! a factory that builds the trait object from a parameter slice.
//!
//! [`Veil`]: super::veil::Veil

use std::collections::HashMap;
use std::sync::Arc;

pub use super::effect::{EffectCache, EffectPipeline};
pub use super::params::{ParamDef, ParamValue};

/// Layer-level procedural-content effect ("void"). Renders the layer's
/// pixels from a shader instead of storing them.
///
/// Voids do not receive an upstream texture — they have no input. The
/// compositor allocates a per-void destination texture at canvas resolution
/// and the void writes its full output there in `encode()`. The compositor
/// then samples that texture through the existing blend pipeline, so every
/// raster-layer feature (blend modes, opacity, masks, group nesting) works
/// for voids without any per-kind branching.
pub trait Void: std::fmt::Debug {
    fn type_id(&self) -> &'static str;
    fn clone_boxed(&self) -> Box<dyn Void>;

    /// Return the current parameter values, in the same order as the
    /// type's [`ParamDef`] array in [`VoidRegistration`].
    fn param_values(&self) -> Vec<ParamValue>;

    /// Allocate per-instance GPU resources. The compositor passes the
    /// destination view (the void's own texture) so the void can build
    /// bind groups that target it directly — voids never sample from a
    /// ping-pong pair the way veils do.
    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dst_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        render_width: u32,
        render_height: u32,
    ) -> EffectCache;

    /// Whether this void uses time-based animation. When true (and visible),
    /// the compositor calls [`Self::update_time`] each frame the
    /// `animation.void_divisor` master clock fires, and keeps the canvas
    /// re-presenting.
    fn needs_animation(&self) -> bool {
        false
    }

    /// Per-frame uniform update for animated voids. Default is a no-op.
    fn update_time(&mut self, _queue: &wgpu::Queue, _cache: &EffectCache, _dt: f32) {}

    /// Render the void's content into `dst_view`. Called once per frame
    /// while the void is visible. Re-rendered eagerly on parameter changes
    /// via [`Self::needs_render`].
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        dst_view: &wgpu::TextureView,
    );
}

/// What each void module returns from its `register()` function.
pub struct VoidRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: &'static [ParamDef],
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    pub from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Void>,
}

/// Auto-discovered void registry with lazy pipeline caching. Modeled on
/// [`super::veil::VeilRegistry`]; the two could in principle share a
/// generic registry, but keeping them separate avoids stamping out an
/// extra layer of generics for the modest amount of code involved.
pub struct VoidRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    display_name: &'static str,
    create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    params: &'static [ParamDef],
    from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Void>,
    cached_pipeline: Option<Arc<EffectPipeline>>,
}

impl Default for VoidRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl VoidRegistry {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        for reg in super::voids::registrations() {
            entries.insert(
                reg.type_id,
                RegistryEntry {
                    display_name: reg.display_name,
                    create_pipeline: reg.create_pipeline,
                    params: reg.params,
                    from_params: reg.from_params,
                    cached_pipeline: None,
                },
            );
        }
        VoidRegistry { entries }
    }

    /// Return all registered void types with display name and parameter
    /// definitions. Sorted by `type_id` for deterministic UI ordering.
    pub fn types(&self) -> Vec<(&'static str, &'static str, &'static [ParamDef])> {
        let mut types: Vec<_> = self
            .entries
            .iter()
            .map(|(&id, e)| (id, e.display_name, e.params))
            .collect();
        types.sort_by_key(|(id, _, _)| *id);
        types
    }

    pub fn param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries.get(type_id).map(|e| e.params).unwrap_or(&[])
    }

    pub fn has(&self, type_id: &str) -> bool {
        self.entries.contains_key(type_id)
    }

    pub fn display_name(&self, type_id: &str) -> &'static str {
        self.entries
            .get(type_id)
            .map(|e| e.display_name)
            .unwrap_or("")
    }

    /// Get or create the shared pipeline for a void type. Pipelines are
    /// shared across all instances of the same type (Arc-wrapped) since
    /// the bind-group layout and shader are identical; only the per-
    /// instance uniform values differ.
    pub fn pipeline(
        &mut self,
        type_id: &str,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Arc<EffectPipeline> {
        let entry = self
            .entries
            .get_mut(type_id)
            .unwrap_or_else(|| panic!("Unknown void type: {type_id}"));
        entry
            .cached_pipeline
            .get_or_insert_with(|| Arc::new((entry.create_pipeline)(device, format)))
            .clone()
    }

    /// Create a void instance from a type string and parameter values.
    pub fn create_void(
        &mut self,
        type_id: &str,
        params: &[ParamValue],
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Box<dyn Void> {
        let entry = self
            .entries
            .get_mut(type_id)
            .unwrap_or_else(|| panic!("Unknown void type: {type_id}"));
        let pipeline = entry
            .cached_pipeline
            .get_or_insert_with(|| Arc::new((entry.create_pipeline)(device, format)))
            .clone();
        (entry.from_params)(params, pipeline)
    }
}
