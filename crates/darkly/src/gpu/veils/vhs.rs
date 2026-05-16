use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

const PARAMS: &[ParamDef] = &[
    ParamDef::Float {
        name: "speed",
        min: 0.0,
        max: 3.0,
        default: 0.5,
    },
    ParamDef::Float {
        name: "wobble",
        min: 0.0,
        max: 2.0,
        default: 1.0,
    },
    ParamDef::Float {
        name: "switching",
        min: 0.0,
        max: 2.0,
        default: 1.0,
    },
    ParamDef::Float {
        name: "bloom",
        min: 0.0,
        max: 2.0,
        default: 1.0,
    },
    ParamDef::Float {
        name: "ac_beat",
        min: 0.0,
        max: 2.0,
        default: 1.0,
    },
];

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "vhs",
        display_name: "VHS",
        params: PARAMS,
        create_pipeline: create_vhs_pipeline,
        from_params: |params, shared| {
            let speed = match params.first() {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.5,
            };
            let wobble = match params.get(1) {
                Some(ParamValue::Float(v)) => *v,
                _ => 1.0,
            };
            let switching = match params.get(2) {
                Some(ParamValue::Float(v)) => *v,
                _ => 1.0,
            };
            let bloom = match params.get(3) {
                Some(ParamValue::Float(v)) => *v,
                _ => 1.0,
            };
            let ac_beat = match params.get(4) {
                Some(ParamValue::Float(v)) => *v,
                _ => 1.0,
            };
            Box::new(Vhs::new(speed, wobble, switching, bloom, ac_beat, shared))
        },
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct VhsUniforms {
    time: f32,
    wobble: f32,
    switching: f32,
    bloom: f32,
    ac_beat: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

#[derive(Clone, Debug)]
pub struct Vhs {
    pub speed: f32,
    pub wobble: f32,
    pub switching: f32,
    pub bloom: f32,
    pub ac_beat: f32,
    /// Accumulated effective time (speed-scaled).
    time: f32,
    shared: Arc<EffectPipeline>,
}

impl Vhs {
    pub fn new(
        speed: f32,
        wobble: f32,
        switching: f32,
        bloom: f32,
        ac_beat: f32,
        shared: Arc<EffectPipeline>,
    ) -> Self {
        Vhs {
            speed,
            wobble,
            switching,
            bloom,
            ac_beat,
            time: 0.0,
            shared,
        }
    }

    fn uniforms(&self) -> VhsUniforms {
        VhsUniforms {
            time: self.time,
            wobble: self.wobble,
            switching: self.switching,
            bloom: self.bloom,
            ac_beat: self.ac_beat,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        }
    }
}

impl Veil for Vhs {
    fn type_id(&self) -> &'static str {
        "vhs"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Float(self.speed),
            ParamValue::Float(self.wobble),
            ParamValue::Float(self.switching),
            ParamValue::Float(self.bloom),
            ParamValue::Float(self.ac_beat),
        ]
    }

    fn needs_animation(&self) -> bool {
        self.speed > 0.0
    }

    fn update_time(&mut self, queue: &wgpu::Queue, cache: &EffectCache, dt: f32) {
        self.time += dt * self.speed;
        if let Some(buf) = cache.uniform_bufs.first() {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&self.uniforms()));
        }
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
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vhs-uniforms"),
            size: std::mem::size_of::<VhsUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&self.uniforms()));

        let layout = &self.shared.bind_group_layout;
        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("vhs-bg-{i}")),
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
            label: Some("vhs"),
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

fn create_vhs_pipeline(device: &wgpu::Device, _format: wgpu::TextureFormat) -> EffectPipeline {
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vhs-bgl"),
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
        label: Some("vhs-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vhs-shader"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../../../../../shaders/veils/vhs.wgsl").into(),
        ),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("vhs-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_vhs"),
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
