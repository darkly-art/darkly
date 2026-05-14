use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

const PARAMS: &[ParamDef] = &[
    ParamDef::Int {
        name: "iterations",
        min: 1,
        max: 50,
        default: 5,
    },
    ParamDef::Float {
        name: "wetness",
        min: 0.0,
        max: 2.0,
        default: 0.5,
    },
];

/// Size of the generated RGBA noise texture used as a flow map.
const NOISE_SIZE: u32 = 256;

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "watercolor",
        display_name: "Watercolor",
        params: PARAMS,
        create_pipeline: create_watercolor_pipeline,
        from_params: |params, shared| {
            let iterations = match params.first() {
                Some(ParamValue::Int(v)) => *v,
                _ => 20,
            };
            let wetness = match params.get(1) {
                Some(ParamValue::Float(v)) => *v,
                _ => 1.0,
            };
            Box::new(Watercolor::new(iterations, wetness, shared))
        },
    }
}

/// GPU uniforms for the watercolor shader.
/// `pass_type`: 0 = RGB→CMYK init, 1 = blur iteration, 2 = CMYK→RGB final.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct WatercolorUniforms {
    pass_type: i32,
    wetness: f32,
    resolution_x: f32,
    resolution_y: f32,
}

#[derive(Clone, Debug)]
pub struct Watercolor {
    pub iterations: i32,
    pub wetness: f32,
    shared: Arc<EffectPipeline>,
}

impl Watercolor {
    pub fn new(iterations: i32, wetness: f32, shared: Arc<EffectPipeline>) -> Self {
        Watercolor {
            iterations: iterations.max(1),
            wetness,
            shared,
        }
    }

    fn make_uniform_buf(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass_type: i32,
        width: u32,
        height: u32,
        label: &str,
    ) -> wgpu::Buffer {
        let uniforms = WatercolorUniforms {
            pass_type,
            wetness: self.wetness,
            resolution_x: width as f32,
            resolution_y: height as f32,
        };
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: std::mem::size_of::<WatercolorUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buf, 0, bytemuck::bytes_of(&uniforms));
        buf
    }
}

/// Generate a 256x256 RGBA noise texture. Uses a simple xorshift PRNG
/// seeded deterministically so every instance gets the same flow map.
fn generate_noise_data() -> Vec<u8> {
    let total = (NOISE_SIZE * NOISE_SIZE * 4) as usize;
    let mut data = Vec::with_capacity(total);
    let mut state: u32 = 0x12345678;
    for _ in 0..total {
        // xorshift32
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        data.push((state >> 24) as u8);
    }
    data
}

fn create_noise_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    let data = generate_noise_data();
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("watercolor-noise"),
        size: wgpu::Extent3d {
            width: NOISE_SIZE,
            height: NOISE_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(NOISE_SIZE * 4),
            rows_per_image: Some(NOISE_SIZE),
        },
        wgpu::Extent3d {
            width: NOISE_SIZE,
            height: NOISE_SIZE,
            depth_or_array_layers: 1,
        },
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

impl Veil for Watercolor {
    fn type_id(&self) -> &'static str {
        "watercolor"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Int(self.iterations),
            ParamValue::Float(self.wetness),
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
        let layout = &self.shared.bind_group_layout;

        // --- Uniform buffers for each pass type ---
        let init_ub = self.make_uniform_buf(
            device,
            queue,
            0,
            render_width,
            render_height,
            "watercolor-ub-init",
        );
        let blur_ub = self.make_uniform_buf(
            device,
            queue,
            1,
            render_width,
            render_height,
            "watercolor-ub-blur",
        );
        let final_ub = self.make_uniform_buf(
            device,
            queue,
            2,
            render_width,
            render_height,
            "watercolor-ub-final",
        );

        // --- Noise texture + repeat sampler for flow-map bias ---
        let (noise_tex, noise_view) = create_noise_texture(device, queue);
        let noise_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("watercolor-noise-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // --- Two aux textures for iterative ping-pong blur ---
        let tex_usage =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let mut aux_textures = vec![noise_tex];
        let mut aux_views = Vec::with_capacity(3);
        // aux_views[0..2] = ping-pong blur targets, aux_views[2] = noise
        for i in 0..2 {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("watercolor-aux-{i}")),
                size: wgpu::Extent3d {
                    width: render_width,
                    height: render_height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: tex_usage,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            aux_views.push(view);
            aux_textures.push(tex);
        }
        aux_views.push(noise_view);

        // Helper: build a bind group with the given input texture view + uniform buf.
        // All bind groups share the noise texture at binding 3.
        let make_bg = |label: &str, input_view: &wgpu::TextureView, ub: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: ub.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&aux_views[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&noise_sampler),
                    },
                ],
            })
        };

        // [0] = init: reads ping_pong[i] → writes aux_views[0]
        let init_bgs: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            make_bg(
                &format!("watercolor-init-{i}"),
                &ping_pong_views[i],
                &init_ub,
            )
        });

        // [1] = blur: [j] reads aux_views[j] (writes to aux_views[1-j])
        let blur_bgs: [wgpu::BindGroup; 2] = std::array::from_fn(|j| {
            make_bg(&format!("watercolor-blur-{j}"), &aux_views[j], &blur_ub)
        });

        // [2] = final: [j] reads aux_views[j] → writes to dst
        let final_bgs: [wgpu::BindGroup; 2] = std::array::from_fn(|j| {
            make_bg(&format!("watercolor-final-{j}"), &aux_views[j], &final_ub)
        });

        EffectCache {
            uniform_bufs: vec![init_ub, blur_ub, final_ub],
            bind_groups: vec![init_bgs, blur_bgs, final_bgs],
            aux_textures,
            aux_views,
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
        let pipeline = &self.shared.pipeline;
        let n = self.iterations as usize;

        // Pass 0: RGB → CMYK — ping_pong[src] → aux[0]
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("watercolor-init"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cache.aux_views[0],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, &cache.bind_groups[0][src_idx], &[]);
            rpass.draw(0..3, 0..1);
        }

        // Passes 1..N: blur iterations — aux[src] → aux[dst], ping-ponging.
        // Iteration i reads aux[i%2], writes to aux[(i+1)%2].
        for i in 0..n {
            let read_idx = i % 2;
            let write_idx = (i + 1) % 2;
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("watercolor-blur"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cache.aux_views[write_idx],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, &cache.bind_groups[1][read_idx], &[]);
            rpass.draw(0..3, 0..1);
        }

        // Final pass: CMYK → RGB — aux[last_written] → dst
        let last_written = n % 2;
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("watercolor-final"),
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
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, &cache.bind_groups[2][last_written], &[]);
            rpass.draw(0..3, 0..1);
        }
    }
}

fn create_watercolor_pipeline(
    device: &wgpu::Device,
    _format: wgpu::TextureFormat,
) -> EffectPipeline {
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("watercolor-bgl"),
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
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("watercolor-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("watercolor-shader"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../../../../../shaders/veils/watercolor.wgsl").into(),
        ),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("watercolor-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_watercolor"),
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
