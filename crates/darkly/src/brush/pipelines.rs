//! Pre-built GPU pipelines for the brush system.
//!
//! Three pipelines:
//! - **Circle**: renders an SDF circle mask to a dab texture (REPLACE blend).
//! - **Stamp**: renders a brush tip texture to a dab texture with transforms.
//! - **Composite**: composites a dab texture onto the canvas with correct
//!   straight-alpha Porter-Duff source-over (REPLACE blend, shader-side composite).
//!
//! The composite pass reads a copy of the canvas region (captured before
//! compositing) so the shader can do manual Porter-Duff blending.  This avoids
//! the premultiplied-stored-as-straight bug that hardware alpha blending causes
//! on straight-alpha layer textures (see compositing-lessons-learned.md #2).
//!
//! Separate from `PaintPipelines` — different concerns (dab generation +
//! dab compositing vs. SDF circle painting + gradient fill).

use std::cell::Cell;
use std::num::NonZeroU64;


/// Uniform data for the circle mask generation shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CircleUniforms {
    pub softness: f32,       // 0-1 fraction of radius
    pub _pad: [f32; 3],      // padding to 16-byte alignment
}

/// Uniform data for the stamp dab generation shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StampUniforms {
    pub dab_width: f32,      // dab viewport width in pixels
    pub dab_height: f32,     // dab viewport height in pixels
    pub opacity: f32,        // dab opacity (0-1)
    pub rotation: f32,       // dab rotation in radians
    pub color: [f32; 4],     // RGBA paint color (straight alpha)
    pub mirror_x: f32,       // 1.0 = flip horizontally
    pub mirror_y: f32,       // 1.0 = flip vertically
    pub application: u32,    // BrushTipApplication as u32
    pub ratio: f32,          // user-controlled aspect ratio squeeze (1.0 = none)
}

/// Uniform data for the texture overlay shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TexOverlayUniforms {
    pub dab_width: f32,
    pub dab_height: f32,
    pub position_x: f32,
    pub position_y: f32,
    pub pattern_width: f32,
    pub pattern_height: f32,
    pub scale: f32,
    pub strength: f32,
    pub blend_mode: u32,
    pub _pad: [f32; 3],
}

/// Uniform data for the blit shader (preview mask blit).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlitUniforms {
    /// UV corner (0..1) inside the source texture where sampling starts.
    pub uv_min: [f32; 2],
    /// UV corner (0..1) inside the source texture where sampling ends.
    pub uv_max: [f32; 2],
}

/// Uniform data for the dab compositing shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CompositeUniforms {
    pub origin: [f32; 2],      // quad top-left in canvas pixels
    pub size: [f32; 2],        // quad size in canvas pixels
    pub canvas_size: [f32; 2], // canvas dimensions
    pub uv_min: [f32; 2],     // min UV in dab texture (nonzero when clipped at top/left)
    pub uv_max: [f32; 2],     // max UV in dab texture
    pub blend_mode: u32,       // 0 = source-over, 1 = erase (destination-out)
    pub fg_premultiplied: u32, // 1 = dab input is premultiplied, 0 = straight alpha
}

/// Ring buffer for dynamic uniform offsets.
///
/// Instead of a single uniform buffer that must be submitted between dabs,
/// each dab writes to a unique offset.  All render passes can go into one
/// command encoder and be submitted once.
///
/// Uses `Cell` for `next_index` so `write()` can take `&self` — the ring is
/// never shared across threads.
const UNIFORM_RING_CAPACITY: u32 = 256;

pub struct DynamicUniformRing {
    buffer: wgpu::Buffer,
    aligned_stride: u64,
    capacity: u32,
    next_index: Cell<u32>,
}

