use std::collections::HashMap;
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;

pub use super::effect::{EffectCache, EffectPipeline};

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

    /// Create GPU resources for this veil instance.
    /// `ping_pong_views` are the screen-sized veil textures used for chaining.
    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ping_pong_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        viewport_width: u32,
        viewport_height: u32,
    ) -> EffectCache;

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
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    #[cfg(target_arch = "wasm32")]
    pub from_js: fn(JsValue, Arc<EffectPipeline>) -> Box<dyn Veil>,
}

/// Auto-discovered veil registry with lazy pipeline caching.
pub struct VeilRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    #[cfg(target_arch = "wasm32")]
    from_js: fn(JsValue, Arc<EffectPipeline>) -> Box<dyn Veil>,
    cached_pipeline: Option<Arc<EffectPipeline>>,
}

impl VeilRegistry {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        for reg in super::veils::registrations() {
            entries.insert(
                reg.type_id,
                RegistryEntry {
                    create_pipeline: reg.create_pipeline,
                    #[cfg(target_arch = "wasm32")]
                    from_js: reg.from_js,
                    cached_pipeline: None,
                },
            );
        }
        VeilRegistry { entries }
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

    /// Create a veil instance from a JS type string and params object.
    #[cfg(target_arch = "wasm32")]
    pub fn create_veil(
        &mut self,
        type_id: &str,
        js: JsValue,
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
        (entry.from_js)(js, pipeline)
    }
}
