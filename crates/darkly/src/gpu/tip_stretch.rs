//! Stretch a single-dab render so its tip shows at full strength.
//!
//! A single dab carries only its brush's per-dab flow, and a brush that builds
//! opacity out of many faint dabs would show a near-invisible preview. Every
//! preview of one dab as the brush's tip (the hover cursor and the dab
//! thumbnail) goes through this pass: the strongest texel's deviation from the
//! background is scaled up to full contrast and every other texel by the same
//! factor, so the shape and its falloff survive and flow no longer decides
//! whether the tip is visible.
//!
//! Deviation is the largest of the three colour channels. A consumer that
//! displays one channel (the hover overlay reads red) sees the full stretch
//! only when the dab is grey, which every terminal's preview is: the hover
//! seeds white paint, and stroke-only bindings fall back to neutral grey.
//!
//! The peak is reduced on the GPU and consumed by the apply dispatch in the
//! same encoder, so it never reaches the CPU and nothing derived from it is
//! stored.

use super::reduction::{encode_over_grid, seeded_storage};

/// The background a dab was drawn over and the contrast its strongest texel
/// is stretched to, both in the source texture's 0..1 units.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TipStretchRefs {
    pub zero: [f32; 3],
    pub full: f32,
}

impl TipStretchRefs {
    /// A dab drawn over transparent black, stretched to full coverage: the
    /// hover cursor's mask.
    pub const COVERAGE: Self = TipStretchRefs {
        zero: [0.0; 3],
        full: 1.0,
    };

    /// A dab drawn in `fg` over `bg` on an 8-bit target, given as the bytes
    /// that target stores them as, so a background texel deviates by exactly
    /// zero and is never amplified.
    pub fn over(fg: [u8; 3], bg: [u8; 3]) -> Self {
        let unorm = |c: [u8; 3]| c.map(|v| v as f32 / 255.0);
        let (fg, zero) = (unorm(fg), unorm(bg));
        let full = (0..3).map(|c| (fg[c] - zero[c]).abs()).fold(0.0, f32::max);
        TipStretchRefs { zero, full }
    }
}

/// Uniform buffer layout matching the shader's `Params` struct.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    zero: [f32; 4],
    full: f32,
    width: u32,
    height: u32,
    _pad: u32,
}

/// The reduce and apply pipelines over one shared bind-group layout.
pub struct TipStretch {
    reduce: wgpu::ComputePipeline,
    apply: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl TipStretch {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tip-stretch-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/tip_stretch.wgsl").into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tip-stretch-bgl"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    2,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    3,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tip-stretch-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = |entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("tip-stretch-pipeline"),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        TipStretch {
            reduce: pipeline("reduce"),
            apply: pipeline("apply"),
            bind_group_layout,
        }
    }

    /// Record the stretch of `src` into `dst` (an `Rgba8Unorm` storage
    /// texture of the same `width × height`) into `encoder`.
    pub fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        (width, height): (u32, u32),
        refs: TipStretchRefs,
    ) {
        use wgpu::util::DeviceExt;
        let [r, g, b] = refs.zero;
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tip-stretch-params"),
            contents: bytemuck::bytes_of(&Params {
                zero: [r, g, b, 0.0],
                full: refs.full,
                width,
                height,
                _pad: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let peak = seeded_storage(device, &[0]);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tip-stretch-bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: peak.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(dst),
                },
            ],
        });
        encode_over_grid(encoder, &self.reduce, &bind_group, width, height);
        encode_over_grid(encoder, &self.apply, &bind_group, width, height);
    }
}
