use crate::gpu::effect::{create_blit_pipeline, EffectCache, EffectPipeline};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

/// Fixed computational budget in pixels. The internal render resolution
/// is derived from this so that landscape and portrait viewports produce
/// roughly equal GPU load. sqrt(130_000) ~ 360, so a square viewport
/// renders at ~360x360. A 16:9 landscape renders at ~480x270.
const PIXEL_BUDGET: f32 = 130_000.0;

const PARAMS: &[ParamDef] = &[
    ParamDef::Float { name: "speed",       min: 0.0, max: 3.0, default: 1.0 },
    ParamDef::Float { name: "rain_amount", min: 0.0, max: 1.0, default: 0.7 },
    ParamDef::Float { name: "direction",   min: 0.0, max: 360.0, default: 0.0 },
    ParamDef::Float { name: "fog_amount",  min: 0.0, max: 1.0, default: 0.0 },
];

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "rainy_glass",
        params: PARAMS,
        create_pipeline: create_rainy_glass_pipeline,
        from_params: |params, shared| {
            let speed = match params.get(0) { Some(ParamValue::Float(v)) => *v, _ => 1.0 };
            let rain_amount = match params.get(1) { Some(ParamValue::Float(v)) => *v, _ => 0.7 };
            let direction = match params.get(2) { Some(ParamValue::Float(v)) => *v, _ => 0.0 };
            let fog_amount = match params.get(3) { Some(ParamValue::Float(v)) => *v, _ => 0.0 };
            Box::new(RainyGlass::new(speed, rain_amount, direction, fog_amount, shared))
        },
    }
}

/// GPU uniforms for the rainy glass shader.
/// All f32 fields — no vec2/vec4 members, so Rust repr(C) and WGSL
/// layouts match without padding.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct RainyGlassUniforms {
    time: f32,
    rain_amount: f32,
    resolution_x: f32,
    resolution_y: f32,
    /// Rain direction in radians. 0 = down (after Y-flip compensation).
    direction: f32,
    /// 0 = clear glass, 1 = fully foggy. Drops and trails cut through.
    fog_amount: f32,
}

#[derive(Clone, Debug)]
pub struct RainyGlass {
    pub speed: f32,
    pub rain_amount: f32,
    /// Rain direction in degrees (0 = down, 90 = right, 180 = up, 270 = left).
    pub direction: f32,
    /// 0 = clear glass (default), 1 = fully foggy. Drops and trails cut through.
    pub fog_amount: f32,
    /// Accumulated effective time (speed-scaled).
    time: f32,
    shared: Arc<EffectPipeline>,
}

/// Compute internal render dimensions from the pixel budget and aspect ratio.
/// Returns (width, height) such that width * height ~ PIXEL_BUDGET.
fn internal_dimensions(viewport_width: u32, viewport_height: u32) -> (u32, u32) {
    let aspect = viewport_width as f32 / viewport_height.max(1) as f32;
    let ih = (PIXEL_BUDGET / aspect).sqrt().round().max(1.0);
    let iw = (ih * aspect).round().max(1.0);
    (iw as u32, ih as u32)
}

impl RainyGlass {
    pub fn new(speed: f32, rain_amount: f32, direction: f32, fog_amount: f32, shared: Arc<EffectPipeline>) -> Self {
        RainyGlass {
            speed,
            rain_amount,
            direction,
            fog_amount,
            time: 0.0,
            shared,
        }
    }
}

impl Veil for RainyGlass {
    fn type_id(&self) -> &'static str {
        "rainy_glass"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Float(self.speed),
            ParamValue::Float(self.rain_amount),
            ParamValue::Float(self.direction),
            ParamValue::Float(self.fog_amount),
        ]
    }

    fn needs_animation(&self) -> bool {
        self.speed > 0.0
    }

    fn update_time(&mut self, queue: &wgpu::Queue, cache: &EffectCache, dt: f32) {
        self.time += dt * self.speed;
        if let Some(buf) = cache.uniform_bufs.first() {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&self.time));
        }
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
        let (iw, ih) = internal_dimensions(viewport_width, viewport_height);
        log::info!(
            "rainy_glass: viewport {}x{} -> internal {}x{} ({} pixels)",
            viewport_width, viewport_height, iw, ih, iw * ih,
        );

        // --- Uniforms ---
        // Convert direction to radians and add π to compensate for our
        // Y-flip (vertex shader does 1-uv.y) vs Shadertoy's Y-up convention.
        let dir_rad = self.direction.to_radians() + std::f32::consts::PI;
        let uniforms = RainyGlassUniforms {
            time: self.time,
            rain_amount: self.rain_amount,
            resolution_x: iw as f32,
            resolution_y: ih as f32,
            direction: dir_rad,
            fog_amount: self.fog_amount,
        };
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rainy-glass-uniforms"),
            size: std::mem::size_of::<RainyGlassUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        // --- Low-res aux texture for the rain shader pass ---
        let aux_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rainy-glass-aux"),
            size: wgpu::Extent3d { width: iw, height: ih, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let aux_view = aux_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // --- Bind groups for the rain shader pass (ping_pong -> aux) ---
        let rain_layout = &self.shared.bind_group_layout;
        let rain_bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("rainy-glass-rain-{i}")),
                layout: rain_layout,
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

        // --- Blit pipeline + bind groups for the upscale pass (aux -> dst) ---
        let blit = create_blit_pipeline(
            device,
            wgpu::TextureFormat::Rgba8Unorm,
            "rainy-glass-blit",
        );
        let blit_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rainy-glass-blit-bg"),
            layout: &blit.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&aux_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        // Upscale source is fixed (always the aux texture), so both
        // ping-pong slots use the same bind group.
        let blit_bind_groups: [wgpu::BindGroup; 2] = [blit_bg.clone(), blit_bg];

        EffectCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: vec![rain_bind_groups, blit_bind_groups],
            aux_textures: vec![aux_tex],
            aux_views: vec![aux_view],
            aux_pipelines: vec![blit.pipeline],
        }
    }

    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        src_idx: usize,
        dst_view: &wgpu::TextureView,
    ) {
        // Pass 1: Run rain shader at internal resolution -> aux texture.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rainy-glass-rain"),
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
            rpass.set_pipeline(&self.shared.pipeline);
            rpass.set_bind_group(0, &cache.bind_groups[0][src_idx], &[]);
            rpass.draw(0..3, 0..1);
        }

        // Pass 2: Blit aux texture up to full viewport -> dst_view.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rainy-glass-upscale"),
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
            rpass.set_bind_group(0, &cache.bind_groups[1][0], &[]);
            rpass.draw(0..3, 0..1);
        }
    }
}

fn create_rainy_glass_pipeline(
    device: &wgpu::Device,
    _format: wgpu::TextureFormat,
) -> EffectPipeline {
    let bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rainy-glass-bgl"),
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
        label: Some("rainy-glass-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rainy-glass-shader"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../../../../../shaders/veils/rainy_glass.wgsl").into(),
        ),
    });

    // The rain shader renders into the low-res aux texture (Rgba8Unorm),
    // not the surface format.
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("rainy-glass-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_rainy_glass"),
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
