use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

const PARAMS: &[ParamDef] = &[
    ParamDef::Float {
        name: "evolution",
        min: 0.0,
        max: 1.0,
        default: 0.05,
    },
    ParamDef::Float {
        name: "color",
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
];

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "noise",
        params: PARAMS,
        create_pipeline: create_evolve_pipeline,
        from_params: |params, shared| {
            let evolution = match params.first() {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.0,
            };
            let color = match params.get(1) {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.0,
            };
            Box::new(Noise::new(evolution, color, shared))
        },
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct NoiseUniforms {
    seed: f32,
    color: f32,
    rate: f32,
    _pad: f32,
}

#[derive(Clone, Debug)]
pub struct Noise {
    /// Per-frame blend rate toward fresh noise. 0 = static, 1 = fully random each frame.
    pub evolution: f32,
    /// 0 = grayscale noise, 1 = full RGB noise.
    pub color: f32,
    /// Monotonic frame counter used as hash seed.
    frame_count: f32,
    /// Which noise texture to write next (ping-pong index).
    noise_idx: usize,
    shared: Arc<EffectPipeline>,
}

impl Noise {
    pub fn new(evolution: f32, color: f32, shared: Arc<EffectPipeline>) -> Self {
        Noise {
            evolution,
            color,
            frame_count: 0.0,
            noise_idx: 0,
            shared,
        }
    }
}

impl Veil for Noise {
    fn type_id(&self) -> &'static str {
        "noise"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Float(self.evolution),
            ParamValue::Float(self.color),
        ]
    }

    fn needs_animation(&self) -> bool {
        self.evolution > 0.0
    }

    fn update_time(&mut self, queue: &wgpu::Queue, cache: &EffectCache, _dt: f32) {
        self.frame_count += 1.0;
        self.noise_idx = 1 - self.noise_idx;
        let uniforms = NoiseUniforms {
            seed: self.frame_count,
            color: self.color,
            rate: self.evolution,
            _pad: 0.0,
        };
        if let Some(buf) = cache.uniform_bufs.first() {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&uniforms));
        }
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
        let uniforms = NoiseUniforms {
            seed: 0.0,
            color: self.color,
            rate: self.evolution,
            _pad: 0.0,
        };
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("noise-uniforms"),
            size: std::mem::size_of::<NoiseUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        // Two noise state textures for ping-pong evolution.
        let noise_textures: Vec<wgpu::Texture> = (0..2)
            .map(|i| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(&format!("noise-state-{i}")),
                    size: wgpu::Extent3d {
                        width: render_width,
                        height: render_height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            })
            .collect();

        let noise_views: Vec<wgpu::TextureView> = noise_textures
            .iter()
            .map(|t| t.create_view(&Default::default()))
            .collect();

        // Initialize both noise textures with identical random data so the first
        // frame produces correct output regardless of which texture is read.
        let pixel_count = (render_width * render_height) as usize;
        let mut noise_data = vec![0u8; pixel_count * 4];
        let mut s = 42u32;
        for pixel in noise_data.chunks_exact_mut(4) {
            s = pcg_hash(s);
            let r = (s >> 24) as u8;
            s = pcg_hash(s);
            let g = (s >> 24) as u8;
            s = pcg_hash(s);
            let b = (s >> 24) as u8;
            s = pcg_hash(s);
            let a = (s >> 24) as u8;
            pixel.copy_from_slice(&[r, g, b, a]);
        }
        for tex in &noise_textures {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &noise_data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(render_width * 4),
                    rows_per_image: Some(render_height),
                },
                wgpu::Extent3d {
                    width: render_width,
                    height: render_height,
                    depth_or_array_layers: 1,
                },
            );
        }

        // --- Evolve bind groups (3-binding layout from shared pipeline) ---
        let evolve_bgs: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("noise-evolve-bg-{i}")),
                layout: &self.shared.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&noise_views[i]),
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

        // --- Apply pipeline (4-binding layout: input + noise + sampler + uniform) ---
        let apply_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("noise-apply-bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let apply_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("noise-apply-pipeline-layout"),
                bind_group_layouts: &[&apply_bgl],
                immediate_size: 0,
            });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("noise-apply-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../../shaders/veils/noise.wgsl").into(),
            ),
        });

        let apply_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("noise-apply-pipeline"),
            layout: Some(&apply_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_apply"),
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

        // --- Apply bind groups: [noise_idx][input_ping_pong_idx] ---
        // Stored as bind_groups[1 + noise_idx][input_idx].
        let apply_bgs_noise0: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("noise-apply-bg-n0-i{i}")),
                layout: &apply_bgl,
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
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&noise_views[0]),
                    },
                ],
            })
        });

        let apply_bgs_noise1: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("noise-apply-bg-n1-i{i}")),
                layout: &apply_bgl,
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
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&noise_views[1]),
                    },
                ],
            })
        });

        EffectCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: vec![evolve_bgs, apply_bgs_noise0, apply_bgs_noise1],
            aux_textures: noise_textures,
            aux_views: noise_views,
            aux_pipelines: vec![apply_pipeline],
        }
    }

    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        src_idx: usize,
        dst_view: &wgpu::TextureView,
    ) {
        // Which noise texture the apply pass should read from.
        let apply_noise_idx = if self.evolution > 0.0 {
            let noise_write = self.noise_idx;
            let noise_read = 1 - noise_write;

            // Evolve pass: read noise[read], replace random pixels, write noise[write].
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("noise-evolve"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cache.aux_views[noise_write],
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
            rpass.set_bind_group(0, &cache.bind_groups[0][noise_read], &[]);
            rpass.draw(0..3, 0..1);
            drop(rpass);

            noise_write
        } else {
            // Static noise: skip evolve, read from the initialized texture.
            0
        };

        // Apply pass: overlay-blend noise onto scene image.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("noise-apply"),
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
            rpass.set_pipeline(&cache.aux_pipelines[0]);
            rpass.set_bind_group(0, &cache.bind_groups[1 + apply_noise_idx][src_idx], &[]);
            rpass.draw(0..3, 0..1);
        }
    }
}

/// CPU-side PCG hash matching the GPU version.
fn pcg_hash(n: u32) -> u32 {
    let mut h = n.wrapping_mul(747796405).wrapping_add(2891336453);
    h = ((h >> ((h >> 28) + 4)) ^ h).wrapping_mul(277803737);
    (h >> 22) ^ h
}

fn create_evolve_pipeline(device: &wgpu::Device, _format: wgpu::TextureFormat) -> EffectPipeline {
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("noise-evolve-bgl"),
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
        label: Some("noise-evolve-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("noise-shader"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../../../../../shaders/veils/noise.wgsl").into(),
        ),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("noise-evolve-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_evolve"),
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
