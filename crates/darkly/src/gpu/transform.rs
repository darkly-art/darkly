//! Floating content GPU pipeline — transform-blend shader, texture management,
//! and GPU commit render pass.
//!
//! Used by both paste-in-place and the interactive transform tool. The GPU
//! texture provides real-time preview during interaction, and the commit
//! render pass writes transformed pixels directly to the layer texture.

use crate::gpu::atlas::LayerTexture;
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

/// Shape of the clear that `setup_transform` applied to the source layer.
///
/// Stored on `FloatingMode::Transform` so that `commit_floating` can replay
/// the same shape after the un-clear/save sequence — without it the
/// transform shader's `discard`-outside-transformed-bounds would leave a
/// duplicate copy of the source at its original position.
pub enum ClearShape {
    /// `setup_transform` did a full-rect clear (no-selection branch).
    /// Replay with `clear_rect`.
    Rect(crate::coord::CanvasRect),
    /// `setup_transform` did a selection-shaped clear (selection branch).
    /// `mask_bind_group` references a canvas-sized R8 snapshot of the
    /// selection that was active at setup time — retained because
    /// `gpu_selection.clear()` runs at the end of `setup_transform` (so
    /// the marching ants disappear during the drag preview), and the
    /// commit-side replay needs that mask shape.
    Selection { mask_bind_group: wgpu::BindGroup },
}

/// How the floating content was created — determines commit/cancel behavior.
pub enum FloatingMode {
    /// Clipboard paste — commit composites INTO target.
    /// `created_layer_id = Some(id)` means the target layer was auto-created
    /// for this paste and should be removed on cancel. `None` means paste
    /// targets a pre-existing layer; cancel is a no-op.
    Paste { created_layer_id: Option<LayerId> },
    /// Extracted from layer — commit writes transformed pixels.
    /// Cancel restores the pre-clear state from RegionStore scratch.
    Transform {
        /// Pre-clear snapshot of the source region. Used by `cancel_floating`
        /// to undo the source clear; carries the saved rect and format.
        cancel_snapshot: crate::gpu::region_store::Snapshot,
        /// Shape of the clear setup_transform applied — replayed at commit
        /// time before the transform render. Carrying the shape as data
        /// keeps the selection and no-selection branches symmetric.
        clear_shape: ClearShape,
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
    /// Target node id. Resolves to either a raster layer or a mask modifier;
    /// the texture's own format (looked up via `compositor.node_texture(...)`)
    /// distinguishes the two — no sidecar boolean needed.
    pub target_layer: LayerId,
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

/// Uniforms for the transform-blend shader (96 bytes, std140-aligned).
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
    /// Canvas-space offset of the render target's (0,0) pixel.
    pub target_offset: [f32; 2],
    /// Render target pixel dimensions.
    pub target_size: [f32; 2],
    /// Full document canvas dimensions in pixels.
    pub canvas_size: [f32; 2],
    /// Opacity (0.0–1.0).
    pub opacity: f32,
    /// Format flag for the commit shader (0.0 = RGBA, 1.0 = R8). The shader
    /// uses this to pick an output channel layout, not to express any
    /// mask-vs-layer concept. Unused by the preview shader.
    pub is_r8: f32,
    /// Target layer pixel offset in canvas coords. Used by the preview shader
    /// to sample the target layer's mask at the correct UV. Ignored by the
    /// commit shader.
    pub layer_offset: [f32; 2],
    /// Target layer texture dimensions in pixels. Same role as layer_offset.
    pub layer_size: [f32; 2],
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
}

/// GPU pipeline + optional active floating content.
pub struct TransformPass {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// Commit pipelines: render transform directly to layer/mask texture.
    commit_rgba_pipeline: wgpu::RenderPipeline,
    commit_r8_pipeline: wgpu::RenderPipeline,
    commit_bind_group_layout: wgpu::BindGroupLayout,
    /// Single-texture BGL used for both dest copy (commit) and premultiply passes.
    single_tex_bgl: wgpu::BindGroupLayout,
    premultiply_pipeline: wgpu::RenderPipeline,
    pub active: Option<TransformState>,
}

impl TransformPass {
    /// `mask_bind_group_layout` must match the layout used by the compositor's
    /// existing per-layer mask bind groups (single texture binding, fragment
    /// stage). Reused so the preview pass can bind the same mask BG that the
    /// blend pass uses for the target layer.
    pub fn new(
        device: &wgpu::Device,
        accum_format: wgpu::TextureFormat,
        mask_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
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
            bind_group_layouts: &[&bind_group_layout, mask_bind_group_layout],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("transform-shader"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("../../../../shaders/source_over.wgsl"),
                    "\n",
                    include_str!("../../../../shaders/transform.wgsl"),
                )
                .into(),
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
        let commit_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

        // Single-texture BGL shared by dest copy (commit) and premultiply passes.
        let single_tex_bgl = super::straight_composite::single_texture_bind_group_layout(
            device,
            "transform-single-tex-bgl",
        );

        let commit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("transform-commit-layout"),
            bind_group_layouts: &[&commit_bind_group_layout, &single_tex_bgl],
            immediate_size: 0,
        });

