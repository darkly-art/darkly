//! Invert-colors filter — `1 - rgb`, alpha preserved.
//!
//! A thin registration over the shared infrastructure: the `invert_color` atom
//! (`shaders/lib/color.wgsl`) supplies the math, the `MaskedFilterPipeline`
//! builder (`gpu/effect.rs`) supplies the plain+masked × RGBA8+R8 pipelines,
//! and `Compositor::filter_node_region` supplies the region plumbing. This
//! file is everything specific to invert.

use crate::gpu::effect::MaskedFilterPipeline;
use crate::gpu::filter::FilterPipelineRegistration;

/// Prepend the shared color atom to the invert shader so `fs_invert` /
/// `fs_invert_masked` can call `invert_color` — the same `include_str!`
/// concatenation `voids/noise.rs` uses for `lib/fbm.wgsl`.
fn shader_source() -> String {
    let color = include_str!("../../../../../shaders/lib/color.wgsl");
    let invert = include_str!("../../../../../shaders/filters/invert.wgsl");
    format!("{color}\n{invert}")
}

fn create_pipeline(device: &wgpu::Device) -> MaskedFilterPipeline {
    MaskedFilterPipeline::new(
        device,
        "filter-invert",
        &shader_source(),
        "fs_invert",
        "fs_invert_masked",
    )
}

pub fn register() -> FilterPipelineRegistration {
    FilterPipelineRegistration {
        type_id: "invert",
        display_name: "Invert Colors",
        create_pipeline,
    }
}
