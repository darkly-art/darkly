//! Floating content GPU pipeline — transform-blend shader, texture management,
//! and GPU commit render pass.
//!
//! Used by both paste-in-place and the interactive transform tool. The GPU
//! texture provides real-time preview during interaction, and the commit
//! render pass writes transformed pixels directly to the layer texture.

use crate::layer::LayerId;

// ---------------------------------------------------------------------------
// Affine matrix helpers  ([a, b, tx, c, d, ty])
// ---------------------------------------------------------------------------

/// 2D affine matrix stored as [a, b, tx, c, d, ty].
/// Transforms point (x,y) → (a*x + b*y + tx, c*x + d*y + ty).
pub type Affine2D = [f32; 6];

/// Identity affine: no transformation.
pub const IDENTITY: Affine2D = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

/// Compute the inverse of a 2D affine matrix.
/// Returns None if the matrix is singular (det ≈ 0).
pub fn affine_inverse(m: &Affine2D) -> Option<Affine2D> {
    let [a, b, tx, c, d, ty] = *m;
    let det = a * d - b * c;
    if det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    Some([
        d * inv_det,
        -b * inv_det,
        (b * ty - d * tx) * inv_det,
        -c * inv_det,
        a * inv_det,
        (c * tx - a * ty) * inv_det,
    ])
}

/// Transform a point by an affine matrix.
pub fn affine_transform(m: &Affine2D, x: f32, y: f32) -> (f32, f32) {
    let [a, b, tx, c, d, ty] = *m;
    (a * x + b * y + tx, c * x + d * y + ty)
}

/// Multiply two affine matrices: result = a ∘ b (apply b first, then a).
pub fn affine_multiply(a: &Affine2D, b: &Affine2D) -> Affine2D {
    [
        a[0] * b[0] + a[1] * b[3],
        a[0] * b[1] + a[1] * b[4],
        a[0] * b[2] + a[1] * b[5] + a[2],
        a[3] * b[0] + a[4] * b[3],
        a[3] * b[1] + a[4] * b[4],
        a[3] * b[2] + a[4] * b[5] + a[5],
    ]
}

/// Build a translation affine.
pub fn affine_translate(tx: f32, ty: f32) -> Affine2D {
    [1.0, 0.0, tx, 0.0, 1.0, ty]
}

/// Build a scale affine.
pub fn affine_scale(sx: f32, sy: f32) -> Affine2D {
    [sx, 0.0, 0.0, 0.0, sy, 0.0]
}

/// Build a rotation affine (angle in radians, CCW).
pub fn affine_rotate(angle: f32) -> Affine2D {
    let (s, c) = angle.sin_cos();
    [c, -s, 0.0, s, c, 0.0]
}

// ---------------------------------------------------------------------------
// FloatingContent — CPU-side data owned by the engine
// ---------------------------------------------------------------------------

/// How the floating content was created — determines commit/cancel behavior.
pub enum FloatingMode {
    /// Clipboard paste — commit composites INTO target. Cancel = no-op.
    Paste,
    /// Extracted from layer — commit writes transformed pixels.
    /// Cancel restores the pre-clear state from RegionStore scratch.
    Transform {
        /// Texture format of the target (Rgba8Unorm or R8Unorm).
        format: wgpu::TextureFormat,
        /// Bounding rect of the source region that was cleared [x, y, w, h].
        clear_rect: [u32; 4],
    },
}

/// Floating content state, owned by the engine.
///
/// Source pixel data lives on the GPU (in TransformState's source_texture).
/// This struct holds only the metadata needed for the transform UI and commit.
pub struct FloatingContent {
    /// Pixel offset of the source content in document space.
    pub source_origin: (i32, i32),
    /// Source dimensions in pixels.
    pub source_width: u32,
    pub source_height: u32,
    /// Current affine transform matrix.
    pub matrix: Affine2D,
    /// Target layer.
    pub target_layer: LayerId,
    /// Whether the target is a mask (vs layer tiles).
    pub target_is_mask: bool,
    /// Determines commit/cancel behavior.
    pub mode: FloatingMode,
}

