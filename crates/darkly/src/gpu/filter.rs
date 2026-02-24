use std::collections::HashMap;
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

/// What each filter module returns from its `register()` function.
/// Contains everything needed to create instances of that filter type.
pub struct FilterRegistration {
    pub type_id: &'static str,
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> FilterPipeline,
    #[cfg(target_arch = "wasm32")]
    pub from_js: fn(JsValue, Arc<FilterPipeline>) -> Box<dyn Filter>,
}

/// Auto-discovered filter registry with lazy pipeline caching.
/// Built from the generated `filters::registrations()` at construction time.
/// Pipelines are only created when a filter of that type is first used.
pub struct FilterRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> FilterPipeline,
    #[cfg(target_arch = "wasm32")]
    from_js: fn(JsValue, Arc<FilterPipeline>) -> Box<dyn Filter>,
    cached_pipeline: Option<Arc<FilterPipeline>>,
}

impl FilterRegistry {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        for reg in super::filters::registrations() {
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
        FilterRegistry { entries }
    }

    /// Get or create the shared pipeline for a filter type.
    pub fn pipeline(
        &mut self,
        type_id: &str,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Arc<FilterPipeline> {
        let entry = self
            .entries
            .get_mut(type_id)
            .unwrap_or_else(|| panic!("Unknown filter type: {type_id}"));
        entry
            .cached_pipeline
            .get_or_insert_with(|| Arc::new((entry.create_pipeline)(device, format)))
            .clone()
    }

    /// Create a filter instance from a JS type string and params object.
    #[cfg(target_arch = "wasm32")]
    pub fn create_filter(
        &mut self,
        type_id: &str,
        js: JsValue,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Box<dyn Filter> {
        let entry = self
            .entries
            .get_mut(type_id)
            .unwrap_or_else(|| panic!("Unknown filter type: {type_id}"));
        let pipeline = entry
            .cached_pipeline
            .get_or_insert_with(|| Arc::new((entry.create_pipeline)(device, format)))
            .clone();
        (entry.from_js)(js, pipeline)
    }
}
