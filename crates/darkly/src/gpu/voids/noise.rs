//! Noise void: domain-warped fractional Brownian motion.
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
use crate::gpu::hash::pcg_hash;
use crate::gpu::preview::{swing, PreviewAnim};
use crate::gpu::void::{DirtyFlag, ParamDef, ParamValue, Void, VoidRegistration, VoidSource};
use crate::units::UnitType;
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
    // Seed indexes the procedural field: every integer produces a different
    // noise pattern, so a randomize button (or just typing a number) gives
    // the "infinite combinations of entropy" the README promises.
    ParamDef::int("seed", 0, i32::MAX, 42)
        .with_label("Seed")
        .with_description("Picks which noise pattern is generated; any two seeds look unrelated."),
    // Octave count of the underlying FBM. More octaves = more detail; cost
    // scales linearly. 5 is a good cloud-like default.
    ParamDef::int("octaves", 1, 8, 5)
        .with_label("Detail")
        .with_description("How many layers of ever-finer noise are stacked up."),
    // Feature size in canvas pixels. Higher = larger blobs; lower =
    // grainier. The default is tuned for 1k-2k canvases producing visible
    // cloud structure without going either flat or noisy. Converted to
    // a frequency multiplier (1 / size) at uniform-write time.
    ParamDef::float("size", 20.0, 2000.0, 200.0)
        .with_label("Size")
        .with_description("How large the noise features are on the canvas.")
        .with_unit(UnitType::Pixels),
    // Domain-warp strength. 0 = pure FBM, increasing values produce more
    // marbled / swirly deformation per Quilez's warp.
    ParamDef::float("warp", 0.0, 3.0, 1.5)
        .with_label("Warp")
        .with_description("Bends the noise into marbled, swirling shapes."),
    // Darkness / tonal contrast. Applied as `pow(value, 1.0 + darkness)`
    // in the shader. 0 = linear (washed-out grayscale); higher values
    // push midtones toward black, giving a Watery-style deep base with
    // brighter peaks. Range tuned so the default looks like a moodier
    // cloud field, not a flat gray ramp.
    ParamDef::float("darkness", 0.0, 3.0, 1.0)
        .with_label("Darkness")
        .with_description(
            "Pushes the midtones down, deepening the field beneath the bright peaks.",
        ),
    // Time slider: z-coordinate into the 3D noise volume. Each value
    // produces a different cross-section of the same FBM field; scrub to
    // explore variations of the current seed without changing pattern
    // identity. Range chosen so the full slider covers many full noise-cell
    // crossings at the default Z scale (Z_SCALE = 0.15 in the shader).
    ParamDef::float("time", 0.0, 100.0, 0.0)
        .with_label("Time")
        .with_description(
            "Scrubs through variations of the same seed without changing its character.",
        ),
];

