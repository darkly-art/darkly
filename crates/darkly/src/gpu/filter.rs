use std::collections::HashMap;
use std::sync::Arc;

pub use super::effect::{EffectCache, EffectPipeline};
use super::params::{ParamDef, ParamValue};

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
    ) -> EffectCache;
}

/// What each filter module returns from its `register()` function.
/// Contains everything needed to create instances of that filter type.
pub struct FilterRegistration {
    pub type_id: &'static str,
    pub params: &'static [ParamDef],
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    pub from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Filter>,
}

/// Auto-discovered filter registry with lazy pipeline caching.
/// Built from the generated `filters::registrations()` at construction time.
/// Pipelines are only created when a filter of that type is first used.
pub struct FilterRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    params: &'static [ParamDef],
    from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Filter>,
    cached_pipeline: Option<Arc<EffectPipeline>>,
}

impl FilterRegistry {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        for reg in super::filters::registrations() {
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
        FilterRegistry { entries }
    }

    /// Get the static parameter definitions for a filter type.
    pub fn param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries
            .get(type_id)
            .map(|e| e.params)
            .unwrap_or(&[])
    }

    /// Get or create the shared pipeline for a filter type.
    pub fn pipeline(
        &mut self,
        type_id: &str,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Arc<EffectPipeline> {
        let entry = self
            .entries
            .get_mut(type_id)
            .unwrap_or_else(|| panic!("Unknown filter type: {type_id}"));
        entry
            .cached_pipeline
            .get_or_insert_with(|| Arc::new((entry.create_pipeline)(device, format)))
            .clone()
    }

    /// Create a filter instance from a type string and parameter values.
    pub fn create_filter(
        &mut self,
        type_id: &str,
        params: &[ParamValue],
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
        (entry.from_params)(params, pipeline)
    }
}
