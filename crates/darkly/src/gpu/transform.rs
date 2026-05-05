//! Floating content GPU pipeline — source-texture management + the commit
//! render pass that writes transformed pixels into a target texture.
//!
//! Used by both paste-in-place and the interactive transform tool. The
//! interactive preview is **not** a separate render path: the compositor
//! maintains a per-target preview texture rebuilt by re-running the same
//! commit shader after each matrix update, and the host's blend pass reads
//! through `effective_*` accessors so the preview composes naturally
//! without a parallel pipeline.

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
    /// Extracted from a layer — commit writes transformed pixels into the
    /// live target after applying `clear_shape` to the source rect.
    /// Cancel is a no-op on the texture: setup_transform doesn't touch
    /// the live target, so there's nothing to restore.
    Transform {
        /// Shape of the source-rect clear that commit applies before the
        /// transform render. The same shape is also applied to the
        /// per-update preview texture so the preview matches what commit
        /// will write.
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

/// Uniforms for the transform-commit shader (80 bytes, std140-aligned).
///
/// One uniform struct; one shader (commit). The preview is now a derived
/// view of the target node's texture, rebuilt by running the same commit
/// shader into a preview texture — no separate preview pipeline.
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
    /// Format flag (0.0 = RGBA, 1.0 = R8). The shader uses this to pick the
    /// output channel layout — it's a format property, not a mask concept.
    pub is_r8: f32,
}

/// GPU resources for an active floating content.
///
/// The "preview" — what the canvas would show if commit ran right now — is
/// a derived view of the target's texture: each time the matrix updates,
/// `render_preview` rebuilds `preview_texture` from a copy of the live
/// target plus the commit shader at the current matrix. The compositor's
/// `effective_*` accessors transparently swap the live view/mask bind
/// group for the preview equivalents, so the host's normal blend pass
/// renders the floating preview without any extra render path.
pub struct TransformState {
    pub source_texture: wgpu::Texture,
    pub source_view: wgpu::TextureView,
    pub uniform_buf: wgpu::Buffer,
    /// Bind group for the commit pass (source + sampler + uniforms).
    pub commit_bind_group: wgpu::BindGroup,
    pub target_layer: LayerId,
    pub target_format: wgpu::TextureFormat,

    /// Per-target preview texture, sized and formatted to match the live
    /// target. Owned by this state — destroyed when floating ends.
    pub preview_texture: wgpu::Texture,
    pub preview_view: wgpu::TextureView,
    /// Bind group sampling `preview_view` against the mask BGL — built
    /// only when the target is R8, so the host's mask sampling can route
    /// through the preview during a mask transform.
    pub preview_mask_bind_group: Option<wgpu::BindGroup>,
}

/// GPU pipelines for the floating-content commit pass + optional active state.
pub struct TransformPass {
    /// Commit pipelines: render transform directly into a target texture.
    /// The same pipelines drive both real commits (writing to the live
    /// target) and per-update preview renders (writing to the preview
    /// texture).
    commit_rgba_pipeline: wgpu::RenderPipeline,
    commit_r8_pipeline: wgpu::RenderPipeline,
    commit_bind_group_layout: wgpu::BindGroupLayout,
    /// Single-texture BGL used for dest copy (commit) and premultiply passes.
    single_tex_bgl: wgpu::BindGroupLayout,
    premultiply_pipeline: wgpu::RenderPipeline,
    pub active: Option<TransformState>,
}