impl DynamicUniformRing {
    fn new(device: &wgpu::Device, label: &str, uniform_size: u64, min_alignment: u32) -> Self {
        let aligned_stride = align_up(uniform_size, min_alignment as u64);
        let capacity = UNIFORM_RING_CAPACITY;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: aligned_stride * capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, aligned_stride, capacity, next_index: Cell::new(0) }
    }

    /// Write uniform data to the next slot.  Returns the byte offset for
    /// `set_bind_group`'s dynamic offset array.
    pub fn write(&self, queue: &wgpu::Queue, data: &[u8]) -> u32 {
        let idx = self.next_index.get();
        debug_assert!(idx < self.capacity, "DynamicUniformRing overflow");
        let offset = idx as u64 * self.aligned_stride;
        queue.write_buffer(&self.buffer, offset, data);
        self.next_index.set(idx + 1);
        offset as u32
    }

    /// Reset to slot 0 for the next frame.
    pub fn reset(&self) {
        self.next_index.set(0);
    }

    fn nearly_full(&self) -> bool {
        // Leave headroom for a few more writes after the check (one dab
        // can use up to 3 ring slots across different pipelines).
        self.next_index.get() >= self.capacity - 4
    }

    /// Binding size for the bind group entry (one slot, not the whole buffer).
    fn binding_size(&self) -> NonZeroU64 {
        NonZeroU64::new(self.aligned_stride).unwrap()
    }
}

