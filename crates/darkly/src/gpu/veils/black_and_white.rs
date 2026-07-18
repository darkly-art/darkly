//! Black and White veil — the viewport surface of the shared black-and-white
//! core ([`crate::gpu::black_and_white`]): a sampler-based fullscreen pass
//! over the veil chain's ping-pong textures. The identity, param schema,
//! uniform packing, and WGSL transform all live in the shared module; this
//! file owns only the veil-side bindings and render pass.

use crate::gpu::black_and_white as bw;
use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::veil::{ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: bw::TYPE_ID,
        display_name: bw::DISPLAY_NAME,
        description: bw::DESCRIPTION,
        params: bw::PARAMS,
        create_pipeline,
        from_params: |params, shared| Box::new(BlackAndWhite::new(params, shared)),
    }
}

#[derive(Clone, Debug)]
pub struct BlackAndWhite {
    /// Schema-ordered values, padded to the full schema with defaults on
    /// creation so `param_values()` always round-trips every slot.
    params: Vec<ParamValue>,
    shared: Arc<EffectPipeline>,
}

impl BlackAndWhite {
    pub fn new(params: &[ParamValue], shared: Arc<EffectPipeline>) -> Self {
        let params = bw::PARAMS
            .iter()
            .enumerate()
            .map(|(i, def)| {
                params
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| def.default_value())
            })
            .collect();
        BlackAndWhite { params, shared }
    }
}

impl Veil for BlackAndWhite {
    fn type_id(&self) -> &'static str {
        bw::TYPE_ID
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
        _viewport_width: u32,
        _viewport_height: u32,
    ) -> EffectCache {
        let uniforms = bw::pack_uniform(&self.params);
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("black-and-white-uniforms"),
            size: std::mem::size_of_val(&uniforms) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::cast_slice(&uniforms));

        let layout = &self.shared.bind_group_layout;
        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("black-and-white-bg-{i}")),
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
            label: Some("black-and-white"),
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
        label: Some("black-and-white-bgl"),
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
        label: Some("black-and-white-pipeline-layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("black-and-white-shader"),
        source: wgpu::ShaderSource::Wgsl(
            format!(
                "{}\n{}",
                bw::SHADER_LIB,
                include_str!("../../../shaders/veils/black_and_white.wgsl"),
            )
            .into(),
        ),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("black-and-white-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_black_and_white"),
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