impl TransformPass {
    pub fn new(device: &wgpu::Device) -> Self {
        // --- Commit pipelines (render directly to a target texture) ---
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
            commit_rgba_pipeline,
            commit_r8_pipeline,
            commit_bind_group_layout,
            single_tex_bgl,
            premultiply_pipeline,
            active: None,
        }
    }

    /// Build the source texture + uniforms + commit bind group for a paste.
    ///
    /// `rgba_data` must be `source_width * source_height * 4` bytes, row-major,
    /// in straight alpha. The pixel data is uploaded to a staging texture and
    /// premultiplied via `premultiply_pipeline` so bilinear sampling during
    /// transform produces correct edge blending.
    ///
    /// `preview_*` parameters are owned by the caller (the compositor builds
    /// them sized to match the live target's `LayerTexture`). They live on
    /// `TransformState` for the duration of the floating session and are
    /// dropped when `clear()` runs.
    #[allow(clippy::too_many_arguments)]
    pub fn set_floating_content(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: &wgpu::Sampler,
        rgba_data: &[u8],
        source_width: u32,
        source_height: u32,
        target_layer: LayerId,
        target_format: wgpu::TextureFormat,
        preview_texture: wgpu::Texture,
        preview_view: wgpu::TextureView,
        preview_mask_bind_group: Option<wgpu::BindGroup>,
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
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

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

        // Render the staging texture through the premultiply pipeline.
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

        // Allocate the uniform buffer; caller will fill it via `update_uniforms`.
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transform-uniforms"),
            size: std::mem::size_of::<TransformBlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let commit_bind_group =
            self.make_commit_bind_group(device, &source_view, sampler, &uniform_buf);

        self.active = Some(TransformState {
            source_texture,
            source_view,
            uniform_buf,
            commit_bind_group,
            target_layer,
            target_format,
            preview_texture,
            preview_view,
            preview_mask_bind_group,
        });
    }

    /// Build the source texture by GPU-copying a region from a layer's
    /// texture. Used by interactive transform on existing pixels.
    ///
    /// `target_format` matches the layer's format. RGBA8 sources are
    /// premultiplied (straight-alpha layer data needs premul for correct
    /// bilinear interpolation in the commit shader). R8 (mask) sources skip
    /// premultiply — single-channel, no alpha.
    #[allow(clippy::too_many_arguments)]
    pub fn set_floating_content_from_gpu(
        &mut self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        sampler: &wgpu::Sampler,
        layer: &LayerTexture,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        target_layer: LayerId,
        target_format: wgpu::TextureFormat,
        preview_texture: wgpu::Texture,
        preview_view: wgpu::TextureView,
        preview_mask_bind_group: Option<wgpu::BindGroup>,
    ) {
        let layer_texture = &layer.texture;
        let layer_offset = (layer.offset_x, layer.offset_y);
        let layer_dims = (layer.width, layer.height);
        let is_r8 = target_format == wgpu::TextureFormat::R8Unorm;

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

        // GPU→GPU copy: canvas-space `source_origin` → layer-local pixel coords.
        let local_src_x_signed = source_origin.0 - layer_offset.0;
        let local_src_y_signed = source_origin.1 - layer_offset.1;
        let src_x = local_src_x_signed.max(0) as u32;
        let src_y = local_src_y_signed.max(0) as u32;
        let copy_w = source_width.min(layer_dims.0.saturating_sub(src_x));
        let copy_h = source_height.min(layer_dims.1.saturating_sub(src_y));
        let dst_x = (-local_src_x_signed).max(0) as u32;
        let dst_y = (-local_src_y_signed).max(0) as u32;

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
            width: copy_w.min(source_width.saturating_sub(dst_x)),
            height: copy_h.min(source_height.saturating_sub(dst_y)),
            depth_or_array_layers: 1,
        };

        if !is_r8 && copy_size.width > 0 && copy_size.height > 0 {
            // RGBA: copy → temp, then premultiply render to source.
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
        } else if copy_size.width > 0 && copy_size.height > 0 {
            // Mask (R8): direct copy, no premultiply.
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

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transform-uniforms"),
            size: std::mem::size_of::<TransformBlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let commit_bind_group =
            self.make_commit_bind_group(device, &source_view, sampler, &uniform_buf);

        self.active = Some(TransformState {
            source_texture,
            source_view,
            uniform_buf,
            commit_bind_group,
            target_layer,
            target_format,
            preview_texture,
            preview_view,
            preview_mask_bind_group,
        });
    }

    /// Bind group for the commit pass: source + sampler + uniforms.
    fn make_commit_bind_group(
        &self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        uniform_buf: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("transform-commit-bg"),
            layout: &self.commit_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
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
    }

    /// Update the uniform buffer for the current matrix + target geometry.
    /// Used by both `commit_to_texture` (writes into the live target) and
    /// preview rendering (writes into the preview texture). The uniform
    /// `target_offset` / `target_size` describe where on the canvas the
    /// render target's pixels live, so the shader can map UV→canvas coords
    /// for paste-extent (offset/oversized) targets.
    #[allow(clippy::too_many_arguments)]
    pub fn update_uniforms(
        &self,
        queue: &wgpu::Queue,
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
        let Some(state) = self.active.as_ref() else {
            return;
        };
        let inv = affine_inverse(matrix).unwrap_or(IDENTITY);
        let is_r8 = if state.target_format == wgpu::TextureFormat::R8Unorm {
            1.0
        } else {
            0.0
        };
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
        };
        queue.write_buffer(&state.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Run the commit shader, writing the transformed source into
    /// `target_view`. Caller is responsible for `update_uniforms` first.
    /// Used both for real commits (writing to the live target) and for
    /// preview renders (writing to the preview texture).
    ///
    /// The destination is copied to a temp via `copy_for_compositing` so
    /// the shader can do straight-alpha source-over without feedback. The
    /// pipeline is REPLACE-blend; the shader picks the output channel
    /// layout off the `is_r8` uniform.
    pub fn render_commit(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target_texture: &wgpu::Texture,
        target_view: &wgpu::TextureView,
    ) {
        let Some(state) = self.active.as_ref() else {
            return;
        };

        let dest_bg = super::straight_composite::copy_for_compositing(
            device,
            encoder,
            &self.single_tex_bgl,
            target_texture,
            state.target_format,
        );

        let pipeline = match state.target_format {
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
