use crate::gpu::effect::{
    create_effect_pipeline, Binding, Effect, EffectCache, EffectPipeline, EffectRegistration,
    COLOR_TARGETS,
};
use crate::gpu::params::{ParamDef, ParamSlots, ParamValue};
use crate::gpu::preview::{PreviewAnim, PREVIEW_SECONDS};
use std::sync::Arc;

const PARAMS: &[ParamDef] = &[
    ParamDef::float("speed", 0.0, 3.0, 0.5)
        .with_label("Speed")
        .with_description("How fast the tape artefacts drift and flicker."),
    ParamDef::float("wobble", 0.0, 2.0, 1.0)
        .with_label("Wobble")
        .with_description("Horizontal waver of each scanline, as if the tape were stretched."),
    ParamDef::float("switching", 0.0, 2.0, 1.0)
        .with_label("Switching Noise")
        .with_description(
            "Torn band of static at the bottom of the frame where the head switches.",
        ),
    ParamDef::float("bloom", 0.0, 2.0, 1.0)
        .with_label("Bloom")
        .with_description("How far bright areas smear and glow into their surroundings."),
    ParamDef::float("ac_beat", 0.0, 2.0, 1.0)
        .with_label("Hum Bar")
        .with_description("Slow bright bar rolling up the frame from mains interference."),
];

pub fn register() -> EffectRegistration {
    EffectRegistration {
        type_id: "vhs",
        display_name: "VHS",
        category: "Veils",
        icon: "fa6-solid:tv",
        hotkey_action: "effectVhs",
        description: "Analog VHS tape artifacts — scanlines, noise, and color bleed.",
        params: PARAMS,
        preview: Some(PreviewAnim::ONE_WAY),
        preview_at: None,
        targets: COLOR_TARGETS,
        create_pipeline: create_vhs_pipeline,
        from_params: |params, shared| {
            let (speed, wobble, switching, bloom, ac_beat) = read_params(params);
            Box::new(Vhs::new(speed, wobble, switching, bloom, ac_beat, shared))
        },
    }
}

/// Read the schema-ordered parameter vector, falling back to the schema
/// defaults per slot. The one place the positional order is decoded, so
/// construction and a later parameter change cannot disagree about it.
fn read_params(params: &[ParamValue]) -> (f32, f32, f32, f32, f32) {
    let speed = params.float_at(0, 0.5);
    let wobble = params.float_at(1, 1.0);
    let switching = params.float_at(2, 1.0);
    let bloom = params.float_at(3, 1.0);
    let ac_beat = params.float_at(4, 1.0);
    (speed, wobble, switching, bloom, ac_beat)
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

impl Effect for Vhs {
    fn type_id(&self) -> &'static str {
        "vhs"
    }

    fn clone_boxed(&self) -> Box<dyn Effect> {
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

    /// Two seconds of the veil's own tape clock. The artefacts this veil is
    /// made of are temporal — the wobble, the switching noise, the AC beat — so
    /// its preview runs time rather than any parameter. The clock runs forward
    /// and does not return to where it started, so the sequence does not loop;
    /// making it do so would mean a periodic time basis in the shader, which is
    /// a change to the effect rather than to its preview.
    fn set_params(
        &mut self,
        queue: &wgpu::Queue,
        cache: &EffectCache,
        params: &[ParamValue],
    ) -> bool {
        let (speed, wobble, switching, bloom, ac_beat) = read_params(params);
        self.speed = speed;
        self.wobble = wobble;
        self.switching = switching;
        self.bloom = bloom;
        self.ac_beat = ac_beat;
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
        true
    }

    /// The preview is a span of the effect's own clock, so positioning it is
    /// setting the time directly — scaled by `speed`, so a slower instance
    /// covers less of its motion over the same preview.
    fn seek(&mut self, queue: &wgpu::Queue, cache: &EffectCache, t: f32) {
        self.time = PREVIEW_SECONDS * t * self.speed;
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
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

fn create_vhs_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    create_effect_pipeline(
        device,
        format,
        "vhs",
        &[Binding::Texture, Binding::Sampler, Binding::Uniform],
        include_str!("../../../shaders/effects/vhs.wgsl"),
        "fs_vhs",
    )
}
