use crate::gpu::effect::{
    create_effect_pipeline, Binding, Effect, EffectCache, EffectPipeline, EffectRegistration,
    COLOR_TARGETS,
};
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::preview::{swing, PreviewAnim};
use std::sync::Arc;

/// Ice normal map baked into the binary. RGB-encoded surface normal
/// (x, y, z) packed as (n*0.5+0.5). Decoded at texture-upload time.
const FROZEN_NORMAL_BYTES: &[u8] = include_bytes!("../../../resources/veils/frozen.jpg");

const PARAMS: &[ParamDef] = &[
    ParamDef::float("strength", 0.0, 0.2, 0.02)
        .with_label("Strength")
        .with_description("How far the frosted surface displaces what is behind it."),
    ParamDef::float("scale", 0.1, 5.0, 1.0)
        .with_label("Scale")
        .with_description("Size of the ice crystals."),
    ParamDef::float("chromatic", 0.0, 1.0, 0.1)
        .with_label("Chromatic")
        .with_description("Color separation through the ice, like light through a prism."),
];

pub fn register() -> EffectRegistration {
    EffectRegistration {
        type_id: "frozen",
        display_name: "Frozen",
        category: "Veils",
        icon: "fa6-solid:snowflake",
        hotkey_action: "effectFrozen",
        description: "Frost the view behind a pane of refracting ice.",
        params: PARAMS,
        // Just short of the sweep's peak — `swing(0.4375)` is 0.96, so the
        // still shows the frost within a few percent of its heaviest.
        preview: Some(PreviewAnim::LOOPING.with_still_at(0.4375)),
        preview_at: Some(preview_params),
        targets: COLOR_TARGETS,
        create_pipeline: create_frozen_pipeline,
        from_params: |params, shared| {
            let (strength, scale, chromatic) = read_params(params);
            Box::new(Frozen::new(strength, scale, chromatic, shared))
        },
    }
}

/// The ice crystals coarsen and tighten again — `scale` sweeps across a wide
/// band so the frost pattern visibly grows and shrinks.
///
/// `strength` rides with it rather than holding at its schema default.
/// Displacement is absolute UV (`disp = n.xy * strength * …` in the shader,
/// with no `scale` term), so a fixed `strength` against a zooming pattern
/// halves the warp *per crystal* as the crystals double — which the eye reads
/// as the refraction weakening, not as the crystals growing. Sweeping the two
/// together holds warp-per-crystal constant, and that is what makes the motion
/// read as size. `chromatic` holds, so no colour fringing rides along.
fn preview_params(t: f32) -> Vec<ParamValue> {
    let scale = 0.6 + 1.8 * swing(t);
    vec![
        ParamValue::Float(0.02 * scale),
        ParamValue::Float(scale),
        ParamValue::Float(0.1),
    ]
}

