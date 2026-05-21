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

use crate::gpu::effect::{
    create_blit_bind_group, create_blit_pipeline, EffectCache, EffectPipeline,
};
use crate::gpu::void::{ParamDef, ParamValue, Void, VoidRegistration};
use std::sync::Arc;

/// Procedural-render downscale factor. The FBM shader runs into an aux
/// texture sized at `canvas / AUX_DOWNSCALE` (floored at 64) and a bilinear
/// blit pass upsamples to the void's destination. At default `size = 200`
/// FBM features are ~200 canvas pixels wide, so half-resolution is
/// visually indistinguishable while quartering the per-pixel cost of the
/// expensive 3D-FBM shader.
const AUX_DOWNSCALE: u32 = 2;
const AUX_MIN_DIM: u32 = 64;
const AUX_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Side length of the 3D noise volume sampled by `fbm_value_noise3` in the
/// shader. 64³ × 4 bytes ≈ 1 MiB per void instance; small enough to live
/// in L2 cache on most GPUs, large enough that the FBM domain (which scales
/// up to ~16× the base coordinate at 5 octaves) crosses many texture
/// periods per pixel, masking any cycling pattern.
const NOISE3D_DIM: u32 = 64;

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
    // Feature size in canvas pixels. Higher = larger blobs; lower =
    // grainier. The default is tuned for 1k–2k canvases producing visible
    // cloud structure without going either flat or noisy. Converted to
    // a frequency multiplier (1 / size) at uniform-write time.
    ParamDef::Float {
        name: "size",
        min: 20.0,
        max: 2000.0,
        default: 200.0,
    },
    // Domain-warp strength. 0 = pure FBM, increasing values produce more
    // marbled / swirly deformation per Quilez's warp.
    ParamDef::Float {
        name: "warp",
        min: 0.0,
        max: 3.0,
        default: 1.5,
    },
    // Darkness / tonal contrast. Applied as `pow(value, 1.0 + darkness)`
    // in the shader. 0 = linear (washed-out grayscale); higher values
    // push midtones toward black, giving a Watery-style deep base with
    // brighter peaks. Range tuned so the default looks like a moodier
    // cloud field, not a flat gray ramp.
    ParamDef::Float {
        name: "darkness",
        min: 0.0,
        max: 3.0,
        default: 1.0,
    },
    // Morph speed. 0 = static; higher values evolve the field in place
    // through the z-axis of the 3D noise volume (features appear, morph,
    // and dissolve at fixed canvas positions). The compositor's animation
    // master-clock divisor throttles how often `update_time` fires.
    ParamDef::Float {
        name: "speed",
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
    // User-editable fields. `update_params` writes contiguously through
    // `time` and stops there — keeps the engine-baked `canvas_scale` after
    // safe across slider drags.
    seed: u32,
    octaves: i32,
    frequency: f32,
    warp: f32,
    darkness: f32,
    time: f32,
    // Engine-baked at create_cache. Layout note: must follow `time` so the
    // partial write in `update_params` never touches it.
    canvas_scale: f32,
    _pad0: f32,
}

/// Byte offset of the `time` field inside `NoiseUniforms`. `update_time`
/// writes only this field (an offset write) so the per-cache `canvas_scale`
/// baked at `create_cache` time is not clobbered each tick.
const TIME_FIELD_OFFSET: u64 = 20;

