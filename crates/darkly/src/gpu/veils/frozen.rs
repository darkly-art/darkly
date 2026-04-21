use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

/// Ice normal map baked into the binary. RGB-encoded surface normal
/// (x, y, z) packed as (n*0.5+0.5). Decoded at texture-upload time.
const FROZEN_NORMAL_BYTES: &[u8] = include_bytes!("../../../resources/veils/frozen.jpg");

const PARAMS: &[ParamDef] = &[
    ParamDef::Float {
        name: "strength",
        min: 0.0,
        max: 0.2,
        default: 0.04,
    },
    ParamDef::Float {
        name: "scale",
        min: 0.1,
        max: 5.0,
        default: 1.0,
    },
    ParamDef::Float {
        name: "chromatic",
        min: 0.0,
        max: 1.0,
        default: 0.1,
    },
];

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "frozen",
        params: PARAMS,
        create_pipeline: create_frozen_pipeline,
        from_params: |params, shared| {
            let strength = match params.first() {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.04,
            };
            let scale = match params.get(1) {
                Some(ParamValue::Float(v)) => *v,
                _ => 1.0,
            };
            let chromatic = match params.get(2) {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.1,
            };
            Box::new(Frozen::new(strength, scale, chromatic, shared))
        },
    }
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
    /// Tile density for the ice pattern. 1.0 = one tile across the shorter
    /// screen dimension; higher = more, finer crystals.
    pub scale: f32,
    /// Chromatic aberration: 0 = clean refraction, 1 = pronounced prism edge.
    pub chromatic: f32,
    shared: Arc<EffectPipeline>,
}

impl Frozen {
    pub fn new(strength: f32, scale: f32, chromatic: f32, shared: Arc<EffectPipeline>) -> Self {
        Frozen {
            strength,
            scale,
            chromatic,
            shared,
        }
    }
}

impl Veil for Frozen {
    fn type_id(&self) -> &'static str {
        "frozen"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Float(self.strength),
            ParamValue::Float(self.scale),
            ParamValue::Float(self.chromatic),
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
        // Decode the baked-in normal map and upload as an aux texture.
        let decoded = image::load_from_memory(FROZEN_NORMAL_BYTES)
            .expect("failed to decode frozen normal map")
            .to_rgba8();
        let (nw, nh) = decoded.dimensions();
        let normal_aspect = nw as f32 / nh as f32;

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

        let uniforms = FrozenUniforms {
            resolution_x: render_width as f32,
            resolution_y: render_height as f32,
            normal_aspect,
            strength: self.strength,
            scale: self.scale,
            chromatic: self.chromatic,
            _pad0: 0.0,
            _pad1: 0.0,
        };
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frozen-uniforms"),
            size: std::mem::size_of::<FrozenUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

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

fn create_frozen_pipeline(device: &wgpu::Device, _format: wgpu::TextureFormat) -> EffectPipeline {
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("frozen-bgl"),
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
        label: Some("frozen-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("frozen-shader"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../../../../../shaders/veils/frozen.wgsl").into(),
        ),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("frozen-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_frozen"),
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