/// Read the schema-ordered parameter vector, falling back to the schema
/// defaults per slot. The one place the positional order is decoded, so
/// construction and a later parameter change cannot disagree about it.
fn read_params(params: &[ParamValue]) -> (f32, f32, f32) {
    let strength = match params.first() {
        Some(ParamValue::Float(v)) => *v,
        _ => 0.02,
    };
    let scale = match params.get(1) {
        Some(ParamValue::Float(v)) => *v,
        _ => 1.0,
    };
    let chromatic = match params.get(2) {
        Some(ParamValue::Float(v)) => *v,
        _ => 0.1,
    };
    (strength, scale, chromatic)
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FrozenUniforms {
    resolution_x: f32,
    resolution_y: f32,
    /// Width / height of the decoded normal map texture.
    normal_aspect: f32,
    strength: f32,
    scale: f32,
    chromatic: f32,
    _pad0: f32,
    _pad1: f32,
}

#[derive(Clone, Debug)]
pub struct Frozen {
    /// UV displacement magnitude. 0 = no refraction, 0.2 = heavy distortion.
    pub strength: f32,
    /// Size of the ice crystals. 1.0 = one tile of the normal map across
    /// `sqrt(area)`; higher = **fewer, larger** crystals, because the shader
    /// divides the sampling extent by this. Note the refraction magnitude does
    /// *not* ride along — `strength` is absolute UV displacement, so raising
    /// `scale` alone makes the frost read as milder.
    pub scale: f32,
    /// Chromatic aberration: 0 = clean refraction, 1 = pronounced prism edge.
    pub chromatic: f32,
    /// Render resolution and the decoded normal map's aspect, kept from
    /// `create_cache` so [`uniforms`](Self::uniforms) rebuilds the whole struct
    /// from state.
    resolution: (f32, f32),
    normal_aspect: f32,
    shared: Arc<EffectPipeline>,
}

impl Frozen {
    pub fn new(strength: f32, scale: f32, chromatic: f32, shared: Arc<EffectPipeline>) -> Self {
        Frozen {
            strength,
            scale,
            chromatic,
            resolution: (0.0, 0.0),
            normal_aspect: 1.0,
            shared,
        }
    }

    fn uniforms(&self) -> FrozenUniforms {
        FrozenUniforms {
            resolution_x: self.resolution.0,
            resolution_y: self.resolution.1,
            normal_aspect: self.normal_aspect,
            strength: self.strength,
            scale: self.scale,
            chromatic: self.chromatic,
            _pad0: 0.0,
            _pad1: 0.0,
        }
    }
}

impl Effect for Frozen {
    fn type_id(&self) -> &'static str {
        "frozen"
    }

    fn clone_boxed(&self) -> Box<dyn Effect> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Float(self.strength),
            ParamValue::Float(self.scale),
            ParamValue::Float(self.chromatic),
        ]
    }

    /// The uniform also carries the render resolution and the normal map's
    /// aspect, and those are untouched here — which is why the cache stays
    /// valid and this answers `true`.
    fn set_params(
        &mut self,
        queue: &wgpu::Queue,
        cache: &EffectCache,
        params: &[ParamValue],
    ) -> bool {
        let (strength, scale, chromatic) = read_params(params);
        self.strength = strength;
        self.scale = scale;
        self.chromatic = chromatic;
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
        true
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
        // Decode the baked-in normal map and upload as an aux texture.
        let decoded = image::load_from_memory(FROZEN_NORMAL_BYTES)
            .expect("failed to decode frozen normal map")
            .to_rgba8();
        let (nw, nh) = decoded.dimensions();
        self.normal_aspect = nw as f32 / nh as f32;
        self.resolution = (render_width as f32, render_height as f32);

        let normal_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frozen-normal"),
            size: wgpu::Extent3d {
                width: nw,
                height: nh,
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
                texture: &normal_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            decoded.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(nw * 4),
                rows_per_image: Some(nh),
            },
            wgpu::Extent3d {
                width: nw,
                height: nh,
                depth_or_array_layers: 1,
            },
        );
        let normal_view = normal_tex.create_view(&Default::default());

        // Dedicated sampler with REPEAT wrap so the normal map tiles
        // seamlessly across the viewport at any `scale`.
        let normal_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("frozen-normal-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frozen-uniforms"),
            size: std::mem::size_of::<FrozenUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&self.uniforms()));

        let layout = &self.shared.bind_group_layout;
        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("frozen-bg-{i}")),
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
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&normal_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&normal_sampler),
                    },
                ],
            })
        });

        EffectCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: vec![bind_groups],
            aux_textures: vec![normal_tex],
            aux_views: vec![normal_view],
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
            label: Some("frozen"),
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

fn create_frozen_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    create_effect_pipeline(
        device,
        format,
        "frozen",
        &[
            Binding::Texture,
            Binding::Sampler,
            Binding::Uniform,
            Binding::Texture,
            Binding::Sampler,
        ],
        include_str!("../../../shaders/effects/frozen.wgsl"),
        "fs_frozen",
    )
}