fn align_up(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

/// Pre-built render pipelines for the brush system.
pub struct BrushPipelines {
    circle_pipeline: wgpu::RenderPipeline,
    stamp_pipeline: wgpu::RenderPipeline,
    tex_overlay_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,

    circle_uniform_ring: DynamicUniformRing,
    pub(crate) circle_uniform_bind_group: wgpu::BindGroup,

    stamp_uniform_ring: DynamicUniformRing,
    pub(crate) stamp_uniform_bind_group: wgpu::BindGroup,

    tex_overlay_uniform_ring: DynamicUniformRing,
    pub(crate) tex_overlay_uniform_bind_group: wgpu::BindGroup,

    composite_uniform_ring: DynamicUniformRing,
    pub(crate) composite_uniform_bind_group: wgpu::BindGroup,

    blit_pipeline: wgpu::RenderPipeline,
    blit_uniform_ring: DynamicUniformRing,
    pub(crate) blit_uniform_bind_group: wgpu::BindGroup,

    /// 1x1 white selection texture — bound when no selection is active.
    pub(crate) default_selection_bind_group: wgpu::BindGroup,
    pub(crate) selection_bgl: wgpu::BindGroupLayout,

    /// Canvas-region copy texture for shader-side Porter-Duff compositing.
    /// Sized to full canvas dimensions — the brush footprint after scaling
    /// can be up to the canvas size.
    canvas_copy_texture: wgpu::Texture,
    // View and BGL are held alive for the bind group's internal Arc references.
    _canvas_copy_view: wgpu::TextureView,
    pub(crate) canvas_copy_bind_group: wgpu::BindGroup,
    canvas_copy_bgl: wgpu::BindGroupLayout,
}

impl BrushPipelines {
    /// Create brush pipelines.
    ///
    /// `dab_bgl` is the dab texture bind group layout from `DabTexturePool`.
    /// `canvas_w`/`canvas_h` size the canvas-copy texture (used for shader-side
    /// Porter-Duff compositing — must be large enough for the biggest possible
    /// brush footprint, which is bounded by the canvas dimensions).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dab_bgl: &wgpu::BindGroupLayout,
        canvas_w: u32,
        canvas_h: u32,
    ) -> Self {
        // --- Shaders ---
        let circle_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-circle"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/circle.wgsl").into(),
            ),
        });

        let stamp_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-stamp"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/stamp.wgsl").into(),
            ),
        });

        let tex_overlay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-tex-overlay"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/texture_overlay.wgsl").into(),
            ),
        });

        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-composite"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("../../../../shaders/source_over.wgsl"), "\n",
                    include_str!("../../../../shaders/brush/composite.wgsl"),
                ).into(),
            ),
        });

        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-blit"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/blit.wgsl").into(),
            ),
        });

        // --- Bind group layouts ---
        let min_align = device.limits().min_uniform_buffer_offset_alignment;

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush-uniform-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let selection_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush-selection-bgl"),
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
            ],
        });

        // Canvas copy bind group layout (texture + sampler, same structure as dab).
        let canvas_copy_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush-canvas-copy-bgl"),
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
            ],
        });

        // --- Pipeline layouts ---
        // Circle: group(0) = uniforms only (renders to dab texture).
        let circle_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-circle-layout"),
            bind_group_layouts: &[&uniform_bgl],
            immediate_size: 0,
        });

        // Stamp: group(0) = uniforms, group(1) = brush tip texture+sampler.
        let stamp_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-stamp-layout"),
            bind_group_layouts: &[&uniform_bgl, dab_bgl],
            immediate_size: 0,
        });

        // Texture overlay: group(0) = uniforms, group(1) = dab texture, group(2) = pattern texture.
        let tex_overlay_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-tex-overlay-layout"),
            bind_group_layouts: &[&uniform_bgl, dab_bgl, dab_bgl],
            immediate_size: 0,
        });

        // Composite: group(0) = uniforms, group(1) = dab texture, group(2) = selection,
        //            group(3) = canvas copy (for shader-side Porter-Duff).
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-composite-layout"),
            bind_group_layouts: &[&uniform_bgl, dab_bgl, &selection_bgl, &canvas_copy_bgl],
            immediate_size: 0,
        });

        // Blit: group(0) = uniforms, group(1) = source texture+sampler.
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-blit-layout"),
            bind_group_layouts: &[&uniform_bgl, dab_bgl],
            immediate_size: 0,
        });

        // --- Dynamic uniform rings ---
        let circle_uniform_ring = DynamicUniformRing::new(
            device, "brush-circle-uniforms",
            std::mem::size_of::<CircleUniforms>() as u64, min_align,
        );
        let circle_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-circle-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &circle_uniform_ring.buffer,
                    offset: 0,
                    size: Some(circle_uniform_ring.binding_size()),
                }),
            }],
        });

        let stamp_uniform_ring = DynamicUniformRing::new(
            device, "brush-stamp-uniforms",
            std::mem::size_of::<StampUniforms>() as u64, min_align,
        );
        let stamp_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-stamp-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &stamp_uniform_ring.buffer,
                    offset: 0,
                    size: Some(stamp_uniform_ring.binding_size()),
                }),
            }],
        });

        let tex_overlay_uniform_ring = DynamicUniformRing::new(
            device, "brush-tex-overlay-uniforms",
            std::mem::size_of::<TexOverlayUniforms>() as u64, min_align,
        );
        let tex_overlay_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-tex-overlay-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &tex_overlay_uniform_ring.buffer,
                    offset: 0,
                    size: Some(tex_overlay_uniform_ring.binding_size()),
                }),
            }],
        });

        let composite_uniform_ring = DynamicUniformRing::new(
            device, "brush-composite-uniforms",
            std::mem::size_of::<CompositeUniforms>() as u64, min_align,
        );
        let composite_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-composite-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &composite_uniform_ring.buffer,
                    offset: 0,
                    size: Some(composite_uniform_ring.binding_size()),
                }),
            }],
        });

        let blit_uniform_ring = DynamicUniformRing::new(
            device, "brush-blit-uniforms",
            std::mem::size_of::<BlitUniforms>() as u64, min_align,
        );
        let blit_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-blit-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &blit_uniform_ring.buffer,
                    offset: 0,
                    size: Some(blit_uniform_ring.binding_size()),
                }),
            }],
        });

        // --- Default selection (1x1 white = fully selected) ---
        let sel_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brush-default-selection"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &sel_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        let sel_view = sel_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sel_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("brush-selection-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let default_selection_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-default-selection-bg"),
            layout: &selection_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&sel_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sel_sampler),
                },
            ],
        });

        // --- Canvas copy texture (for shader-side Porter-Duff) ---
        // Sized to the full canvas so any brush footprint (including scaled
        // brushes) can be composited without hitting a size cap.
        let canvas_copy_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brush-canvas-copy"),
            size: wgpu::Extent3d {
                width: canvas_w,
                height: canvas_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let canvas_copy_view = canvas_copy_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let canvas_copy_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("brush-canvas-copy-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let canvas_copy_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-canvas-copy-bg"),
            layout: &canvas_copy_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&canvas_copy_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&canvas_copy_sampler),
                },
            ],
        });

        // --- Pipelines ---

        // Circle: REPLACE blend — we clear the dab texture and write the SDF mask.
        let circle_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-circle"),
            layout: Some(&circle_layout),
            vertex: wgpu::VertexState {
                module: &circle_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &circle_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // Stamp: REPLACE blend — clear dab texture and stamp the tip image.
        let stamp_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-stamp"),
            layout: Some(&stamp_layout),
            vertex: wgpu::VertexState {
                module: &stamp_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &stamp_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // Texture overlay: REPLACE blend — reads dab + pattern, writes textured dab.
        let tex_overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-tex-overlay"),
            layout: Some(&tex_overlay_layout),
            vertex: wgpu::VertexState {
                module: &tex_overlay_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &tex_overlay_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // Composite: REPLACE blend — the shader does Porter-Duff source-over
        // manually by reading the canvas copy, producing correct straight-alpha output.
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-composite"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // Blit: stretch a UV sub-rect of the source across the target viewport.
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-blit"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            circle_pipeline,
            stamp_pipeline,
            tex_overlay_pipeline,
            composite_pipeline,
            circle_uniform_ring,
            circle_uniform_bind_group,
            stamp_uniform_ring,
            stamp_uniform_bind_group,
            tex_overlay_uniform_ring,
            tex_overlay_uniform_bind_group,
            composite_uniform_ring,
            composite_uniform_bind_group,
            blit_pipeline,
            blit_uniform_ring,
            blit_uniform_bind_group,
            default_selection_bind_group,
            selection_bgl,
            canvas_copy_texture,
            _canvas_copy_view: canvas_copy_view,
            canvas_copy_bind_group,
            canvas_copy_bgl,
        }
    }

    pub fn circle_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.circle_pipeline
    }

    pub fn stamp_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.stamp_pipeline
    }

    pub fn tex_overlay_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.tex_overlay_pipeline
    }

    pub fn composite_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.composite_pipeline
    }

    pub fn blit_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.blit_pipeline
    }

    pub fn selection_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.selection_bgl
    }

    pub fn canvas_copy_texture(&self) -> &wgpu::Texture {
        &self.canvas_copy_texture
    }

    pub fn canvas_copy_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.canvas_copy_bgl
    }

    /// Write circle mask uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_circle_uniforms(&self, queue: &wgpu::Queue, uniforms: &CircleUniforms) -> u32 {
        self.circle_uniform_ring.write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Write stamp dab uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_stamp_uniforms(&self, queue: &wgpu::Queue, uniforms: &StampUniforms) -> u32 {
        self.stamp_uniform_ring.write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Write texture overlay uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_tex_overlay_uniforms(&self, queue: &wgpu::Queue, uniforms: &TexOverlayUniforms) -> u32 {
        self.tex_overlay_uniform_ring.write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Write composite uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_composite_uniforms(&self, queue: &wgpu::Queue, uniforms: &CompositeUniforms) -> u32 {
        self.composite_uniform_ring.write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Write blit uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_blit_uniforms(&self, queue: &wgpu::Queue, uniforms: &BlitUniforms) -> u32 {
        self.blit_uniform_ring.write(queue, bytemuck::bytes_of(uniforms))
    }

    /// True if any ring is close to capacity.  The caller should flush
    /// the current encoder, reset rings, and create a fresh encoder.
    pub fn rings_nearly_full(&self) -> bool {
        self.circle_uniform_ring.nearly_full()
            || self.stamp_uniform_ring.nearly_full()
            || self.tex_overlay_uniform_ring.nearly_full()
            || self.composite_uniform_ring.nearly_full()
            || self.blit_uniform_ring.nearly_full()
    }

    /// Reset all uniform rings for a new frame.
    pub fn reset_uniform_rings(&self) {
        self.circle_uniform_ring.reset();
        self.stamp_uniform_ring.reset();
        self.tex_overlay_uniform_ring.reset();
        self.composite_uniform_ring.reset();
        self.blit_uniform_ring.reset();
    }
}