impl FloatingContent {
    /// Compute the bounding box of the transformed source in document pixels.
    /// Returns (min_x, min_y, max_x, max_y) inclusive.
    pub fn transformed_bounds(&self) -> (i32, i32, i32, i32) {
        let (ox, oy) = self.source_origin;
        let w = self.source_width as f32;
        let h = self.source_height as f32;

        // Transform the four corners of the source rectangle
        let corners = [
            affine_transform(&self.matrix, 0.0, 0.0),
            affine_transform(&self.matrix, w, 0.0),
            affine_transform(&self.matrix, 0.0, h),
            affine_transform(&self.matrix, w, h),
        ];

        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for (cx, cy) in &corners {
            min_x = min_x.min(*cx);
            min_y = min_y.min(*cy);
            max_x = max_x.max(*cx);
            max_y = max_y.max(*cy);
        }

        (
            (min_x + ox as f32).floor() as i32,
            (min_y + oy as f32).floor() as i32,
            (max_x + ox as f32).ceil() as i32,
            (max_y + oy as f32).ceil() as i32,
        )
    }
}

// ---------------------------------------------------------------------------
// TransformPass — GPU pipeline and active state, owned by compositor
// ---------------------------------------------------------------------------

/// Uniforms for the transform-blend shader (64 bytes, std140-aligned).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TransformBlendUniforms {
    /// Inverse affine row 0: [a, b, tx, _pad]
    pub inv_row0: [f32; 4],
    /// Inverse affine row 1: [c, d, ty, _pad]
    pub inv_row1: [f32; 4],
    /// Source origin in canvas pixel coords.
    pub source_origin: [f32; 2],
    /// Source texture dimensions in pixels.
    pub source_size: [f32; 2],
    /// Full canvas dimensions in pixels.
    pub canvas_size: [f32; 2],
    /// Opacity (0.0–1.0).
    pub opacity: f32,
    pub _pad: f32,
}

/// GPU resources for an active floating content.
pub struct TransformState {
    pub source_texture: wgpu::Texture,
    pub source_view: wgpu::TextureView,
    pub uniform_buf: wgpu::Buffer,
    /// bind_groups[src_accum_index] — two for ping-pong.
    pub bind_groups: [wgpu::BindGroup; 2],
    /// Bind group reading from composite cache as background.
    pub cache_source_bind_group: wgpu::BindGroup,
    /// Bind group for commit pass (source + sampler + uniforms only).
    pub commit_bind_group: wgpu::BindGroup,
    pub target_layer: LayerId,
    pub target_is_mask: bool,
}

/// GPU pipeline + optional active floating content.
pub struct TransformPass {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// Commit pipelines: render transform directly to layer/mask texture.
    commit_rgba_pipeline: wgpu::RenderPipeline,
    commit_r8_pipeline: wgpu::RenderPipeline,
    commit_bind_group_layout: wgpu::BindGroupLayout,
    pub active: Option<TransformState>,
}