#[derive(Clone, Debug)]
pub struct Noise {
    pub seed: i32,
    pub octaves: i32,
    /// Feature size in canvas pixels. Converted to a frequency multiplier
    /// (`1.0 / size`) when written to the GPU uniform.
    pub size: f32,
    pub warp: f32,
    pub darkness: f32,
    pub speed: f32,
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
        let size = match params.get(2) {
            Some(ParamValue::Float(v)) => *v,
            _ => 200.0,
        };
        let warp = match params.get(3) {
            Some(ParamValue::Float(v)) => *v,
            _ => 1.5,
        };
        let darkness = match params.get(4) {
            Some(ParamValue::Float(v)) => *v,
            _ => 1.0,
        };
        let speed = match params.get(5) {
            Some(ParamValue::Float(v)) => *v,
            _ => 0.0,
        };
        Noise {
            seed,
            octaves,
            size,
            warp,
            darkness,
            speed,
            time: 0.0,
            shared,
        }
    }

    fn uniforms(&self, canvas_scale: f32) -> NoiseUniforms {
        NoiseUniforms {
            seed: self.seed as u32,
            octaves: self.octaves,
            frequency: 1.0 / self.size.max(1.0),
            warp: self.warp,
            darkness: self.darkness,
            time: self.time,
            canvas_scale,
            _pad0: 0.0,
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
            ParamValue::Float(self.size),
            ParamValue::Float(self.warp),
            ParamValue::Float(self.darkness),
            ParamValue::Float(self.speed),
        ]
    }

    fn needs_animation(&self) -> bool {
        self.speed > 0.0
    }

    fn update_time(&mut self, queue: &wgpu::Queue, cache: &EffectCache, dt: f32) {
        self.time += dt * self.speed;
        // Partial write: only the `time` field. `canvas_scale` is baked
        // into the buffer at `create_cache` time and must not be clobbered.
        if let Some(buf) = cache.uniform_bufs.first() {
            queue.write_buffer(buf, TIME_FIELD_OFFSET, bytemuck::bytes_of(&self.time));
        }
    }

    fn update_params(&mut self, queue: &wgpu::Queue, cache: &EffectCache, params: &[ParamValue]) {
        self.seed = match params.first() {
            Some(ParamValue::Int(v)) => *v,
            _ => self.seed,
        };
        self.octaves = match params.get(1) {
            Some(ParamValue::Int(v)) => *v,
            _ => self.octaves,
        };
        self.size = match params.get(2) {
            Some(ParamValue::Float(v)) => *v,
            _ => self.size,
        };
        self.warp = match params.get(3) {
            Some(ParamValue::Float(v)) => *v,
            _ => self.warp,
        };
        self.darkness = match params.get(4) {
            Some(ParamValue::Float(v)) => *v,
            _ => self.darkness,
        };
        self.speed = match params.get(5) {
            Some(ParamValue::Float(v)) => *v,
            _ => self.speed,
        };
        // Partial write: skip `canvas_scale` (baked at create_cache time).
        // Writes the first 24 bytes — fields seed..time inclusive. The
        // `time` field is included so the user-visible animation clock
        // continues uninterrupted across param drags.
        if let Some(buf) = cache.uniform_bufs.first() {
            let scratch = self.uniforms(0.0);
            let bytes = &bytemuck::bytes_of(&scratch)[..TIME_FIELD_OFFSET as usize + 4];
            queue.write_buffer(buf, 0, bytes);
        }
    }

    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _dst_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        render_width: u32,
        render_height: u32,
    ) -> EffectCache {
        let aux_w = (render_width / AUX_DOWNSCALE).max(AUX_MIN_DIM);
        let aux_h = (render_height / AUX_DOWNSCALE).max(AUX_MIN_DIM);
        let canvas_scale = render_width as f32 / aux_w as f32;

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("void-noise-uniforms"),
            size: std::mem::size_of::<NoiseUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &uniform_buf,
            0,
            bytemuck::bytes_of(&self.uniforms(canvas_scale)),
        );

        // 3D noise volume: filled with PCG-hashed bytes, sampled with
        // hardware trilinear filtering by `fbm_value_noise3`. One volume
        // per void instance (~1 MiB at 64³ RGBA8).
        let noise_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("void-noise-volume"),
            size: wgpu::Extent3d {
                width: NOISE3D_DIM,
                height: NOISE3D_DIM,
                depth_or_array_layers: NOISE3D_DIM,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let noise_bytes = seed_noise_volume(NOISE3D_DIM, self.seed as u32);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &noise_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &noise_bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(NOISE3D_DIM * 4),
                rows_per_image: Some(NOISE3D_DIM),
            },
            wgpu::Extent3d {
                width: NOISE3D_DIM,
                height: NOISE3D_DIM,
                depth_or_array_layers: NOISE3D_DIM,
            },
        );
        let noise_view = noise_tex.create_view(&Default::default());

        // Dedicated sampler with Repeat addressing so the seed-offset
        // wrap in the shader works cleanly. Linear filter gives hardware
        // trilinear interpolation.
        let noise_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("void-noise-volume-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // FBM-pass bind groups. Layout from the shared pipeline:
        //   binding 0: uniform buffer
        //   binding 1: 3D noise texture
        //   binding 2: noise sampler
        // Duplicated to keep the [BindGroup; 2] cache shape — voids don't
        // ping-pong but the cache layout is shared with veils.
        let fbm_bg = |label: &str| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.shared.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&noise_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&noise_sampler),
                    },
                ],
            })
        };
        let fbm_bgs = [fbm_bg("void-noise-fbm-bg-0"), fbm_bg("void-noise-fbm-bg-1")];

        // Aux texture: the FBM shader renders here, then a bilinear blit
        // pass upsamples to the void's destination. Format matches the
        // void's destination so a single blit pipeline serves both.
        let aux_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("void-noise-aux"),
            size: wgpu::Extent3d {
                width: aux_w,
                height: aux_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: AUX_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let aux_view = aux_tex.create_view(&Default::default());

        let blit = create_blit_pipeline(device, AUX_FORMAT, "void-noise-blit");
        let blit_bg = |label: &str| {
            create_blit_bind_group(device, &blit.bind_group_layout, &aux_view, sampler, label)
        };
        let blit_bgs = [
            blit_bg("void-noise-blit-bg-0"),
            blit_bg("void-noise-blit-bg-1"),
        ];

        EffectCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: vec![fbm_bgs, blit_bgs],
            aux_textures: vec![aux_tex],
            aux_views: vec![aux_view],
            aux_pipelines: vec![blit.pipeline],
        }
    }

    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        dst_view: &wgpu::TextureView,
    ) {
        // Pass 1: 3D-FBM into the low-res aux texture.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("void-noise-fbm"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cache.aux_views[0],
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
        // Pass 2: bilinear blit aux → destination.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("void-noise-blit"),
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
            rpass.set_pipeline(&cache.aux_pipelines[0]);
            rpass.set_bind_group(0, &cache.bind_groups[1][0], &[]);
            rpass.draw(0..3, 0..1);
        }
    }
}

