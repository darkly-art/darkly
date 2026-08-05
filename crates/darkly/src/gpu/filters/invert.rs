//! Invert-colors filter — `1 - rgb`, alpha preserved.
//!
//! A thin registration over the shared infrastructure: the `invert_color` atom
//! (`shaders/lib/color.wgsl`) supplies the math, the `MaskedFilterPipeline`
//! builder (`gpu/effect.rs`) supplies the plain+masked × RGBA8+R8 pipelines,
//! and `Compositor::filter_node_region` supplies the region plumbing. This
//! file is everything specific to invert.

use std::sync::Arc;

use crate::gpu::effect::MaskedFilterPipeline;
use crate::gpu::filter::{FilterEffect, FilterPipelineRegistration};
use crate::gpu::params::ConstParamValue;
use crate::gpu::preview::{ANIMATED_FRAMES, PREVIEW_FPS};
use crate::gpu::preview_recipe::{Key, PreviewRecipe, Track, TrackTarget};

/// Prepend the shared color atom to the invert shader so `fs_invert` /
/// `fs_invert_masked` can call `invert_color` — the same `include_str!`
/// concatenation `voids/noise.rs` uses for `lib/fbm.wgsl`.
fn shader_source() -> String {
    let color = include_str!("../../../shaders/lib/color.wgsl");
    let invert = include_str!("../../../shaders/filters/invert.wgsl");
    format!("{color}\n{invert}")
}

fn create_pipeline(device: &wgpu::Device) -> Arc<dyn FilterEffect> {
    Arc::new(MaskedFilterPipeline::new(
        device,
        "filter-invert",
        &shader_source(),
        "fs_invert",
        "fs_invert_masked",
    ))
}

/// The inversion fades in over the untouched image and back out. Invert takes
/// no parameters of its own, so the host layer's opacity is the whole of the
/// motion — and a crossfade between the original and its negative shows what the
/// filter does better than either end alone.
static PREVIEW: PreviewRecipe = PreviewRecipe {
    frames: ANIMATED_FRAMES,
    fps: PREVIEW_FPS,
    tracks: &[Track {
        target: TrackTarget::Layer("opacity"),
        keys: &[
            Key {
                t: 0.0,
                value: ConstParamValue::Float(0.0),
            },
            Key {
                t: 0.5,
                value: ConstParamValue::Float(1.0),
            },
            Key {
                t: 1.0,
                value: ConstParamValue::Float(0.0),
            },
        ],
    }],
};

pub fn register() -> FilterPipelineRegistration {
    FilterPipelineRegistration {
        type_id: "invert",
        display_name: "Invert Colors",
        icon: "fa6-solid:circle-half-stroke",
        description: "Invert every color channel for a photo-negative.",
        params: &[],
        preview: Some(&PREVIEW),
        create_pipeline,
    }
}
