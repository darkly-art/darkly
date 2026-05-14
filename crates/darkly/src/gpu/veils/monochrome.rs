use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

const PARAMS: &[ParamDef] = &[
    ParamDef::Float {
        name: "red_weight",
        min: 0.0,
        max: 1.0,
        default: 0.299,
    },
    ParamDef::Float {
        name: "green_weight",
        min: 0.0,
        max: 1.0,
        default: 0.587,
    },
    ParamDef::Float {
        name: "blue_weight",
        min: 0.0,
        max: 1.0,
        default: 0.114,
    },
    ParamDef::Float {
        name: "tint_hue",
        min: 0.0,
        max: 360.0,
        default: 0.0,
    },
    ParamDef::Float {
        name: "tint_strength",
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
];

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "monochrome",
        display_name: "Monochrome",
        params: PARAMS,
        create_pipeline: create_monochrome_pipeline,
        from_params: |params, shared| {
            let red_weight = match params.first() {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.299,
            };
            let green_weight = match params.get(1) {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.587,
            };
            let blue_weight = match params.get(2) {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.114,
            };
            let tint_hue = match params.get(3) {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.0,
            };
            let tint_strength = match params.get(4) {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.0,
            };
            Box::new(Monochrome::new(
                red_weight,
                green_weight,
                blue_weight,
                tint_hue,
                tint_strength,
                shared,
            ))
        },
    }
}

/// GPU uniforms for the monochrome shader.
/// Layout must match the WGSL `Params` struct exactly.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct MonochromeUniforms {
    red_weight: f32,
    green_weight: f32,
    blue_weight: f32,
    tint_hue: f32,
    tint_strength: f32,
    _pad1: f32,
    _pad2: f32,
    _pad3: f32,
}

#[derive(Clone, Debug)]
pub struct Monochrome {
    pub red_weight: f32,
    pub green_weight: f32,
    pub blue_weight: f32,
    pub tint_hue: f32,
    pub tint_strength: f32,
    shared: Arc<EffectPipeline>,
}

impl Monochrome {
    pub fn new(
        red_weight: f32,
        green_weight: f32,
        blue_weight: f32,
        tint_hue: f32,
        tint_strength: f32,
        shared: Arc<EffectPipeline>,
    ) -> Self {
        Monochrome {
            red_weight,
            green_weight,
            blue_weight,
            tint_hue,
            tint_strength,
            shared,
        }
    }
}

impl Veil for Monochrome {
    fn type_id(&self) -> &'static str {
        "monochrome"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Float(self.red_weight),
            ParamValue::Float(self.green_weight),
            ParamValue::Float(self.blue_weight),
            ParamValue::Float(self.tint_hue),
            ParamValue::Float(self.tint_strength),
        ]
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
        let uniforms = MonochromeUniforms {
            red_weight: self.red_weight,
            green_weight: self.green_weight,
            blue_weight: self.blue_weight,
            tint_hue: self.tint_hue,
            tint_strength: self.tint_strength,
            _pad1: 0.0,
            _pad2: 0.0,
            _pad3: 0.0,
        };
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("monochrome-uniforms"),
            size: std::mem::size_of::<MonochromeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let layout = &self.shared.bind_group_layout;
        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("monochrome-bg-{i}")),
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
            label: Some("monochrome"),
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

fn create_monochrome_pipeline(
    device: &wgpu::Device,
    _format: wgpu::TextureFormat,
) -> EffectPipeline {
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("monochrome-bgl"),
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
        label: Some("monochrome-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("monochrome-shader"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../../../../../shaders/veils/monochrome.wgsl").into(),
        ),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("monochrome-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_monochrome"),
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
