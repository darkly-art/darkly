use crate::gpu::effect::{create_effect_pipeline, Binding, EffectCache, EffectPipeline};
use crate::gpu::preview::{PreviewAnim, PREVIEW_SECONDS};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

const PARAMS: &[ParamDef] = &[
    ParamDef::float("speed", 0.0, 3.0, 0.5)
        .with_label("Speed")
        .with_description("How fast the droplets run down the glass."),
    ParamDef::float("rain_amount", 0.0, 1.0, 0.5)
        .with_label("Rain")
        .with_description("How many droplets cover the glass."),
    ParamDef::float("direction", 0.0, 360.0, 0.0)
        .with_label("Direction")
        .with_description("Which way the rain is driven."),
    ParamDef::float("fog_amount", 0.0, 1.0, 0.0)
        .with_label("Fog")
        .with_description("How much condensation clouds the glass between droplets."),
    ParamDef::float("scale", 0.1, 5.0, 1.4)
        .with_label("Scale")
        .with_description("Size of the droplets."),
];

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "rainy_glass",
        display_name: "Rainy Glass",
        description: "Raindrops run down a pane of glass over the view.",
        params: PARAMS,
        preview: Some(PreviewAnim::ONE_WAY),
        create_pipeline: create_rainy_glass_pipeline,
        from_params: |params, shared| {
            let speed = match params.first() {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.5,
            };
            let rain_amount = match params.get(1) {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.5,
            };
            let direction = match params.get(2) {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.0,
            };
            let fog_amount = match params.get(3) {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.0,
            };
            let scale = match params.get(4) {
                Some(ParamValue::Float(v)) => *v,
                _ => 1.4,
            };
            Box::new(RainyGlass::new(
                speed,
                rain_amount,
                direction,
                fog_amount,
                scale,
                shared,
            ))
        },
    }
}

/// GPU uniforms for the rainy glass shader.
/// All f32 fields, no vec2/vec4 members, so Rust repr(C) and WGSL
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
    /// Zoom level for the raindrop pattern. 1.0 = default, higher = more drops.
    scale: f32,
    _pad: f32,
}

#[derive(Clone, Debug)]
pub struct RainyGlass {
    pub speed: f32,
    pub rain_amount: f32,
    /// Rain direction in degrees (0 = down, 90 = right, 180 = up, 270 = left).
    pub direction: f32,
    /// 0 = clear glass (default), 1 = fully foggy. Drops and trails cut through.
    pub fog_amount: f32,
    /// Zoom level for the raindrop pattern. 1.0 = default, higher = more drops.
    pub scale: f32,
    /// Accumulated effective time (speed-scaled).
    time: f32,
    /// Render resolution, kept from `create_cache` so
    /// [`uniforms`](Self::uniforms) rebuilds the whole struct from state.
    resolution: (f32, f32),
    shared: Arc<EffectPipeline>,
}

impl RainyGlass {
    pub fn new(
        speed: f32,
        rain_amount: f32,
        direction: f32,
        fog_amount: f32,
        scale: f32,
        shared: Arc<EffectPipeline>,
    ) -> Self {
        RainyGlass {
            speed,
            rain_amount,
            direction,
            fog_amount,
            scale,
            time: 0.0,
            resolution: (0.0, 0.0),
            shared,
        }
    }

    fn uniforms(&self) -> RainyGlassUniforms {
        RainyGlassUniforms {
            time: self.time,
            rain_amount: self.rain_amount,
            resolution_x: self.resolution.0,
            resolution_y: self.resolution.1,
            // Add π to compensate for our Y-flip (the vertex shader does
            // `1 - uv.y`) against Shadertoy's Y-up convention.
            direction: self.direction.to_radians() + std::f32::consts::PI,
            fog_amount: self.fog_amount,
            scale: self.scale,
            _pad: 0.0,
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
            ParamValue::Float(self.scale),
        ]
    }

    fn needs_animation(&self) -> bool {
        self.speed > 0.0
    }

    /// Two seconds of droplets running down the glass. The motion *is* the
    /// effect, so the preview runs the veil's own clock rather than a
    /// parameter. It runs forward and does not return to its start, so the
    /// sequence does not loop.
    fn preview_at(&mut self, queue: &wgpu::Queue, cache: &EffectCache, t: f32) -> bool {
        self.time = PREVIEW_SECONDS * t * self.speed;
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
        true
    }

    fn update_time(&mut self, queue: &wgpu::Queue, cache: &EffectCache, dt: f32) {
        self.time += dt * self.speed;
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
    }

    fn create_cache(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ping_pong_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        render_width: u32,
        render_height: u32,
    ) -> EffectCache {
        self.resolution = (render_width as f32, render_height as f32);
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rainy-glass-uniforms"),
            size: std::mem::size_of::<RainyGlassUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&self.uniforms()));

        let layout = &self.shared.bind_group_layout;
        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("rainy-glass-bg-{i}")),
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
            label: Some("rainy-glass"),
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

fn create_rainy_glass_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> EffectPipeline {
    create_effect_pipeline(
        device,
        format,
        "rainy-glass",
        &[Binding::Texture, Binding::Sampler, Binding::Uniform],
        include_str!("../../../shaders/veils/rainy_glass.wgsl"),
        "fs_rainy_glass",
    )
}
