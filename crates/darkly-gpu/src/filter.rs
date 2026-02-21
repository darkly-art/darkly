use darkly_core::layer::{FilterParams, FilterTypeId};
use std::collections::HashMap;
use wasm_bindgen::JsValue;

/// Cached GPU objects for a filter layer instance (P1).
/// Created once at layer creation, never in the render loop.
pub struct FilterLayerCache {
    /// One uniform buffer per pass.
    pub uniform_bufs: Vec<wgpu::Buffer>,
    /// One bind group per pass, per ping-pong direction.
    /// Indexed as bind_groups[pass_index][ping_pong_src].
    pub bind_groups: Vec<[wgpu::BindGroup; 2]>,
    /// Optional auxiliary textures (e.g., noise texture for noise filter).
    pub aux_textures: Vec<wgpu::Texture>,
    pub aux_views: Vec<wgpu::TextureView>,
}

/// Per-filter-type GPU resources + a factory for creating per-instance state.
/// Implemented by each filter module, registered once at init.
pub trait FilterHandler {
    /// Number of render passes this filter requires.
    fn pass_count(&self) -> u32;
    /// The pipeline for this filter's shader.
    fn pipeline(&self) -> &wgpu::RenderPipeline;
    /// The bind group layout for this filter's shader.
    fn bind_group_layout(&self) -> &wgpu::BindGroupLayout;
    /// Deserialize filter params from a JS object.
    fn create_params(&self, js: JsValue) -> Box<dyn FilterParams>;
    /// Create per-instance GPU state for a newly added filter layer.
    /// Called once at layer creation (P1).
    fn create_instance(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &dyn FilterParams,
        accum_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        canvas_width: u32,
        canvas_height: u32,
    ) -> FilterLayerCache;
}

/// Registry of all available filter pipelines.
/// Pure infrastructure — maps FilterTypeId to handlers, no per-instance state.
pub struct FilterRegistry {
    handlers: HashMap<FilterTypeId, Box<dyn FilterHandler>>,
}

impl FilterRegistry {
    pub fn new() -> Self {
        FilterRegistry {
            handlers: HashMap::new(),
        }
    }

    pub fn register(&mut self, id: FilterTypeId, handler: Box<dyn FilterHandler>) {
        self.handlers.insert(id, handler);
    }

    pub fn get(&self, id: &str) -> Option<&dyn FilterHandler> {
        self.handlers.get(id).map(|h| h.as_ref())
    }
}

impl Default for FilterRegistry {
    fn default() -> Self {
        Self::new()
    }
}