pub fn register() -> VoidRegistration {
    VoidRegistration {
        type_id: TYPE_ID,
        display_name: "Noise",
        description: "Procedural fractal noise: clouds, grain and organic texture from a seed.",
        params: PARAMS,
        icon: "tabler:galaxy",
        preview: Some(PreviewAnim::LOOPING),
        supports_live_transform: true,
        // Purely procedural: no external capture, identity seed transform.
        source: VoidSource::Procedural,
        default_transform: |_, _| crate::transform::Transform::identity(),
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
    darkness: f32,
    time: f32,
    // Multiplier from render-target px → canvas px (the aux buffer may be
    // smaller than the canvas). Baked at `create_cache` and cached on the
    // struct, so every uniform write rebuilds the whole struct from state.
    canvas_scale: f32,
    _pad0: f32,
    // Inverse of the artist transform's homography (packed rows [m, _], see
    // gpu::transform::pack_inv_rows), applied to the canvas-space sampling
    // coordinate so the field pans / scales / rotates / warps with the gizmo.
    // Affine carries inv_row2 = [0,0,1,_], collapsing the perspective divide.
    inv_row0: [f32; 4],
    inv_row1: [f32; 4],
    inv_row2: [f32; 4],
}

#[derive(Debug)]
pub struct Noise {
    pub seed: i32,
    pub octaves: i32,
    /// Feature size in canvas pixels. Converted to a frequency multiplier
    /// (`1.0 / size`) when written to the GPU uniform.
    pub size: f32,
    pub warp: f32,
    pub darkness: f32,
    /// Z-axis offset into the 3D noise volume. Artist-controlled slider; a
    /// scrub control for exploring different cross-sections of the field
    /// without changing the seed.
    pub time: f32,
    /// Artist transform (gizmo affine). The shader samples the field through its
    /// inverse, so the noise pattern pans / scales / rotates under the gizmo.
    transform: crate::transform::Transform,
    /// Render-target→canvas px scale, kept from `create_cache` so every
    /// uniform write rebuilds the whole struct from state and never clobbers it.
    canvas_scale: f32,
    shared: Arc<EffectPipeline>,
    dirty: DirtyFlag,
}

impl Clone for Noise {
    fn clone(&self) -> Self {
        // Clones come up via `clone_boxed` (used by undo / clone_subtree).
        // The clone owns a fresh `EffectCache` from `ensure_void_layer`, so
        // start dirty to force a first encode.
        Noise {
            seed: self.seed,
            octaves: self.octaves,
            size: self.size,
            warp: self.warp,
            darkness: self.darkness,
            time: self.time,
            transform: self.transform,
            canvas_scale: self.canvas_scale,
            shared: self.shared.clone(),
            dirty: DirtyFlag::new_dirty(),
        }
    }
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
        let time = match params.get(5) {
            Some(ParamValue::Float(v)) => *v,
            _ => 0.0,
        };
        Noise {
            seed,
            octaves,
            size,
            warp,
            darkness,
            time,
            transform: crate::transform::Transform::identity(),
            canvas_scale: 1.0,
            shared,
            dirty: DirtyFlag::new_dirty(),
        }
    }

    fn uniforms(&self) -> NoiseUniforms {
        let [inv_row0, inv_row1, inv_row2] =
            crate::gpu::transform::pack_inv_rows(&self.transform.to_projective());
        NoiseUniforms {
            seed: self.seed as u32,
            octaves: self.octaves,
            frequency: 1.0 / self.size.max(1.0),
            warp: self.warp,
            darkness: self.darkness,
            time: self.time,
            canvas_scale: self.canvas_scale,
            _pad0: 0.0,
            inv_row0,
            inv_row1,
            inv_row2,
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
            ParamValue::Float(self.time),
        ]
    }

    fn take_dirty(&mut self) -> bool {
        self.dirty.take()
    }

    fn mark_dirty(&mut self) {
        self.dirty.mark();
    }

    /// The field drifts forward through the noise volume and rewinds. `time` is
    /// an ordinary parameter rather than the animation trait's clock, so the
    /// whole sweep is a value that can be written and re-written, which is what
    /// lets it return to where it started and close the loop.
    fn preview_at(&mut self, queue: &wgpu::Queue, cache: &EffectCache, t: f32) -> bool {
        self.time = 6.0 * swing(t);
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
        self.dirty.mark();
        true
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
        self.time = match params.get(5) {
            Some(ParamValue::Float(v)) => *v,
            _ => self.time,
        };
        // Full write: `canvas_scale` is cached on the struct, so rebuilding
        // the whole uniform can't clobber it.
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
        self.dirty.mark();
    }

    fn set_transform(
        &mut self,
        queue: &wgpu::Queue,
        cache: &EffectCache,
        transform: &crate::transform::Transform,
    ) {
        self.transform = *transform;
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
        self.dirty.mark();
    }

    fn create_cache(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _dst_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        render_width: u32,
        render_height: u32,
    ) -> EffectCache {
        let aux_w = (render_width / AUX_DOWNSCALE).max(AUX_MIN_DIM);
        let aux_h = (render_height / AUX_DOWNSCALE).max(AUX_MIN_DIM);
        self.canvas_scale = render_width as f32 / aux_w as f32;

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("void-noise-uniforms"),
            size: std::mem::size_of::<NoiseUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&self.uniforms()));

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
        // Duplicated to keep the [BindGroup; 2] cache shape: voids don't
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
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    // WGSL has no native #include, so concatenate the shared helpers ahead of
    // this void's shader: the inverse-homography sampler (so the field warps
    // with perspective, sharing one impl with the floating path) and the FBM
    // primitive. A future warp veil will assemble the same way.
    let proj_src = include_str!("../../../shaders/lib/projective.wgsl");
    // `fbm.wgsl` (3D) depends on `fbm_pcg` from `fbm2d.wgsl`, so the 2D core
    // must come first.
    let fbm2d_src = include_str!("../../../shaders/lib/fbm2d.wgsl");
    let fbm_src = include_str!("../../../shaders/lib/fbm.wgsl");
    let void_src = include_str!("../../../shaders/voids/noise.wgsl");
    let full_src = format!("{proj_src}\n{fbm2d_src}\n{fbm_src}\n{void_src}");

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
/// reserved for future use). The seed makes per-instance volumes distinct:
/// if two voids have the same `seed` param they share an identical
/// volume layout, which is the artist-visible determinism contract.
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
