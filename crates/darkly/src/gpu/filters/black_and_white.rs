//! Black and White filter — the layer/destructive surface of the shared
//! black-and-white core ([`crate::gpu::black_and_white`]). The identity,
//! param schema, uniform packing, and WGSL transform all live in the shared
//! module; like HSV this is the no-aux [`ParamFilter`] specialization
//! (`[src, uniform]`, plus the mask variant), so this file only wires the
//! shared pieces into the filter pipeline.

use std::sync::Arc;

use crate::gpu::black_and_white as bw;
use crate::gpu::effect::EffectCache;
use crate::gpu::filter::{FilterEffect, FilterPipelineRegistration};
use crate::gpu::param_filter::{ParamFilter, SrcSampling};
use crate::gpu::params::ParamValue;

/// Allocate (once) and refresh the params uniform — the [`ParamFilter`]
/// `prepare` half.
fn prepare(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    params: &[ParamValue],
    cache: &mut EffectCache,
) {
    if cache.uniform_bufs.is_empty() {
        cache
            .uniform_bufs
            .push(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("filter-black-and-white-uniform"),
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
    }
    queue.write_buffer(
        &cache.uniform_bufs[0],
        0,
        bytemuck::cast_slice(&bw::pack_uniform(params)),
    );
}

fn create_pipeline(device: &wgpu::Device) -> Arc<dyn FilterEffect> {
    Arc::new(ParamFilter::new(
        device,
        "filter-black-and-white",
        &format!(
            "{}\n{}",
            bw::SHADER_LIB,
            include_str!("../../../shaders/filters/black_and_white.wgsl"),
        ),
        "fs_black_and_white",
        "fs_black_and_white_masked",
        false, // no aux texture — packed uniform only
        SrcSampling::Load,
        prepare,
    ))
}

pub fn register() -> FilterPipelineRegistration {
    FilterPipelineRegistration {
        // A string literal rather than `bw::TYPE_ID`: the frontend's
        // preset_hotkey_ids test derives filter action ids by scanning this
        // directory's sources for `type_id: "…"`. Equality with the shared
        // const is pinned by `gpu::black_and_white`'s identity test.
        type_id: "black_and_white",
        display_name: bw::DISPLAY_NAME,
        description: bw::DESCRIPTION,
        icon: "fa6-solid:droplet-slash",
        params: bw::PARAMS,
        create_pipeline,
    }
}
