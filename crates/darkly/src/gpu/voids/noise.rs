//! Noise void — domain-warped fractional Brownian motion.
//!
//! The cloudy / marbled / lightning-like procedural field described in the
//! README as the "de-facto Void". Pure-GPU: the shader generates the layer's
//! pixels from a handful of uniforms; nothing is stored on disk except the
//! params themselves.
//!
//! The FBM primitive lives in `shaders/lib/fbm.wgsl` and is concatenated
//! ahead of this void's fragment shader at pipeline-creation time. A future
//! warp veil will reuse the same helper as a displacement map.

use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::void::{ParamDef, ParamValue, Void, VoidRegistration};
use std::sync::Arc;

pub const TYPE_ID: &str = "noise";

const PARAMS: &[ParamDef] = &[
    // Seed indexes the procedural field — every integer produces a different
    // noise pattern, so a randomize button (or just typing a number) gives
    // the "infinite combinations of entropy" the README promises.
    ParamDef::Int {
        name: "seed",
        min: 0,
        max: i32::MAX,
        default: 42,
    },
    // Octave count of the underlying FBM. More octaves = more detail; cost
    // scales linearly. 5 is a good cloud-like default.
    ParamDef::Int {
        name: "octaves",
        min: 1,
        max: 8,
        default: 5,
    },
    // Base spatial frequency. Lower = larger features; higher = grainier.
    // The default is tuned for 1k–2k canvases producing visible cloud
    // structure without going either flat or noisy.
    ParamDef::Float {
        name: "frequency",
        min: 0.0005,
        max: 0.05,
        default: 0.005,
    },
    // Domain-warp strength. 0 = pure FBM, increasing values produce more
    // marbled / swirly deformation per Quilez's warp.
    ParamDef::Float {
        name: "warp",
        min: 0.0,
        max: 3.0,
        default: 1.5,
    },
    // Color blend. 0 = grayscale (single field), 1 = full RGB (three
    // independent fields). Mid-values give tinted variants.
    ParamDef::Float {
        name: "color",
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
    // Per-frame drift rate. 0 = static; higher values scroll the field over
    // time. The compositor's animation master-clock divisor throttles how
    // often `update_time` fires.
    ParamDef::Float {
        name: "evolution",
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
];

pub fn register() -> VoidRegistration {
    VoidRegistration {
        type_id: TYPE_ID,
        display_name: "Noise",
        params: PARAMS,
        create_pipeline,
        from_params: |params, shared| Box::new(Noise::from_params(params, shared)),
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct NoiseUniforms {
    seed: u32,
    octaves: i32,
    frequency: f32,
    warp: f32,
    color: f32,
    time: f32,
    _pad0: f32,
    _pad1: f32,
}

#[derive(Clone, Debug)]
pub struct Noise {
    pub seed: i32,
    pub octaves: i32,
    pub frequency: f32,
    pub warp: f32,
    pub color: f32,
    pub evolution: f32,
    /// Accumulated time for animation. Transient compositor state — not
    /// undoable, not serialized. The document-side params (above) are the
    /// authoritative inputs; `time` is just a drift offset that resets to 0
    /// whenever the void is re-created from params (e.g. on load).
    time: f32,
    shared: Arc<EffectPipeline>,
}

impl Noise {
    fn from_params(params: &[ParamValue], shared: Arc<EffectPipeline>) -> Self {
        let seed = match params.first() {
            Some(ParamValue::Int(v)) => *v,
            _ => 42,
        };
        let octaves = match params.get(1) {
            Some(ParamValue::Int(v)) => *v,
            _ => 5,
        };
        let frequency = match params.get(2) {
            Some(ParamValue::Float(v)) => *v,
            _ => 0.005,
        };
        let warp = match params.get(3) {
            Some(ParamValue::Float(v)) => *v,
            _ => 1.5,
        };
        let color = match params.get(4) {
            Some(ParamValue::Float(v)) => *v,
            _ => 0.0,
        };
        let evolution = match params.get(5) {
            Some(ParamValue::Float(v)) => *v,
            _ => 0.0,
        };
        Noise {
            seed,
            octaves,
            frequency,
            warp,
            color,
            evolution,
            time: 0.0,
            shared,
        }
    }

    fn uniforms(&self) -> NoiseUniforms {
        NoiseUniforms {
            seed: self.seed as u32,
            octaves: self.octaves,
            frequency: self.frequency,
            warp: self.warp,
            color: self.color,
            time: self.time,
            _pad0: 0.0,
            _pad1: 0.0,
        }
    }
}

impl Void for Noise {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn clone_boxed(&self) -> Box<dyn Void> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Int(self.seed),
            ParamValue::Int(self.octaves),
            ParamValue::Float(self.frequency),
            ParamValue::Float(self.warp),
            ParamValue::Float(self.color),
            ParamValue::Float(self.evolution),
        ]
    }

    fn needs_animation(&self) -> bool {
        self.evolution > 0.0
    }

    fn update_time(&mut self, queue: &wgpu::Queue, cache: &EffectCache, dt: f32) {
        self.time += dt * self.evolution;
        if let Some(buf) = cache.uniform_bufs.first() {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&self.uniforms()));
        }
    }

    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _dst_view: &wgpu::TextureView,
        _sampler: &wgpu::Sampler,
        _render_width: u32,
        _render_height: u32,
    ) -> EffectCache {
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("void-noise-uniforms"),
            size: std::mem::size_of::<NoiseUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&self.uniforms()));

        // Build the bind group twice off the same uniform buffer so the
        // `EffectCache::bind_groups: Vec<[BindGroup; 2]>` shape stays
        // consistent with veils. Voids have no ping-pong source — the
        // encode path always reads index 0 — but mirroring keeps the shared
        // cache shape simple.
        let bg = |label: &str| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.shared.bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                }],
            })
        };
        let bind_groups = [bg("void-noise-bg-0"), bg("void-noise-bg-1")];

        EffectCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: vec![bind_groups],
            aux_textures: Vec::new(),
            aux_views: Vec::new(),
            aux_pipelines: Vec::new(),
        }
    }

    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        dst_view: &wgpu::TextureView,
    ) {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("void-noise-encode"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        rpass.set_pipeline(&self.shared.pipeline);
        rpass.set_bind_group(0, &cache.bind_groups[0][0], &[]);
        rpass.draw(0..3, 0..1);
    }
}

fn create_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("void-noise-bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("void-noise-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    // WGSL has no native #include — concatenate the shared FBM helper ahead
    // of this void's shader. A future warp veil will assemble the same way.
    let fbm_src = include_str!("../../../../../shaders/lib/fbm.wgsl");
    let void_src = include_str!("../../../../../shaders/voids/noise.wgsl");
    let full_src = format!("{fbm_src}\n{void_src}");

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("void-noise-shader"),
        source: wgpu::ShaderSource::Wgsl(full_src.into()),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("void-noise-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
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
