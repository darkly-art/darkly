use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

const PARAMS: &[ParamDef] = &[
    ParamDef::Int   { name: "kernel_size", min: 1, max: 12, default: 6 },
    ParamDef::Float { name: "sharpness",   min: 1.0, max: 18.0, default: 8.0 },
    ParamDef::Float { name: "hardness",    min: 1.0, max: 200.0, default: 100.0 },
];

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "kuwahara",
        params: PARAMS,
        create_pipeline: create_kuwahara_pipeline,
        from_params: |params, shared| {
            let kernel_size = match params.get(0) { Some(ParamValue::Int(v)) => *v, _ => 6 };
            let sharpness = match params.get(1) { Some(ParamValue::Float(v)) => *v, _ => 8.0 };
            let hardness = match params.get(2) { Some(ParamValue::Float(v)) => *v, _ => 100.0 };
            Box::new(Kuwahara::new(kernel_size, sharpness, hardness, shared))
        },
    }
}

/// GPU uniforms for the Kuwahara shader.
/// Layout must match the WGSL `Params` struct exactly.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct KuwaharaUniforms {
    kernel_size: i32,
    sharpness: f32,
    hardness: f32,
    _pad: f32,
    resolution_x: f32,
    resolution_y: f32,
}

#[derive(Clone, Debug)]
pub struct Kuwahara {
    pub kernel_size: i32,
    pub sharpness: f32,
    pub hardness: f32,
    shared: Arc<EffectPipeline>,
}

impl Kuwahara {
    pub fn new(kernel_size: i32, sharpness: f32, hardness: f32, shared: Arc<EffectPipeline>) -> Self {
        Kuwahara {
            kernel_size: kernel_size.max(1),
            sharpness,
            hardness,
            shared,
        }
    }
}

impl Veil for Kuwahara {
    fn type_id(&self) -> &'static str {
        "kuwahara"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Int(self.kernel_size),
            ParamValue::Float(self.sharpness),
            ParamValue::Float(self.hardness),
        ]
    }

    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ping_pong_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        viewport_width: u32,
        viewport_height: u32,
    ) -> EffectCache {
        let uniforms = KuwaharaUniforms {
            kernel_size: self.kernel_size,
            sharpness: self.sharpness,
            hardness: self.hardness,
            _pad: 0.0,
            resolution_x: viewport_width as f32,
            resolution_y: viewport_height as f32,
        };
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("kuwahara-uniforms"),
            size: std::mem::size_of::<KuwaharaUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let layout = &self.shared.bind_group_layout;
        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("kuwahara-bg-{i}")),
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
            label: Some("kuwahara"),
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

fn create_kuwahara_pipeline(
    device: &wgpu::Device,
    _format: wgpu::TextureFormat,
) -> EffectPipeline {
    let bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kuwahara-bgl"),
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
        label: Some("kuwahara-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("kuwahara-shader"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../../../../../shaders/veils/kuwahara.wgsl").into(),
        ),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("kuwahara-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_kuwahara"),
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