impl TransformPass {
    pub fn new(device: &wgpu::Device, accum_format: wgpu::TextureFormat) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("transform-bgl"),
            entries: &[
                // binding 0: background accumulator
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
                // binding 1: source texture
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // binding 2: sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // binding 3: uniforms
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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
            label: Some("transform-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("transform-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/transform.wgsl").into(),
            ),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("transform-blend-pipeline"),
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
                    format: accum_format,
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

        // --- Commit pipelines (render directly to layer/mask texture) ---
        let commit_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("transform-commit-bgl"),
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

        let commit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("transform-commit-layout"),
            bind_group_layouts: &[&commit_bind_group_layout],
            immediate_size: 0,
        });

        let commit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("transform-commit-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/transform_commit.wgsl").into(),
            ),
        });

        let blend_composite = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let make_commit_pipeline = |label: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&commit_layout),
                vertex: wgpu::VertexState {
                    module: &commit_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &commit_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend_composite),
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
            })
        };

        let commit_rgba_pipeline = make_commit_pipeline(
            "transform-commit-rgba", wgpu::TextureFormat::Rgba8Unorm,
        );
        let commit_r8_pipeline = make_commit_pipeline(
            "transform-commit-r8", wgpu::TextureFormat::R8Unorm,
        );

        TransformPass {
            pipeline,
            bind_group_layout,
            commit_rgba_pipeline,
            commit_r8_pipeline,
            commit_bind_group_layout,
            active: None,
        }
    }

    /// Upload flat RGBA pixel data as a source texture and create bind groups
    /// for compositing against both ping-pong accumulators.
    ///
    /// `rgba_data` must be `source_width * source_height * 4` bytes, row-major,
    /// in straight alpha. This method converts to premultiplied alpha for
    /// correct hardware bilinear interpolation.
    pub fn set_floating_content(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: &wgpu::Sampler,
        accum_views: &[wgpu::TextureView; 2],
        cache_view: &wgpu::TextureView,
        rgba_data: &[u8],
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        canvas_width: u32,
        canvas_height: u32,
        target_layer: LayerId,
        target_is_mask: bool,
    ) {
        let source_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transform-source"),
            size: wgpu::Extent3d {
                width: source_width.max(1),
                height: source_height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Convert straight alpha → premultiplied alpha for correct bilinear interpolation.
        let pixel_count = (source_width * source_height) as usize;
        let mut premul = vec![0u8; pixel_count * 4];
        for i in 0..pixel_count {
            let off = i * 4;
            let a = rgba_data[off + 3] as f32 / 255.0;
            premul[off]     = (rgba_data[off]     as f32 * a).round() as u8;
            premul[off + 1] = (rgba_data[off + 1] as f32 * a).round() as u8;
            premul[off + 2] = (rgba_data[off + 2] as f32 * a).round() as u8;
            premul[off + 3] = rgba_data[off + 3];
        }

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &premul,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(source_width * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: source_width.max(1),
                height: source_height.max(1),
                depth_or_array_layers: 1,
            },
        );

        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Uniform buffer (identity matrix initially)
        let uniforms = TransformBlendUniforms {
            inv_row0: [1.0, 0.0, 0.0, 0.0],
            inv_row1: [0.0, 1.0, 0.0, 0.0],
            source_origin: [source_origin.0 as f32, source_origin.1 as f32],
            source_size: [source_width as f32, source_height as f32],
            canvas_size: [canvas_width as f32, canvas_height as f32],
            opacity: 1.0,
            _pad: 0.0,
        };

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transform-uniforms"),
            size: std::mem::size_of::<TransformBlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        // Create bind groups for both ping-pong directions
        let make_bind_group = |bg_view: &wgpu::TextureView, label: &str| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(bg_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform_buf.as_entire_binding(),
                    },
                ],
            })
        };

        let bg0 = make_bind_group(&accum_views[0], "transform-bg0");
        let bg1 = make_bind_group(&accum_views[1], "transform-bg1");
        let cache_bg = make_bind_group(cache_view, "transform-bg-cache");

        // Commit bind group (source + sampler + uniforms — no background)
        let commit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("transform-commit-bg"),
            layout: &self.commit_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source_view),
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
        });

        self.active = Some(TransformState {
            source_texture,
            source_view,
            uniform_buf,
            bind_groups: [bg0, bg1],
            cache_source_bind_group: cache_bg,
            commit_bind_group,
            target_layer,
            target_is_mask,
        });
    }

    /// Set floating content by copying a region directly from a layer GPU texture.
    ///
    /// GPU→GPU copy via `copy_texture_to_texture` — no CPU round-trip.
    /// Used by `begin_transform` when extracting content from a GPU-authoritative layer.
    pub fn set_floating_content_from_gpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        sampler: &wgpu::Sampler,
        accum_views: &[wgpu::TextureView; 2],
        cache_view: &wgpu::TextureView,
        layer_texture: &wgpu::Texture,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        canvas_width: u32,
        canvas_height: u32,
        target_layer: LayerId,
        target_is_mask: bool,
    ) {
        let src_format = if target_is_mask {
            wgpu::TextureFormat::R8Unorm
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };

        // The source texture is always RGBA8 (the transform shader expects RGBA).
        // For masks (R8), we create an RGBA8 texture and copy the R8 channel.
        // The transform commit shader handles the RGBA→R8 conversion.
        let source_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transform-source-gpu"),
            size: wgpu::Extent3d {
                width: source_width.max(1),
                height: source_height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: src_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        // GPU→GPU copy from layer texture to source texture.
        let src_x = source_origin.0.max(0) as u32;
        let src_y = source_origin.1.max(0) as u32;
        let copy_w = source_width.min(canvas_width.saturating_sub(src_x));
        let copy_h = source_height.min(canvas_height.saturating_sub(src_y));

        if copy_w > 0 && copy_h > 0 {
            // Offset into the source texture if source_origin is negative
            let dst_x = (-source_origin.0).max(0) as u32;
            let dst_y = (-source_origin.1).max(0) as u32;

            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: layer_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: src_x, y: src_y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &source_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: dst_x, y: dst_y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: copy_w.min(source_width - dst_x),
                    height: copy_h.min(source_height - dst_y),
                    depth_or_array_layers: 1,
                },
            );
        }

        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Uniform buffer (identity matrix initially)
        let uniforms = TransformBlendUniforms {
            inv_row0: [1.0, 0.0, 0.0, 0.0],
            inv_row1: [0.0, 1.0, 0.0, 0.0],
            source_origin: [source_origin.0 as f32, source_origin.1 as f32],
            source_size: [source_width as f32, source_height as f32],
            canvas_size: [canvas_width as f32, canvas_height as f32],
            opacity: 1.0,
            _pad: 0.0,
        };

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transform-uniforms"),
            size: std::mem::size_of::<TransformBlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        // Create bind groups (same pattern as set_floating_content)
        let make_bind_group = |bg_view: &wgpu::TextureView, label: &str| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(bg_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform_buf.as_entire_binding(),
                    },
                ],
            })
        };

        let bg0 = make_bind_group(&accum_views[0], "transform-gpu-bg0");
        let bg1 = make_bind_group(&accum_views[1], "transform-gpu-bg1");
        let cache_bg = make_bind_group(cache_view, "transform-gpu-bg-cache");

        let commit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("transform-gpu-commit-bg"),
            layout: &self.commit_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source_view),
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
        });

        self.active = Some(TransformState {
            source_texture,
            source_view,
            uniform_buf,
            bind_groups: [bg0, bg1],
            cache_source_bind_group: cache_bg,
            commit_bind_group,
            target_layer,
            target_is_mask,
        });
    }

    /// Update the affine matrix uniform for real-time preview.
    pub fn update_matrix(
        &self,
        queue: &wgpu::Queue,
        matrix: &Affine2D,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        canvas_width: u32,
        canvas_height: u32,
    ) {
        let state = match &self.active {
            Some(s) => s,
            None => return,
        };

        let inv = affine_inverse(matrix).unwrap_or(IDENTITY);

        let uniforms = TransformBlendUniforms {
            inv_row0: [inv[0], inv[1], inv[2], 0.0],
            inv_row1: [inv[3], inv[4], inv[5], 0.0],
            source_origin: [source_origin.0 as f32, source_origin.1 as f32],
            source_size: [source_width as f32, source_height as f32],
            canvas_size: [canvas_width as f32, canvas_height as f32],
            opacity: 1.0,
            _pad: 0.0,
        };

        queue.write_buffer(&state.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Render the transformed source directly onto a target texture (layer or mask).
    /// Used by commit_floating() to replace CPU-side rasterize_to_tiles().
    pub fn commit_to_texture(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        target_view: &wgpu::TextureView,
        target_format: wgpu::TextureFormat,
        matrix: &Affine2D,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        canvas_width: u32,
        canvas_height: u32,
    ) {
        let state = match &self.active {
            Some(s) => s,
            None => return,
        };

        let inv = affine_inverse(matrix).unwrap_or(IDENTITY);
        let is_mask = if target_format == wgpu::TextureFormat::R8Unorm { 1.0 } else { 0.0 };

        // Reuse the preview uniform struct — _pad becomes is_mask for commit.
        let uniforms = TransformBlendUniforms {
            inv_row0: [inv[0], inv[1], inv[2], 0.0],
            inv_row1: [inv[3], inv[4], inv[5], 0.0],
            source_origin: [source_origin.0 as f32, source_origin.1 as f32],
            source_size: [source_width as f32, source_height as f32],
            canvas_size: [canvas_width as f32, canvas_height as f32],
            opacity: 1.0,
            _pad: is_mask,
        };

        queue.write_buffer(&state.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let pipeline = match target_format {
            wgpu::TextureFormat::R8Unorm => &self.commit_r8_pipeline,
            _ => &self.commit_rgba_pipeline,
        };

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("transform-commit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });

        rpass.set_pipeline(pipeline);
        rpass.set_bind_group(0, &state.commit_bind_group, &[]);
        rpass.draw(0..3, 0..1);
    }

    /// Remove floating content GPU state.
    pub fn clear(&mut self) {
        self.active = None;
    }

    /// Check if floating content is active and targets the given layer.
    pub fn targets_layer(&self, layer_id: LayerId) -> bool {
        self.active.as_ref().map_or(false, |s| s.target_layer == layer_id)
    }
}