        let commit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("transform-commit-shader"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("../../../../shaders/source_over.wgsl"),
                    "\n",
                    include_str!("../../../../shaders/transform_commit.wgsl"),
                )
                .into(),
            ),
        });

        // Commit uses REPLACE blend — shader computes Porter-Duff manually
        // to avoid premultiplied-stored-as-straight artifacts (lesson #4).
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
            })
        };

        let commit_rgba_pipeline =
            make_commit_pipeline("transform-commit-rgba", wgpu::TextureFormat::Rgba8Unorm);
        let commit_r8_pipeline =
            make_commit_pipeline("transform-commit-r8", wgpu::TextureFormat::R8Unorm);

        // --- Premultiply pipeline (straight→premultiplied alpha conversion) ---
        let premultiply_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("premultiply-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/premultiply.wgsl").into(),
            ),
        });

        let premultiply_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("premultiply-layout"),
            bind_group_layouts: &[&single_tex_bgl],
            immediate_size: 0,
        });

        let premultiply_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("premultiply-pipeline"),
            layout: Some(&premultiply_layout),
            vertex: wgpu::VertexState {
                module: &premultiply_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &premultiply_shader,
                entry_point: Some("fs_main"),
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

        TransformPass {
            pipeline,
            bind_group_layout,
            commit_rgba_pipeline,
            commit_r8_pipeline,
            commit_bind_group_layout,
            single_tex_bgl,
            premultiply_pipeline,
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
        layer_offset: (i32, i32),
        layer_size: (u32, u32),
        target_layer: LayerId,
    ) {
        // Source texture is the premultiplied destination for the floating
        // shaders. We upload the caller's straight-alpha RGBA into a temp
        // texture and run the existing premultiply pipeline GPU-side — this
        // avoids a 9M-iteration scalar loop in WASM for a 3K paste.
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
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        // Staging texture for straight-alpha upload — sampled by the
        // premultiply shader.
        let temp_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transform-source-staging"),
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

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &temp_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba_data,
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

        // Render the staging texture through the premultiply pipeline into
        // source_texture.
        {
            let temp_view = temp_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let premul_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("transform-source-premul-bg"),
                layout: &self.single_tex_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&temp_view),
                }],
            });

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("transform-source-premul"),
            });
            {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("transform-source-premul-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &source_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                rpass.set_pipeline(&self.premultiply_pipeline);
                rpass.set_bind_group(0, &premul_bg, &[]);
                rpass.draw(0..3, 0..1);
            }
            queue.submit(std::iter::once(encoder.finish()));
        }

        // Uniform buffer (identity matrix initially). Preview pass renders to
        // canvas-sized accumulator: target_offset=0, target_size=canvas_size.
        let uniforms = TransformBlendUniforms {
            inv_row0: [1.0, 0.0, 0.0, 0.0],
            inv_row1: [0.0, 1.0, 0.0, 0.0],
            source_origin: [source_origin.0 as f32, source_origin.1 as f32],
            source_size: [source_width as f32, source_height as f32],
            target_offset: [0.0, 0.0],
            target_size: [canvas_width as f32, canvas_height as f32],
            canvas_size: [canvas_width as f32, canvas_height as f32],
            opacity: 1.0,
            is_r8: 0.0,
            layer_offset: [layer_offset.0 as f32, layer_offset.1 as f32],
            layer_size: [layer_size.0 as f32, layer_size.1 as f32],
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
        layer: &LayerTexture,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        canvas_width: u32,
        canvas_height: u32,
        target_layer: LayerId,
        target_format: wgpu::TextureFormat,
    ) {
        let layer_texture = &layer.texture;
        let layer_offset = (layer.offset_x, layer.offset_y);
        let layer_dims = (layer.width, layer.height);
        let is_r8 = target_format == wgpu::TextureFormat::R8Unorm;

        // Source texture matches the target's format. RGBA8 sources are
        // premultiplied by an extra render pass (straight-alpha source data
        // requires it for correct bilinear interpolation in the transform
        // shaders). R8 sources skip premultiply — single-channel, no alpha.
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
            format: target_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        // GPU→GPU copy from layer texture to source texture.
        // `source_origin` is canvas-space; convert to layer-local pixel
        // coords by subtracting the layer's offset, then clip the read to
        // the layer texture's actual extent (which may differ from canvas).
        let local_src_x_signed = source_origin.0 - layer_offset.0;
        let local_src_y_signed = source_origin.1 - layer_offset.1;
        let src_x = local_src_x_signed.max(0) as u32;
        let src_y = local_src_y_signed.max(0) as u32;
        let copy_w = source_width.min(layer_dims.0.saturating_sub(src_x));
        let copy_h = source_height.min(layer_dims.1.saturating_sub(src_y));
        let dst_x = (-local_src_x_signed).max(0) as u32;
        let dst_y = (-local_src_y_signed).max(0) as u32;
        let _ = (canvas_width, canvas_height); // kept on the signature for the uniform write below.

        let copy_src = wgpu::TexelCopyTextureInfo {
            texture: layer_texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: src_x,
                y: src_y,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        };
        let copy_size = wgpu::Extent3d {
            width: copy_w.min(source_width - dst_x),
            height: copy_h.min(source_height - dst_y),
            depth_or_array_layers: 1,
        };

        if !is_r8 && copy_w > 0 && copy_h > 0 {
            // RGBA: copy layer → temp, then premultiply render to source.
            // Straight-alpha layer data must be converted to premultiplied alpha
            // for correct bilinear interpolation in the transform shaders.
            let temp_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("premultiply-temp"),
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

            encoder.copy_texture_to_texture(
                copy_src,
                wgpu::TexelCopyTextureInfo {
                    texture: &temp_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: dst_x,
                        y: dst_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                copy_size,
            );

            // Render pass: temp (straight) → source (premultiplied).
            let temp_view = temp_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let premul_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("premultiply-bg"),
                layout: &self.single_tex_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&temp_view),
                }],
            });

            let premul_target_view =
                source_texture.create_view(&wgpu::TextureViewDescriptor::default());
            {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("premultiply"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &premul_target_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                rpass.set_pipeline(&self.premultiply_pipeline);
                rpass.set_bind_group(0, &premul_bg, &[]);
                rpass.draw(0..3, 0..1);
            }
        } else if copy_w > 0 && copy_h > 0 {
            // Mask (R8): direct copy, no premultiply needed.
            encoder.copy_texture_to_texture(
                copy_src,
                wgpu::TexelCopyTextureInfo {
                    texture: &source_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: dst_x,
                        y: dst_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                copy_size,
            );
        }

        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Uniform buffer (identity matrix initially). Preview pass renders to
        // canvas-sized accumulator: target_offset=0, target_size=canvas_size.
        // The mask shares the layer's bounds, so layer_offset/layer_size also
        // double as the mask's UV mapping for the preview shader.
        let uniforms = TransformBlendUniforms {
            inv_row0: [1.0, 0.0, 0.0, 0.0],
            inv_row1: [0.0, 1.0, 0.0, 0.0],
            source_origin: [source_origin.0 as f32, source_origin.1 as f32],
            source_size: [source_width as f32, source_height as f32],
            target_offset: [0.0, 0.0],
            target_size: [canvas_width as f32, canvas_height as f32],
            canvas_size: [canvas_width as f32, canvas_height as f32],
            opacity: 1.0,
            is_r8: 0.0,
            layer_offset: [layer_offset.0 as f32, layer_offset.1 as f32],
            layer_size: [layer_dims.0 as f32, layer_dims.1 as f32],
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
        layer_offset: (i32, i32),
        layer_size: (u32, u32),
    ) {
        let state = match &self.active {
            Some(s) => s,
            None => return,
        };

        let inv = affine_inverse(matrix).unwrap_or(IDENTITY);

        // Preview pass renders to canvas-sized accumulator.
        let uniforms = TransformBlendUniforms {
            inv_row0: [inv[0], inv[1], inv[2], 0.0],
            inv_row1: [inv[3], inv[4], inv[5], 0.0],
            source_origin: [source_origin.0 as f32, source_origin.1 as f32],
            source_size: [source_width as f32, source_height as f32],
            target_offset: [0.0, 0.0],
            target_size: [canvas_width as f32, canvas_height as f32],
            canvas_size: [canvas_width as f32, canvas_height as f32],
            opacity: 1.0,
            is_r8: 0.0,
            layer_offset: [layer_offset.0 as f32, layer_offset.1 as f32],
            layer_size: [layer_size.0 as f32, layer_size.1 as f32],
        };

        queue.write_buffer(&state.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Render the transformed source directly onto a target texture (layer or mask).
    /// Used by commit_floating() to replace CPU-side rasterize_to_tiles().
    ///
    /// The destination is copied to a temp texture and the shader computes
    /// Porter-Duff source-over manually, outputting with REPLACE blend. This
    /// avoids the premultiplied-stored-as-straight bug from hardware blending.
    ///
    /// `source_origin` is canvas-space; the target's `(target_offset,
    /// target_width, target_height)` describe its canvas-space placement so
    /// the shader can map UV → canvas coords on offset paste-extent layers.
    pub fn commit_to_texture(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        target_texture: &wgpu::Texture,
        target_view: &wgpu::TextureView,
        target_format: wgpu::TextureFormat,
        matrix: &Affine2D,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        target_offset: (i32, i32),
        target_width: u32,
        target_height: u32,
        canvas_width: u32,
        canvas_height: u32,
    ) {
        let state = match &self.active {
            Some(s) => s,
            None => return,
        };

        let inv = affine_inverse(matrix).unwrap_or(IDENTITY);
        let is_r8 = if target_format == wgpu::TextureFormat::R8Unorm {
            1.0
        } else {
            0.0
        };

        // Reuse the preview uniform struct — `is_r8` selects the commit
        // shader's output channel layout. layer_offset/layer_size are unused
        // by the commit shader.
        let uniforms = TransformBlendUniforms {
            inv_row0: [inv[0], inv[1], inv[2], 0.0],
            inv_row1: [inv[3], inv[4], inv[5], 0.0],
            source_origin: [source_origin.0 as f32, source_origin.1 as f32],
            source_size: [source_width as f32, source_height as f32],
            target_offset: [target_offset.0 as f32, target_offset.1 as f32],
            target_size: [target_width as f32, target_height as f32],
            canvas_size: [canvas_width as f32, canvas_height as f32],
            opacity: 1.0,
            is_r8,
            layer_offset: [0.0, 0.0],
            layer_size: [0.0, 0.0],
        };

        queue.write_buffer(&state.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        // Copy destination for shader-side Porter-Duff (see straight_composite module).
        let dest_bg = super::straight_composite::copy_for_compositing(
            device,
            encoder,
            &self.single_tex_bgl,
            target_texture,
            target_format,
        );

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
        rpass.set_bind_group(1, &dest_bg, &[]);
        rpass.draw(0..3, 0..1);
    }

    /// Remove floating content GPU state.
    pub fn clear(&mut self) {
        self.active = None;
    }

    /// Check if floating content is active and targets the given layer.
    pub fn targets_layer(&self, layer_id: LayerId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|s| s.target_layer == layer_id)
    }
}
