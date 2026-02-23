use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;

/// Cached GPU objects for a filter layer instance.
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

/// Shared GPU resources for a filter type (pipeline + bind group layout).
/// Arc-wrapped so multiple filter instances of the same type share them.
pub struct FilterPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
}

impl std::fmt::Debug for FilterPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterPipeline").finish_non_exhaustive()
    }
}

/// Unified trait for filters. Each filter is one struct that holds both its
/// user-facing parameters and an Arc to the shared GPU pipeline.
pub trait Filter: std::fmt::Debug {
    fn type_id(&self) -> &'static str;
    fn clone_boxed(&self) -> Box<dyn Filter>;
    fn pass_count(&self) -> u32;
    fn pipeline(&self) -> &wgpu::RenderPipeline;
    fn bind_group_layout(&self) -> &wgpu::BindGroupLayout;
    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        accum_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        canvas_width: u32,
        canvas_height: u32,
    ) -> FilterLayerCache;
}

/// Create a filter from a JS type string and params object.
/// This is the one dispatch point that maps strings to concrete types.
#[cfg(target_arch = "wasm32")]
pub fn create_filter(
    type_id: &str,
    js: JsValue,
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    pipelines: &mut FilterPipelines,
) -> Box<dyn Filter> {
    use super::filters::noise;
    match type_id {
        "noise" => Box::new(noise::Noise::from_js(js, pipelines.noise(device, format))),
        _ => panic!("Unknown filter type: {type_id}"),
    }
}

/// Lazily-initialized shared pipelines for all filter types.
/// Lives on the Compositor.
pub struct FilterPipelines {
    noise: Option<Arc<FilterPipeline>>,
}

impl FilterPipelines {
    pub fn new() -> Self {
        FilterPipelines { noise: None }
    }

    pub fn noise(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Arc<FilterPipeline> {
        self.noise
            .get_or_insert_with(|| {
                Arc::new(super::filters::noise::create_pipeline(device, format))
            })
            .clone()
    }
}
