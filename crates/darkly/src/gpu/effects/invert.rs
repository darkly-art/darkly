//! Invert colors — `1 - rgb`, alpha preserved.
//!
//! A thin registration over shared infrastructure: the `invert_color` atom
//! (`shaders/lib/color.wgsl`) supplies the math and
//! [`ParamEffect`](crate::gpu::param_effect::ParamEffect) supplies the single
//! pass. This file is everything specific to invert.
//!
//! The one effect that declares [`MASK_TARGETS`]: inverting a single-channel
//! coverage mask is meaningful, so the registry will compile its pipeline
//! against R8 as well as RGBA8.

use std::sync::Arc;

use crate::gpu::effect::{
    create_effect_pipeline, Binding, EffectPipeline, EffectRegistration, MASK_TARGETS,
};
use crate::gpu::param_effect::{ParamEffectKind, Resources};
use crate::gpu::preview::PreviewAnim;

const BINDINGS: &[Binding] = &[Binding::Texture];

/// Prepend the shared color atom to the invert shader so `fs_invert` can call
/// `invert_color` — the same `include_str!` concatenation `voids/noise.rs` uses
/// for `lib/fbm.wgsl`.
fn shader_source() -> String {
    let color = include_str!("../../../shaders/lib/color.wgsl");
    let invert = include_str!("../../../shaders/effects/invert.wgsl");
    format!("{color}\n{invert}")
}

fn create_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    create_effect_pipeline(
        device,
        format,
        "invert",
        BINDINGS,
        &shader_source(),
        "fs_invert",
    )
}

fn kind() -> Arc<ParamEffectKind> {
    ParamEffectKind::new("invert", "invert", &[], BINDINGS, Resources::None)
}

pub fn register() -> EffectRegistration {
    EffectRegistration {
        type_id: "invert",
        display_name: "Invert Colors",
        category: "Filters",
        icon: "fa6-solid:circle-half-stroke",
        description: "Invert every color channel for a photo-negative.",
        hotkey_action: "effectInvert",
        params: &[],
        // Invert takes no parameters, so there is nothing to sweep and nothing
        // to declare: one frame of the effect fully applied, which is the whole
        // of what it does.
        preview: Some(PreviewAnim::STILL),
        preview_at: None,
        targets: MASK_TARGETS,
        create_pipeline,
        from_params: |params, shared| kind().instance(params, shared),
    }
}
