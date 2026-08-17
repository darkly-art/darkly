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
//
// The affine math + the `Transform` record now live in the dependency-free
// `crate::transform` module (the consumer-agnostic helper). Re-exported here so
// the GPU pipeline keeps referring to `gpu::transform::Affine2D` etc. — one
// home for the math, no duplication.
pub use crate::transform::{
    affine_inverse, affine_multiply, affine_rotate, affine_scale, affine_transform,
    affine_translate, mat3_apply, mat3_inverse, Affine2D, Mat3, Transform, IDENTITY, MAT3_IDENTITY,
};

/// Pack the inverse of a projective matrix into three std140-padded rows
/// (`[m00, m01, m02, _]`, `[m10, m11, m12, _]`, `[m20, m21, m22, _]`) for the
/// transform-sampling shaders. A singular matrix (e.g. a corner dragged behind
/// the camera mid-gesture) falls back to identity so the sample stays finite.
///
/// The one home for this packing — shared by the floating commit uniforms
/// ([`TransformBlendUniforms`]) and the void uniform builders. Pass a void's
/// `transform.to_projective()`, or the floating path's already-baked [`Mat3`].
pub fn pack_inv_rows(m: &Mat3) -> [[f32; 4]; 3] {
    let inv = mat3_inverse(m).unwrap_or(MAT3_IDENTITY);
    [
        [inv[0], inv[1], inv[2], 0.0],
        [inv[3], inv[4], inv[5], 0.0],
        [inv[6], inv[7], inv[8], 0.0],
    ]
}

// ---------------------------------------------------------------------------
// FloatingContent — CPU-side data owned by the engine
// ---------------------------------------------------------------------------

/// Type-owned source-clear shape for one interactive transform target.
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
    Selection {
        mask_bind_group: std::sync::Arc<wgpu::BindGroup>,
        uncovered: crate::document::PixelValue,
    },
}

/// How the floating content was created — determines commit/cancel behavior.
pub enum FloatingMode {
    /// Clipboard paste — commit composites INTO target.
    /// `created_layer_id = Some(id)` means the target layer was auto-created
    /// for this paste and should be removed on cancel. `None` means paste
    /// targets a pre-existing layer; cancel is a no-op.
    Paste { created_layer_id: Option<LayerId> },
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
    /// Current user transform — affine (`Basic`) or projective
    /// (`Perspective`). The GPU consumes its [`Transform::to_projective`].
    pub transform: Transform,
    /// Target node id. Resolves to either a raster layer or a mask filter;
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
        let m = self.transform.to_projective();

