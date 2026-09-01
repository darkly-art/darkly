//! Black and White — the registration over the shared black-and-white core
//! ([`crate::gpu::black_and_white`]), which owns the identity, param schema,
//! uniform packing and WGSL transform. This file wires those into one
//! [`ParamEffect`](crate::gpu::param_effect::ParamEffect) and declares how the
//! effect surfaces.
//!
//! The transform is a function of one texel's colour, so the source is read by
//! integer texel index and the bind group carries no sampler.

use std::sync::Arc;

use crate::gpu::black_and_white as bw;
use crate::gpu::effect::{
    create_effect_pipeline, Binding, EffectPipeline, EffectRegistration, COLOR_TARGETS,
};
use crate::gpu::param_effect::{ParamEffectKind, Resources};

const BINDINGS: &[Binding] = &[Binding::Texture, Binding::Uniform];

fn create_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    let shader = format!(
        "{}\n{}",
        bw::SHADER_LIB,
        include_str!("../../../shaders/effects/black_and_white.wgsl"),
    );
    create_effect_pipeline(
        device,
        format,
        "black-and-white",
        BINDINGS,
        &shader,
        "fs_black_and_white",
    )
}

fn kind() -> Arc<ParamEffectKind> {
    ParamEffectKind::new(
        bw::TYPE_ID,
        "black-and-white",
        bw::PARAMS,
        BINDINGS,
        Resources::Packed(|params| bytemuck::cast_slice(&bw::pack_uniform(params)).to_vec()),
    )
}

pub fn register() -> EffectRegistration {
    EffectRegistration {
        type_id: bw::TYPE_ID,
        display_name: bw::DISPLAY_NAME,
        category: "Filters",
        icon: "fa6-solid:droplet-slash",
        description: bw::DESCRIPTION,
        hotkey_action: "effectBlack_and_white",
        params: bw::PARAMS,
        preview: Some(bw::PREVIEW),
        preview_at: Some(bw::preview_params),
        targets: COLOR_TARGETS,
        create_pipeline,
        from_params: |params, shared| kind().instance(params, shared),
    }
}