fn create_pipeline(device: &wgpu::Device, _format: wgpu::TextureFormat) -> EffectPipeline {
    // The FBM pass renders into the aux texture (AUX_FORMAT), not directly
    // to the void's destination, so the pipeline's target format is fixed.
    // The bilinear-blit pass handles converting to whatever destination
    // format the compositor allocated.
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("void-noise-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
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
                format: AUX_FORMAT,
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

/// PCG-hashed bytes for the 3D noise volume. RGBA8 layout matches the
/// `Rgba8Unorm` texture format; each channel is an independent
/// pseudo-random byte so callers that read different channels get
/// decorrelated noise (currently only `.x` is read, but the others are
/// reserved for future use). The seed makes per-instance volumes distinct
/// — if two voids have the same `seed` param they share an identical
/// volume layout, which is the user-visible determinism contract.
fn seed_noise_volume(dim: u32, seed: u32) -> Vec<u8> {
    let count = (dim * dim * dim) as usize;
    let mut bytes = vec![0u8; count * 4];
    let mut s = seed.wrapping_mul(747796405).wrapping_add(2891336453);
    for b in &mut bytes {
        s = pcg_hash(s);
        *b = (s >> 24) as u8;
    }
    bytes
}

/// PCG hash matching the GPU-side `fbm_pcg` in `shaders/lib/fbm.wgsl`.
/// Used to seed the 3D noise volume on the CPU.
fn pcg_hash(n: u32) -> u32 {
    let mut h = n.wrapping_mul(747796405).wrapping_add(2891336453);
    h = ((h >> ((h >> 28) + 4)) ^ h).wrapping_mul(277803737);
    (h >> 22) ^ h
}