        // Transform the four corners of the source rectangle (perspective
        // divide). A mid-drag homography can sweep a corner toward `w ≈ 0`,
        // producing ±∞ / NaN; clamp each component so the preview bounds stay
        // finite instead of poisoning the affected-rect math.
        let clamp = |v: f32| {
            if v.is_finite() {
                v.clamp(-1.0e6, 1.0e6)
            } else {
                0.0
            }
        };
        let corners = [
            mat3_apply(&m, 0.0, 0.0),
            mat3_apply(&m, w, 0.0),
            mat3_apply(&m, 0.0, h),
            mat3_apply(&m, w, h),
        ];

        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for (cx, cy) in &corners {
            let cx = clamp(*cx);
            let cy = clamp(*cy);
            min_x = min_x.min(cx);
            min_y = min_y.min(cy);
            max_x = max_x.max(cx);
            max_y = max_y.max(cy);
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

/// Uniforms for the transform-commit shader (96 bytes, std140-aligned).
///
/// One uniform struct; one shader (commit). The preview is now a derived
/// view of the target node's texture, rebuilt by running the same commit
/// shader into a preview texture — no separate preview pipeline.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TransformBlendUniforms {
    /// Inverse homography row 0: `[m00, m01, m02, _pad]`.
    pub inv_row0: [f32; 4],
    /// Inverse homography row 1: `[m10, m11, m12, _pad]`.
    pub inv_row1: [f32; 4],
    /// Inverse homography row 2: `[m20, m21, m22, _pad]`. Affine is the
    /// special case `[0, 0, 1, _]` (perspective divide collapses to `w ≡ 1`).
    pub inv_row2: [f32; 4],
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
    /// Selection coverage stays separate from source values so an unselected
    /// texel cannot alias a selected zero-valued R8 texel. Selected transforms
    /// own this texture for the session; whole-content transforms and pastes
    /// sample the transform pass's shared opaque fallback.
    ///
    /// Representation informed by GIMP contributors' `gimp_selection_extract`
    /// (https://gitlab.gnome.org/GNOME/gimp/-/blob/master/app/core/gimpselection.c)
    /// and Krita contributors' `TransformStrokeStrategy::createDeviceCache`
    /// (https://invent.kde.org/graphics/krita/-/blob/master/plugins/tools/tool_transform2/strokes/transform_stroke_strategy.cpp).
    pub source_coverage_texture: Option<wgpu::Texture>,
    pub source_coverage_view: Option<wgpu::TextureView>,
    pub uniform_buf: wgpu::Buffer,
    /// Bind group for the commit pass (source value + coverage + sampler + uniforms).
    pub commit_bind_group: wgpu::BindGroup,
    pub target_layer: LayerId,
    pub target_format: wgpu::TextureFormat,

    /// Per-target preview texture. Canvas-sized so a translate that drags
    /// content past the source bounding box still has somewhere on the
    /// preview to write — clipped at canvas bounds (the only thing the
    /// viewport renders), not at the live texture's bounds. Owned by this
    /// state — destroyed when floating ends.
    pub preview_texture: wgpu::Texture,
    pub preview_view: wgpu::TextureView,
    /// Bind group sampling `preview_view` against the mask BGL — built
    /// only when the target is R8, so the host's mask sampling can route
    /// through the preview during a mask transform.
    pub preview_mask_bind_group: Option<wgpu::BindGroup>,
    /// Canvas-aligned blend uniforms used by the host blend pass when it
    /// samples the preview. Mirrors the live layer's blend props (opacity,
    /// blend mode, isolated) but overrides `layer_offset = (0, 0)` and
    /// `layer_size = canvas` to match the preview texture's canvas-aligned
    /// extent. Decouples preview's sampling geometry from the live
    /// texture's so the preview can be sized independently.
    pub preview_blend_uniform_buf: wgpu::Buffer,
}

/// Split a straight-alpha RGBA clip into the two channels a mask target needs:
/// `(values, coverage)`.
///
/// A mask texel is a single grayscale value with no alpha of its own, so the
/// clip's own alpha cannot ride along in the pixel — it becomes coverage, which
/// is what the commit shader weights the write by. That is what makes the
/// transparent parts of a clip leave the mask's existing pixels untouched
/// instead of stamping a rectangle over them. GIMP draws the same line,
/// converting a floating paste to the pasted-to drawable's own format *with
/// alpha* (`gimp_edit_paste_get_layers`, app/core/gimp-edit.c).
///
/// Values are emitted as opaque RGBA so they ride the same staging upload and
/// premultiply pass the RGBA path uses — premultiplying by an alpha of 1 is the
/// identity, and the shader reads the red channel for an R8 target. Luminance
/// uses the BT.709 weights shared with `lib/black_and_white.wgsl`.
fn rgba_to_mask_values(rgba: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut values = Vec::with_capacity(rgba.len());
    let mut coverage = Vec::with_capacity(rgba.len() / 4);
    for px in rgba.chunks_exact(4) {
        let luma = (0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32) / 255.0;
        let v = (luma.clamp(0.0, 1.0) * 255.0).round() as u8;
        values.extend_from_slice(&[v, v, v, 255]);
        coverage.push(px[3]);
    }
    (values, coverage)
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
    _opaque_coverage_texture: wgpu::Texture,
    opaque_coverage_view: wgpu::TextureView,
    /// One-target compatibility state used only by paste. Interactive transforms
    /// own explicit states in the compositor's `TransformGpuSession`.
    pub paste: Option<TransformState>,
}

impl TransformPass {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
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
                ],
            });

        // Single-texture BGL shared by dest copy (commit) and premultiply passes.
        let single_tex_bgl = super::straight_composite::single_texture_bind_group_layout(
            device,
            "transform-single-tex-bgl",
        );

        let commit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("transform-commit-layout"),
            bind_group_layouts: &[Some(&commit_bind_group_layout), Some(&single_tex_bgl)],
            immediate_size: 0,
        });

        let commit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("transform-commit-shader"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("../../shaders/source_over.wgsl"),
                    "\n",
                    include_str!("../../shaders/lib/projective.wgsl"),
                    "\n",
                    include_str!("../../shaders/transform_commit.wgsl"),
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
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/premultiply.wgsl").into()),
        });

        let premultiply_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("premultiply-layout"),
            bind_group_layouts: &[Some(&single_tex_bgl)],
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

        let (opaque_coverage_texture, opaque_coverage_view) =
            Self::create_source_coverage(device, queue, 1, 1, &[255], "transform-coverage-opaque");

        TransformPass {
            commit_rgba_pipeline,
            commit_r8_pipeline,
            commit_bind_group_layout,
            single_tex_bgl,
            premultiply_pipeline,
            _opaque_coverage_texture: opaque_coverage_texture,
            opaque_coverage_view,
            paste: None,
        }
    }

    fn create_source_coverage(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        data: &[u8],
        label: &str,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        debug_assert_eq!(data.len(), (width * height) as usize);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
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
        source_coverage: Option<&[u8]>,
        source_width: u32,
        source_height: u32,
        target_layer: LayerId,
        target_format: wgpu::TextureFormat,
        preview_texture: wgpu::Texture,
        preview_view: wgpu::TextureView,
        preview_mask_bind_group: Option<wgpu::BindGroup>,
        preview_blend_uniform_buf: wgpu::Buffer,
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

        // An RGBA clip has to be converted into the target's terms before the
        // commit shader can sample it: for an R8 target that shader reads the
        // red channel, which is a real mask value only once the conversion has
        // happened, and weights the write by coverage, which is where the clip's
        // alpha has to go.
        let mask_conversion =
            (target_format == wgpu::TextureFormat::R8Unorm).then(|| rgba_to_mask_values(rgba_data));

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &temp_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            mask_conversion
                .as_ref()
                .map_or(rgba_data, |(values, _)| values.as_slice()),
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

        // A mask target's coverage is the clip's own alpha, narrowed by whatever
        // coverage the caller already imposed (a selection). An RGBA target's
        // source keeps its alpha channel and the shader's source-over consumes
        // it directly, so there only the caller's coverage applies.
        let mask_coverage = mask_conversion.as_ref().map(|(_, alpha)| {
            source_coverage.map_or_else(
                || alpha.clone(),
                |caller| {
                    alpha
                        .iter()
                        .zip(caller)
                        .map(|(&a, &c)| ((a as u16 * c as u16) / 255) as u8)
                        .collect()
                },
            )
        });
        let source_coverage = mask_coverage.as_deref().or(source_coverage);

        let source_coverage = source_coverage.map(|coverage| {
            Self::create_source_coverage(
                device,
                queue,
                source_width,
                source_height,
                coverage,
                "transform-source-coverage",
            )
        });
        let coverage_view = source_coverage
            .as_ref()
            .map_or(&self.opaque_coverage_view, |(_, view)| view);
        let commit_bind_group =
            self.make_commit_bind_group(device, &source_view, coverage_view, sampler, &uniform_buf);
        let (source_coverage_texture, source_coverage_view) = source_coverage
            .map(|(texture, view)| (Some(texture), Some(view)))
            .unwrap_or((None, None));

        self.paste = Some(TransformState {
            source_texture,
            source_view,
            source_coverage_texture,
            source_coverage_view,
            uniform_buf,
            commit_bind_group,
            target_layer,
            target_format,
            preview_texture,
            preview_view,
            preview_mask_bind_group,
            preview_blend_uniform_buf,
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
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        sampler: &wgpu::Sampler,
        layer: &LayerTexture,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        source_coverage: Option<&[u8]>,
        target_layer: LayerId,
        target_format: wgpu::TextureFormat,
        preview_texture: wgpu::Texture,
        preview_view: wgpu::TextureView,
        preview_mask_bind_group: Option<wgpu::BindGroup>,
        preview_blend_uniform_buf: wgpu::Buffer,
    ) {
        let layer_texture = layer.texture();
        let layer_canvas = layer.canvas_extent();
        let layer_offset = (layer_canvas.x0(), layer_canvas.y0());
        let layer_dims = (layer_canvas.width, layer_canvas.height);
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

        let source_coverage = source_coverage.map(|coverage| {
            Self::create_source_coverage(
                device,
                queue,
                source_width,
                source_height,
                coverage,
                "transform-source-coverage",
            )
        });
        let coverage_view = source_coverage
            .as_ref()
            .map_or(&self.opaque_coverage_view, |(_, view)| view);
        let commit_bind_group =
            self.make_commit_bind_group(device, &source_view, coverage_view, sampler, &uniform_buf);
        let (source_coverage_texture, source_coverage_view) = source_coverage
            .map(|(texture, view)| (Some(texture), Some(view)))
            .unwrap_or((None, None));

        self.paste = Some(TransformState {
            source_texture,
            source_view,
            source_coverage_texture,
            source_coverage_view,
            uniform_buf,
            commit_bind_group,
            target_layer,
            target_format,
            preview_texture,
            preview_view,
            preview_mask_bind_group,
            preview_blend_uniform_buf,
        });
    }

    /// Bind group for the commit pass: source value + coverage + sampler + uniforms.
    fn make_commit_bind_group(
        &self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
        coverage_view: &wgpu::TextureView,
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(coverage_view),
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
        matrix: &Mat3,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        target_offset: (i32, i32),
        target_width: u32,
        target_height: u32,
        canvas_width: u32,
        canvas_height: u32,
    ) {
        let Some(state) = self.paste.as_ref() else {
            return;
        };
        self.update_state_uniforms(
            queue,
            state,
            matrix,
            source_origin,
            source_width,
            source_height,
            target_offset,
            target_width,
            target_height,
            canvas_width,
            canvas_height,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_state_uniforms(
        &self,
        queue: &wgpu::Queue,
        state: &TransformState,
        matrix: &Mat3,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        target_offset: (i32, i32),
        target_width: u32,
        target_height: u32,
        canvas_width: u32,
        canvas_height: u32,
    ) {
        let [inv_row0, inv_row1, inv_row2] = pack_inv_rows(matrix);
        let is_r8 = if state.target_format == wgpu::TextureFormat::R8Unorm {
            1.0
        } else {
            0.0
        };
        let uniforms = TransformBlendUniforms {
            inv_row0,
            inv_row1,
            inv_row2,
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
        let Some(state) = self.paste.as_ref() else {
            return;
        };
        self.render_state_commit(device, encoder, state, target_texture, target_view);
    }

    pub fn render_state_commit(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        state: &TransformState,
        target_texture: &wgpu::Texture,
        target_view: &wgpu::TextureView,
    ) {
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
        self.paste = None;
    }

    pub fn take_paste_state(&mut self) -> Option<TransformState> {
        self.paste.take()
    }

    /// Check if floating content is active and targets the given layer.
    pub fn targets_layer(&self, layer_id: LayerId) -> bool {
        self.paste
            .as_ref()
            .is_some_and(|s| s.target_layer == layer_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::homography_from_corners;

    /// A mask stores a value and no alpha, so an RGBA clip splits into a
    /// luminance value plus a coverage channel carrying the clip's alpha.
    /// Reading the source's red channel instead — which is what the commit
    /// shader does for an R8 target before this conversion — turns any
    /// non-red clip into a black rectangle over the mask.
    #[test]
    fn rgba_to_mask_values_splits_luminance_from_alpha() {
        // Opaque green, opaque white, fully transparent, half-transparent black.
        let src = [
            0, 255, 0, 255, //
            255, 255, 255, 255, //
            0, 255, 0, 0, //
            0, 0, 0, 128,
        ];
        let (values, coverage) = rgba_to_mask_values(&src);

        // BT.709 luminance, independent of alpha — alpha rides `coverage`.
        assert_eq!(values[0], (0.7152f32 * 255.0).round() as u8);
        assert_eq!(values[4], 255);
        assert_eq!(values[8], (0.7152f32 * 255.0).round() as u8);
        assert_eq!(values[12], 0);

        // Values are opaque so the shared premultiply pass is the identity.
        assert!(values.chunks_exact(4).all(|px| px[3] == 255));
        // Gray, so the shader's red-channel read is the value whatever it picks.
        assert!(values
            .chunks_exact(4)
            .all(|px| px[0] == px[1] && px[1] == px[2]));

        // Coverage is the clip's alpha: transparent texels write nothing.
        assert_eq!(coverage, vec![255, 255, 0, 128]);
    }

    /// A near-degenerate matrix (a corner driving the homogeneous `w → 0`,
    /// folding behind the camera) has no usable inverse; `pack_inv_rows` must
    /// fall back to identity rows rather than emit NaN/∞ — the CPU half of the
    /// shader's degenerate guard. The matching shader-side clamp is
    /// `proj_local`'s `abs(hw) < 1e-8 → ok = 0` (transparent).
    #[test]
    fn pack_inv_rows_clamps_degenerate_to_identity() {
        // Singular: bottom-right ≈ 0 collapses the determinant.
        let singular: Mat3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1e-13];
        let rows = pack_inv_rows(&singular);
        assert_eq!(rows[0], [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(rows[1], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(rows[2], [0.0, 0.0, 1.0, 0.0]);
    }

    /// A real perspective homography packs to finite inverse rows with the
    /// std140 padding word zeroed.
    #[test]
    fn pack_inv_rows_packs_finite_perspective() {
        let corners = [(16.0, 0.0), (48.0, 0.0), (64.0, 64.0), (0.0, 64.0)];
        let m = homography_from_corners(64.0, 64.0, corners).expect("non-degenerate");
        let rows = pack_inv_rows(&m);
        for row in rows {
            for v in row {
                assert!(v.is_finite(), "inverse row component must be finite");
            }
            assert_eq!(row[3], 0.0, "padding word stays zero");
        }
    }
}
