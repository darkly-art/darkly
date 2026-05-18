use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

const PARAMS: &[ParamDef] = &[
    ParamDef::Float {
        name: "radius",
        min: 0.1,
        max: 1.0,
        default: 0.5,
    },
    ParamDef::Float {
        name: "threshold",
        min: 0.01,
        max: 1.0,
        default: 0.1,
    },
];

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "lens_blur",
        display_name: "Lens Blur",
        params: PARAMS,
        create_pipeline: create_lens_blur_pipeline,
        from_params: |params, shared| {
            let radius = match params.first() {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.5,
            };
            let threshold = match params.get(1) {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.1,
            };
            Box::new(LensBlur::new(radius, threshold, shared))
        },
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LensBlurUniforms {
    radius: f32,
    threshold: f32,
    resolution_x: f32,
    resolution_y: f32,
}

#[derive(Clone, Debug)]
pub struct LensBlur {
    pub radius: f32,
    pub threshold: f32,
    shared: Arc<EffectPipeline>,
}

impl LensBlur {
    pub fn new(radius: f32, threshold: f32, shared: Arc<EffectPipeline>) -> Self {
        LensBlur {
            radius: radius.max(0.1),
            threshold: threshold.max(0.01),
            shared,
        }
    }
}

impl Veil for LensBlur {
    fn type_id(&self) -> &'static str {
        "lens_blur"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Float(self.radius),
            ParamValue::Float(self.threshold),
        ]
    }

    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ping_pong_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        render_width: u32,
        render_height: u32,
    ) -> EffectCache {
        let uniforms = LensBlurUniforms {
            radius: self.radius,
            threshold: self.threshold,
            resolution_x: render_width as f32,
            resolution_y: render_height as f32,
        };
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lens-blur-uniforms"),
            size: std::mem::size_of::<LensBlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let layout = &self.shared.bind_group_layout;
        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("lens-blur-bg-{i}")),
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
            label: Some("lens-blur"),
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

fn create_lens_blur_pipeline(
    device: &wgpu::Device,
    _format: wgpu::TextureFormat,
) -> EffectPipeline {
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("lens-blur-bgl"),
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
        label: Some("lens-blur-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("lens-blur-shader"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../../../../../shaders/veils/lens_blur.wgsl").into(),
        ),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("lens-blur-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_lens_blur"),
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
