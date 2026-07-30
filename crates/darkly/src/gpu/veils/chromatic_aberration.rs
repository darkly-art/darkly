//! Chromatic aberration veil — the whole-canvas post-process surface of the CA
//! effect. Directional reuse: the effect definition (schema, uniform packing,
//! GPU layout) is owned by the filter module
//! ([`crate::gpu::filters::chromatic_aberration`]); this veil imports it so the
//! three surfaces stay a single source of truth. Unlike the filter, a veil holds
//! its params opaquely and packs them at cache-build time.

use std::sync::Arc;

use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::filters::chromatic_aberration::{
    pack_uniform, GpuAberrationParams, DESCRIPTION, PARAMS,
};
use crate::gpu::veil::{ParamValue, Veil, VeilRegistration};

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "chromatic_aberration",
        display_name: "Chromatic Aberration",
        description: DESCRIPTION,
        params: PARAMS,
        create_pipeline,
        from_params: |params, shared| Box::new(ChromaticAberration::new(params.to_vec(), shared)),
    }
}

#[derive(Clone, Debug)]
pub struct ChromaticAberration {
    params: Vec<ParamValue>,
    shared: Arc<EffectPipeline>,
}

impl ChromaticAberration {
    pub fn new(params: Vec<ParamValue>, shared: Arc<EffectPipeline>) -> Self {
        ChromaticAberration { params, shared }
    }
}

impl Veil for ChromaticAberration {
    fn type_id(&self) -> &'static str {
        "chromatic_aberration"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        self.params.clone()
    }

    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ping_pong_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        _render_width: u32,
        _render_height: u32,
    ) -> EffectCache {
        let packed = pack_uniform(&self.params);
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chromatic-aberration-uniforms"),
            size: std::mem::size_of::<GpuAberrationParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&packed));

        let layout = &self.shared.bind_group_layout;
        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("chromatic-aberration-bg-{i}")),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&ping_pong_views[i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform_buf.as_entire_binding(),
                    },
                ],
            })
        });

        EffectCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: vec![bind_groups],
            aux_textures: vec![],
            aux_views: vec![],
            aux_pipelines: vec![],
        }
    }

    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        src_idx: usize,
        dst_view: &wgpu::TextureView,
    ) {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chromatic-aberration"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        rpass.set_pipeline(&self.shared.pipeline);
        rpass.set_bind_group(0, &cache.bind_groups[0][src_idx], &[]);
        rpass.draw(0..3, 0..1);
    }
}

fn create_pipeline(device: &wgpu::Device, _format: wgpu::TextureFormat) -> EffectPipeline {
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("chromatic-aberration-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("chromatic-aberration-pipeline-layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("chromatic-aberration-shader"),
        // Prepend the shared aberration lib — same pattern the filter uses.
        source: wgpu::ShaderSource::Wgsl(
            format!(
                "{}\n{}",
                include_str!("../../../shaders/lib/aberration.wgsl"),
                include_str!("../../../shaders/veils/chromatic_aberration.wgsl"),
            )
            .into(),
        ),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("chromatic-aberration-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_chromatic_aberration"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    EffectPipeline {
        pipeline,
        bind_group_layout,
    }
}
