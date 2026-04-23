use std::collections::HashMap;
use std::sync::Arc;

pub use super::effect::{EffectCache, EffectPipeline};
pub use super::params::{ParamDef, ParamValue};

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
    pub params: &'static [ParamDef],
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    pub from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Veil>,
}

/// Auto-discovered veil registry with lazy pipeline caching.
pub struct VeilRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    params: &'static [ParamDef],
    from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Veil>,
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
                    create_pipeline: reg.create_pipeline,
                    params: reg.params,
                    from_params: reg.from_params,
                    cached_pipeline: None,
                },
            );
        }
        VeilRegistry { entries }
    }

    /// Return all registered veil type IDs with their parameter definitions.
    pub fn types(&self) -> Vec<(&'static str, &'static [ParamDef])> {
        let mut types: Vec<_> = self.entries.iter().map(|(&id, e)| (id, e.params)).collect();
        types.sort_by_key(|(id, _)| *id);
        types
    }

    /// Get the static parameter definitions for a veil type.
    pub fn param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries.get(type_id).map(|e| e.params).unwrap_or(&[])
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
            .get_or_insert_with(|| Arc::new((entry.create_pipeline)(device, format)))
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
            .get_or_insert_with(|| Arc::new((entry.create_pipeline)(device, format)))
            .clone();
        (entry.from_params)(params, pipeline)
    }
}
