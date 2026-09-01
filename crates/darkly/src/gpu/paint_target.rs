//! GPU paint target: a texture you can paint on via GPU render passes.
//!
//! Works for both RGBA8 layer textures and R8 mask textures.
//! Each operation is a self-contained render pass, with no persistent state between calls.

use crate::coord::CanvasRect;
use crate::gpu::atlas::{CanvasFrame, LayerTexture};

struct PaintUniformChunk {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    slots: u32,
    next: u32,
}

/// One command buffer together with immutable, draw-local paint uniforms.
pub struct PaintCommandEncoder<'a> {
    encoder: wgpu::CommandEncoder,
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    pipelines: &'a PaintPipelines,
    stride: u64,
    initial_slots: u32,
    chunks: Vec<PaintUniformChunk>,
}

impl<'a> PaintCommandEncoder<'a> {
    pub fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        pipelines: &'a PaintPipelines,
        label: &'static str,
        initial_slots: usize,
    ) -> Self {
        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let size = std::mem::size_of::<PaintUniforms>() as u64;
        let stride = size.div_ceil(alignment).max(1) * alignment;
        Self {
            encoder: device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) }),
            device,
            queue,
            pipelines,
            stride,
            initial_slots: initial_slots.max(1) as u32,
            chunks: Vec::new(),
        }
    }

    pub fn with_raw<R>(&mut self, f: impl FnOnce(&mut wgpu::CommandEncoder) -> R) -> R {
        f(&mut self.encoder)
    }

    fn reserve_uniform(&mut self, uniforms: &PaintUniforms) -> (usize, u32) {
        let needs_chunk = self
            .chunks
            .last()
            .is_none_or(|chunk| chunk.next == chunk.slots);
        if needs_chunk {
            let slots = self
                .chunks
                .last()
                .map_or(self.initial_slots, |chunk| chunk.slots.saturating_mul(2));
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("paint-command-uniforms"),
                size: self.stride * u64::from(slots),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group =
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("paint-command-uniform-bg"),
                    layout: &self.pipelines.uniform_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buffer,
                            offset: 0,
                            size: std::num::NonZeroU64::new(
                                std::mem::size_of::<PaintUniforms>() as u64
                            ),
                        }),
                    }],
                });
            self.chunks.push(PaintUniformChunk {
                buffer,
                bind_group,
                slots,
                next: 0,
            });
        }
        let index = self.chunks.len() - 1;
        let chunk = &mut self.chunks[index];
        let offset = u64::from(chunk.next) * self.stride;
        chunk.next += 1;
        self.queue
            .write_buffer(&chunk.buffer, offset, bytemuck::bytes_of(uniforms));
        (index, offset as u32)
    }

    pub fn submit(self) {
        self.queue.submit([self.encoder.finish()]);
    }
}

/// A GPU texture you can paint on. Lightweight handle (no owned GPU state).
///
/// All coordinate-bearing fields are private. Callers go through the typed
/// accessors ([`canvas_extent`], [`layer_extent`], [`canvas_size`]) so the
/// canvas/layer-local distinction lives in the type system rather than in
/// convention.
///
/// Brush coordinates are passed in **canvas space**. Vertex-stage NDC
/// mapping uses the target's pixel dimensions ([`layer_extent`]) and
/// canvas-space offset ([`canvas_extent`]). Fragment-stage selection
/// sampling uses the document canvas size ([`canvas_size`]) so off-canvas
/// pixels sample outside the selection texture and clamp/wrap correctly.
///
/// [`canvas_extent`]: GpuPaintTarget::canvas_extent
/// [`layer_extent`]: GpuPaintTarget::layer_extent
/// [`canvas_size`]: GpuPaintTarget::canvas_size
#[derive(Copy, Clone)]
pub struct GpuPaintTarget<'a> {
    texture: &'a wgpu::Texture,
    view: &'a wgpu::TextureView,
    format: wgpu::TextureFormat,
    /// Texture pixel dimensions. Exposed via [`layer_extent`](Self::layer_extent).
    width: u32,
    height: u32,
    /// Canvas-space offset of pixel (0, 0). Exposed via [`canvas_extent`](Self::canvas_extent).
    offset_x: i32,
    offset_y: i32,
    /// Document canvas size: used for fragment-stage selection UV.
    /// Exposed via [`canvas_size`](Self::canvas_size).
    canvas_width: u32,
    canvas_height: u32,
    /// Plane-space offset of the canvas window (`Document::canvas_origin`).
    /// The selection mask is a window-sized texture anchored here, so the
    /// fragment shader maps a plane position `p` to selection UV via
    /// `(p - canvas_origin) / canvas_size`. `(0, 0)` for an un-cropped doc.
    canvas_origin_x: i32,
    canvas_origin_y: i32,
}

impl<'a> GpuPaintTarget<'a> {
    /// Wrap any node texture as a paint target. The texture's own format
    /// drives all downstream pipeline dispatch (R8 mask vs RGBA layer).
    /// Replaces `from_layer` / `from_mask`: callers no longer dispatch on
    /// node kind, only on the texture they hand in.
    pub fn from_node(tex: &'a LayerTexture, canvas: CanvasRect) -> Self {
        let extent = tex.canvas_extent();
        GpuPaintTarget {
            texture: tex.texture(),
            view: tex.view(),
            format: tex.format(),
            width: extent.width,
            height: extent.height,
            offset_x: extent.origin.x,
            offset_y: extent.origin.y,
            canvas_width: canvas.width,
            canvas_height: canvas.height,
            canvas_origin_x: canvas.origin.x,
            canvas_origin_y: canvas.origin.y,
        }
    }

    /// Wrap a canvas-aligned texture (e.g. the floating preview, the selection
    /// mask) as a paint target. The target's extent matches the canvas: origin
    /// `(0, 0)`, size `(canvas_width, canvas_height)`.
    ///
    /// Use [`from_node`](Self::from_node) for layer textures, which may be
    /// offset or larger than canvas.
    pub fn from_canvas_texture(
        texture: &'a wgpu::Texture,
        view: &'a wgpu::TextureView,
        format: wgpu::TextureFormat,
        canvas: CanvasRect,
    ) -> Self {
        // A canvas-window-sized texture: its pixel (0, 0) sits at the window's
        // plane origin, so its extent *is* the canvas window rect.
        Self::from_extent(texture, view, format, canvas, canvas)
    }

    /// Wrap a texture sitting at an explicit canvas extent. Lower-level
    /// constructor used by the test helpers and the paste-extent
    /// allocation path; production layer code prefers
    /// [`from_node`](Self::from_node).
    pub fn from_extent(
        texture: &'a wgpu::Texture,
        view: &'a wgpu::TextureView,
        format: wgpu::TextureFormat,
        canvas_extent: CanvasRect,
        canvas: CanvasRect,
    ) -> Self {
        GpuPaintTarget {
            texture,
            view,
            format,
            width: canvas_extent.width,
            height: canvas_extent.height,
            offset_x: canvas_extent.x0(),
            offset_y: canvas_extent.y0(),
            canvas_width: canvas.width,
            canvas_height: canvas.height,
            canvas_origin_x: canvas.origin.x,
            canvas_origin_y: canvas.origin.y,
        }
    }

    // ----- Typed accessors -----

    pub fn texture(&self) -> &'a wgpu::Texture {
        self.texture
    }

    pub fn view(&self) -> &'a wgpu::TextureView {
        self.view
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Texture-local extent: always at origin (0, 0).
    pub fn layer_extent(&self) -> crate::coord::LayerRect {
        crate::coord::LayerRect::from_xywh(0, 0, self.width, self.height)
    }

    /// Canvas-space rect this target occupies.
    pub fn canvas_extent(&self) -> CanvasRect {
        CanvasRect::from_xywh(self.offset_x, self.offset_y, self.width, self.height)
    }

    /// Document canvas dimensions in pixels: used to compute fragment-stage
    /// selection UV. Distinct from the target's own extent: paste-extent
    /// and grown layers occupy a `canvas_extent` different from `canvas_size`.
    pub fn canvas_size(&self) -> (u32, u32) {
        (self.canvas_width, self.canvas_height)
    }

    /// Plane-space offset of the canvas window: the anchor of the
    /// window-sized selection mask. Fed to shaders so a plane position maps to
    /// selection UV via `(p - canvas_origin) / canvas_size`.
    pub fn canvas_origin(&self) -> (i32, i32) {
        (self.canvas_origin_x, self.canvas_origin_y)
    }

    /// Borrow this target as a `CanvasFrame` for region-store APIs.
    pub fn canvas_frame(&self) -> CanvasFrame<'a> {
        CanvasFrame {
            texture: self.texture,
            canvas_extent: self.canvas_extent(),
        }
    }

    /// Paint a soft circle onto the target via alpha-over blending.
    pub fn composite_circle(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        cx: f32,
        cy: f32,
        radius: f32,
        color: [u8; 4],
        opacity: f32,
    ) {
        let pipeline = pipelines.composite_pipeline(self.format);
        self.draw_circle(
            encoder, pipeline, pipelines, queue, cx, cy, radius, color, opacity, None,
        );
    }

    /// Paint a soft circle with a custom selection mask bind group.
    pub fn composite_circle_with_selection(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        cx: f32,
        cy: f32,
        radius: f32,
        color: [u8; 4],
        opacity: f32,
        selection_bind_group: &wgpu::BindGroup,
    ) {
        let pipeline = pipelines.composite_pipeline(self.format);
        self.draw_circle(
            encoder,
            pipeline,
            pipelines,
            queue,
            cx,
            cy,
            radius,
            color,
            opacity,
            Some(selection_bind_group),
        );
    }

    /// Fill a canvas-space rect with a solid color via alpha-over blending.
    /// `rect` is in canvas pixel coordinates; origin may be negative on
    /// paste-extent layers.
    pub fn fill_rect(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        rect: CanvasRect,
        color: [u8; 4],
    ) {
        self.fill_rect_inner(encoder, pipelines, queue, rect, color, None);
    }

    /// Fill a canvas-space rect with a solid color, masked by a selection
    /// bind group. Used by flood fill: the fill mask texture is bound as the
    /// "selection".
    pub fn fill_rect_with_selection(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        rect: CanvasRect,
        color: [u8; 4],
        selection_bind_group: &wgpu::BindGroup,
    ) {
        self.fill_rect_inner(
            encoder,
            pipelines,
            queue,
            rect,
            color,
            Some(selection_bind_group),
        );
    }

    /// Set selected pixels to an entity's uncovered value.
    pub fn clear_with_selection(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        selection_bind_group: &wgpu::BindGroup,
        uncovered: crate::document::PixelValue,
    ) {
        match uncovered {
            crate::document::PixelValue::Transparent => {
                self.erase_with_selection(encoder, pipelines, queue, selection_bind_group)
            }
            crate::document::PixelValue::White => self.fill_rect_with_selection(
                encoder,
                pipelines,
                queue,
                self.canvas_extent(),
                [255, 255, 255, 255],
                selection_bind_group,
            ),
        }
    }

    /// Erase pixels within a selection mask. Full-canvas erase modulated by the
    /// selection texture: used for clear_selection_contents.
    pub fn erase_with_selection(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        selection_bind_group: &wgpu::BindGroup,
    ) {
        let pipeline = pipelines.erase_pipeline(self.format);

        let uniforms = PaintUniforms {
            origin: [self.offset_x as f32, self.offset_y as f32],
            size: [self.width as f32, self.height as f32],
            target_offset: [self.offset_x as f32, self.offset_y as f32],
            target_size: [self.width as f32, self.height as f32],
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            canvas_origin: [self.canvas_origin_x as f32, self.canvas_origin_y as f32],
            center: [0.0, 0.0],
            radius: 0.0, // solid fill: coverage from selection only
            softness: 0.0,
            color: [1.0, 1.0, 1.0, 1.0], // full erase strength
            mask_offset: [0.0, 0.0],
            mask_size: [0.0, 0.0],
        };

        self.execute_pass(
            encoder,
            pipeline,
            pipelines,
            queue,
            &uniforms,
            Some(selection_bind_group),
        );
    }

    /// Multiply ALL channels of the target by a mask texture.
    ///
    /// `dst.rgba *= mask_sample`: produces premultiplied output. Use this when
    /// the result will be sampled with bilinear filtering (e.g. transform sources),
    /// where premultiplied data is required for correct interpolation at alpha
    /// edges (see docs/lessons-learned/compositing-lessons-learned.md §2).
    ///
    /// **Do not use for straight-alpha destinations** (layer textures, clipboard
    /// staging). Use `multiply_alpha_by_mask` instead; it preserves RGB and only
    /// scales the alpha channel, which is correct for straight-alpha storage.
    pub fn multiply_by_mask(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        mask_bind_group: &wgpu::BindGroup,
    ) {
        let pipeline = pipelines.mask_multiply_pipeline(self.format);

        // Full-target rect, color = black with full alpha.
        // The shader outputs (0, 0, 0, mask_sample) and the blend state
        // computes dst * SrcAlpha = dst * mask_sample.
        let uniforms = PaintUniforms {
            origin: [self.offset_x as f32, self.offset_y as f32],
            size: [self.width as f32, self.height as f32],
            target_offset: [self.offset_x as f32, self.offset_y as f32],
            target_size: [self.width as f32, self.height as f32],
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            canvas_origin: [self.canvas_origin_x as f32, self.canvas_origin_y as f32],
            center: [0.0, 0.0],
            radius: 0.0,
            softness: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
            mask_offset: [0.0, 0.0],
            mask_size: [0.0, 0.0],
        };

        self.execute_pass(
            encoder,
            pipeline,
            pipelines,
            queue,
            &uniforms,
            Some(mask_bind_group),
        );
    }

    /// Multiply ALL channels of the target by `(1 - mask)`.
    ///
    /// `dst.rgba *= (1 - mask_sample)`: produces premultiplied output.
    /// Same caveat as `multiply_by_mask`: do not use for straight-alpha
    /// destinations. Use `multiply_alpha_by_inverse_mask` instead.
    pub fn multiply_by_inverse_mask(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        mask_bind_group: &wgpu::BindGroup,
    ) {
        let pipeline = pipelines.inverse_mask_multiply_pipeline(self.format);

        let uniforms = PaintUniforms {
            origin: [self.offset_x as f32, self.offset_y as f32],
            size: [self.width as f32, self.height as f32],
            target_offset: [self.offset_x as f32, self.offset_y as f32],
            target_size: [self.width as f32, self.height as f32],
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            canvas_origin: [self.canvas_origin_x as f32, self.canvas_origin_y as f32],
            center: [0.0, 0.0],
            radius: 0.0,
            softness: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
            mask_offset: [0.0, 0.0],
            mask_size: [0.0, 0.0],
        };

        self.execute_pass(
            encoder,
            pipeline,
            pipelines,
            queue,
            &uniforms,
            Some(mask_bind_group),
        );
    }

    /// Multiply only the ALPHA channel of the target by a mask texture.
    ///
    /// `dst.a *= mask_sample`, `dst.rgb` unchanged. Correct for straight-alpha
    /// destinations (layer textures, clipboard staging) where the color channels
    /// represent the actual color independent of opacity. See
    /// docs/lessons-learned/compositing-lessons-learned.md §1: in straight alpha, coverage scaling
    /// only affects the alpha channel.
    pub fn multiply_alpha_by_mask(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        mask_bind_group: &wgpu::BindGroup,
    ) {
        let pipeline = pipelines.alpha_mask_multiply_pipeline(self.format);

        let uniforms = PaintUniforms {
            origin: [self.offset_x as f32, self.offset_y as f32],
            size: [self.width as f32, self.height as f32],
            target_offset: [self.offset_x as f32, self.offset_y as f32],
            target_size: [self.width as f32, self.height as f32],
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            canvas_origin: [self.canvas_origin_x as f32, self.canvas_origin_y as f32],
            center: [0.0, 0.0],
            radius: 0.0,
            softness: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
            mask_offset: [0.0, 0.0],
            mask_size: [0.0, 0.0],
        };

        self.execute_pass(
            encoder,
            pipeline,
            pipelines,
            queue,
            &uniforms,
            Some(mask_bind_group),
        );
    }

    /// Multiply only the ALPHA channel of the target by a mask sampled in the
    /// mask's OWN plane-anchored frame (`mask_frame`), revealing `1.0` outside
    /// the mask footprint. `dst.a *= mask`, `dst.rgb` unchanged.
    ///
    /// The destructive-bake sibling of [`multiply_alpha_by_mask`], which assumes
    /// the bound texture is a canvas-window-sized selection mask. A mask *filter*
    /// texture lives in its own extent (`mask_frame`), so it must be addressed
    /// there and revealed outside its bounds, exactly matching the display path's
    /// `sample_mask_plane` semantics. Used by `apply_mask` so the baked result
    /// matches the live composite.
    ///
    /// [`multiply_alpha_by_mask`]: Self::multiply_alpha_by_mask
    pub fn multiply_alpha_by_mask_in_frame(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        mask_bind_group: &wgpu::BindGroup,
        mask_frame: CanvasRect,
    ) {
        let pipeline = pipelines.alpha_mask_multiply_in_frame_pipeline();

        let uniforms = PaintUniforms {
            origin: [self.offset_x as f32, self.offset_y as f32],
            size: [self.width as f32, self.height as f32],
            target_offset: [self.offset_x as f32, self.offset_y as f32],
            target_size: [self.width as f32, self.height as f32],
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            canvas_origin: [self.canvas_origin_x as f32, self.canvas_origin_y as f32],
            center: [0.0, 0.0],
            radius: 0.0,
            softness: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
            mask_offset: [mask_frame.x0() as f32, mask_frame.y0() as f32],
            mask_size: [mask_frame.width as f32, mask_frame.height as f32],
        };

        self.execute_pass(
            encoder,
            pipeline,
            pipelines,
            queue,
            &uniforms,
            Some(mask_bind_group),
        );
    }

    /// Multiply only the ALPHA channel of the target by `(1 - mask)`.
    ///
    /// `dst.a *= (1 - mask_sample)`, `dst.rgb` unchanged. Straight-alpha
    /// complement of `multiply_alpha_by_mask`. Used by cut-erase to reduce
    /// opacity at selected pixels without darkening the color.
    pub fn multiply_alpha_by_inverse_mask(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        mask_bind_group: &wgpu::BindGroup,
    ) {
        let pipeline = pipelines.alpha_inverse_mask_multiply_pipeline(self.format);

        let uniforms = PaintUniforms {
            origin: [self.offset_x as f32, self.offset_y as f32],
            size: [self.width as f32, self.height as f32],
            target_offset: [self.offset_x as f32, self.offset_y as f32],
            target_size: [self.width as f32, self.height as f32],
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            canvas_origin: [self.canvas_origin_x as f32, self.canvas_origin_y as f32],
            center: [0.0, 0.0],
            radius: 0.0,
            softness: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
            mask_offset: [0.0, 0.0],
            mask_size: [0.0, 0.0],
        };

        self.execute_pass(
            encoder,
            pipeline,
            pipelines,
            queue,
            &uniforms,
            Some(mask_bind_group),
        );
    }

    /// Clear a canvas-space rect to transparent (RGBA) or full reveal (R8).
    /// `rect` is in canvas pixel coordinates; origin may be negative on
    /// paste-extent layers.
    pub fn clear_rect(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        rect: CanvasRect,
    ) {
        let pipeline = pipelines.clear_pipeline(self.format);

        let color = match self.format {
            wgpu::TextureFormat::R8Unorm => [1.0, 1.0, 1.0, 1.0], // 255 = reveal all
            _ => [0.0, 0.0, 0.0, 0.0],                            // transparent
        };

        let uniforms = PaintUniforms {
            origin: [rect.x0() as f32, rect.y0() as f32],
            size: [rect.width as f32, rect.height as f32],
            target_offset: [self.offset_x as f32, self.offset_y as f32],
            target_size: [self.width as f32, self.height as f32],
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            canvas_origin: [self.canvas_origin_x as f32, self.canvas_origin_y as f32],
            center: [0.0, 0.0],
            radius: 0.0,
            softness: 0.0,
            color,
            mask_offset: [0.0, 0.0],
            mask_size: [0.0, 0.0],
        };

        self.execute_pass(encoder, pipeline, pipelines, queue, &uniforms, None);
    }

    /// Render a linear gradient on the target. Selection masking optional.
    pub fn linear_gradient(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        color0: [u8; 4],
        color1: [u8; 4],
        selection: Option<&wgpu::BindGroup>,
    ) {
        let pipeline = pipelines.gradient_pipeline(self.format);

        let uniforms = GradientUniforms {
            origin: [self.offset_x as f32, self.offset_y as f32],
            size: [self.width as f32, self.height as f32],
            target_offset: [self.offset_x as f32, self.offset_y as f32],
            target_size: [self.width as f32, self.height as f32],
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            canvas_origin: [self.canvas_origin_x as f32, self.canvas_origin_y as f32],
            start: [x0, y0],
            end: [x1, y1],
            color0: color_to_float(color0, 1.0),
            color1: color_to_float(color1, 1.0),
        };

        queue.write_buffer(
            &pipelines.gradient_uniform_buf,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        let sel = selection.unwrap_or(&pipelines.default_selection_bind_group);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("paint-gradient"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: self.view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });

        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &pipelines.gradient_uniform_bind_group, &[0]);
        pass.set_bind_group(1, sel, &[]);
        pass.draw(0..3, 0..1);
    }

    // --- Internal ---

    fn fill_rect_inner(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        rect: CanvasRect,
        color: [u8; 4],
        selection: Option<&wgpu::BindGroup>,
    ) {
        let pipeline = pipelines.composite_pipeline(self.format);

        let uniforms = PaintUniforms {
            origin: [rect.x0() as f32, rect.y0() as f32],
            size: [rect.width as f32, rect.height as f32],
            target_offset: [self.offset_x as f32, self.offset_y as f32],
            target_size: [self.width as f32, self.height as f32],
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            canvas_origin: [self.canvas_origin_x as f32, self.canvas_origin_y as f32],
            center: [0.0, 0.0],
            radius: 0.0, // solid fill: no SDF
            softness: 0.0,
            color: color_to_float(color, 1.0),
            mask_offset: [0.0, 0.0],
            mask_size: [0.0, 0.0],
        };

        self.execute_pass(encoder, pipeline, pipelines, queue, &uniforms, selection);
    }

    fn draw_circle(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipeline: &wgpu::RenderPipeline,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        cx: f32,
        cy: f32,
        radius: f32,
        color: [u8; 4],
        opacity: f32,
        selection: Option<&wgpu::BindGroup>,
    ) {
        // Inputs are canvas-space. Pad the quad by softness + 1 pixel so the
        // SDF falloff isn't clipped, then clamp to the layer's canvas extent.
        let softness = 1.0_f32;
        let pad = softness + 1.0;
        let layer_x0 = self.offset_x as f32;
        let layer_y0 = self.offset_y as f32;
        let layer_x1 = layer_x0 + self.width as f32;
        let layer_y1 = layer_y0 + self.height as f32;
        let x0 = (cx - radius - pad).max(layer_x0);
        let y0 = (cy - radius - pad).max(layer_y0);
        let x1 = (cx + radius + pad).min(layer_x1);
        let y1 = (cy + radius + pad).min(layer_y1);

        let uniforms = PaintUniforms {
            origin: [x0, y0],
            size: [x1 - x0, y1 - y0],
            target_offset: [self.offset_x as f32, self.offset_y as f32],
            target_size: [self.width as f32, self.height as f32],
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            canvas_origin: [self.canvas_origin_x as f32, self.canvas_origin_y as f32],
            center: [cx, cy],
            radius,
            softness,
            color: color_to_float(color, opacity),
            mask_offset: [0.0, 0.0],
            mask_size: [0.0, 0.0],
        };

        self.execute_pass(encoder, pipeline, pipelines, queue, &uniforms, selection);
    }

    fn execute_pass(
        &self,
        encoder: &mut PaintCommandEncoder<'_>,
        pipeline: &wgpu::RenderPipeline,
        pipelines: &PaintPipelines,
        _queue: &wgpu::Queue,
        uniforms: &PaintUniforms,
        selection: Option<&wgpu::BindGroup>,
    ) {
        let (chunk_index, offset) = encoder.reserve_uniform(uniforms);
        let sel = selection.unwrap_or(&pipelines.default_selection_bind_group);
        let chunk = &encoder.chunks[chunk_index];
        let mut pass = encoder
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("paint-target"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });

        // Viewport must match the unpadded canvas size so NDC [-1,1] maps to
        // [0, canvas_w] × [0, canvas_h]. Without this, the padded texture dimensions
        // stretch the coordinate space, causing a per-pixel offset that grows from
        // 0 at the origin to (padded - unpadded) at the far edge.
        pass.set_viewport(0.0, 0.0, self.width as f32, self.height as f32, 0.0, 1.0);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &chunk.bind_group, &[offset]);
        pass.set_bind_group(1, sel, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Pre-built render pipelines for paint operations.
///
/// Pipeline variants: {composite, erase, clear} × {RGBA8, R8} for circle/rect ops,
/// plus {gradient} × {RGBA8, R8} with replace blend.
pub struct PaintPipelines {
    composite_rgba: wgpu::RenderPipeline,
    composite_r8: wgpu::RenderPipeline,
    erase_rgba: wgpu::RenderPipeline,
    erase_r8: wgpu::RenderPipeline,
    clear_rgba: wgpu::RenderPipeline,
    clear_r8: wgpu::RenderPipeline,
    gradient_rgba: wgpu::RenderPipeline,
    gradient_r8: wgpu::RenderPipeline,
    mask_multiply_rgba: wgpu::RenderPipeline,
    mask_multiply_r8: wgpu::RenderPipeline,
    inverse_mask_multiply_rgba: wgpu::RenderPipeline,
    inverse_mask_multiply_r8: wgpu::RenderPipeline,
    alpha_mask_multiply_rgba: wgpu::RenderPipeline,
    alpha_inverse_mask_multiply_rgba: wgpu::RenderPipeline,
    /// Destructive mask bake: `dst.a *= mask` with the mask sampled in its own
    /// plane-anchored frame (footprint-aware reveal). RGBA-only; `apply_mask`
    /// guards on raster hosts, so R8 targets never take this path.
    alpha_mask_multiply_in_frame_rgba: wgpu::RenderPipeline,

    uniform_bgl: wgpu::BindGroupLayout,

    gradient_uniform_buf: wgpu::Buffer,
    gradient_uniform_bind_group: wgpu::BindGroup,

    /// 1×1 white selection texture, binds when no selection is active.
    pub(crate) default_selection_bind_group: wgpu::BindGroup,
    pub(crate) selection_bind_group_layout: wgpu::BindGroupLayout,
}

impl PaintPipelines {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        selection_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        // Shared with the brush pipeline + the cached selection bind group;
        // owned here (cheap Arc clone) so the rest of `new` reads as before.
        // See [`crate::gpu::selection::selection_mask_bgl`].
        let selection_bgl = selection_bgl.clone();
        let paint_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("paint-circle"),
            source: wgpu::ShaderSource::Wgsl(
                crate::gpu::canvas_lib::with_canvas_lib(include_str!(
                    "../../shaders/paint_circle.wgsl"
                ))
                .into(),
            ),
        });

        let gradient_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gradient"),
            source: wgpu::ShaderSource::Wgsl(
                crate::gpu::canvas_lib::with_canvas_lib(include_str!(
                    "../../shaders/gradient.wgsl"
                ))
                .into(),
            ),
        });

        let apply_mask_bake_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("apply-mask-bake"),
            source: wgpu::ShaderSource::Wgsl(
                crate::gpu::canvas_lib::with_canvas_lib(include_str!(
                    "../../shaders/apply_mask_bake.wgsl"
                ))
                .into(),
            ),
        });

        // --- Bind group layouts ---
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paint-uniform-bgl"),
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

        let paint_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("paint-pipeline-layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&selection_bgl)],
            immediate_size: 0,
        });

        // Gradient uses the same layout (uniform + selection) but a different uniform buffer.
        let gradient_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gradient-pipeline-layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&selection_bgl)],
            immediate_size: 0,
        });

        // --- Uniform buffers ---
        let gradient_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gradient-uniforms"),
            size: std::mem::size_of::<GradientUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let gradient_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gradient-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: gradient_uniform_buf.as_entire_binding(),
            }],
        });

        // --- Default selection texture (1×1 white = fully selected) ---
        let sel_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("default-selection"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
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
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let sel_view = sel_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("paint-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let default_selection_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint-default-selection-bg"),
            layout: &selection_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&sel_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        // --- Build pipeline variants ---
        let make_pipeline = |label: &str,
                             layout: &wgpu::PipelineLayout,
                             shader: &wgpu::ShaderModule,
                             format: wgpu::TextureFormat,
                             blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
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
                    module: shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        // Source-over compositing (straight alpha).
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

        // Erase on RGBA: reduce alpha only, keep RGB unchanged.
        let blend_erase_rgba = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        // Erase on R8: reduce the single channel toward 0.
        let blend_erase_r8 = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };

        // Clear: replace with source value (no blending).
        let blend_clear = wgpu::BlendState::REPLACE;

        // Gradient: composite blend (selection coverage modulates alpha).
        // For opaque gradient colors at coverage 1.0, this is equivalent to replace.
        let blend_gradient = blend_composite;

        // Mask multiply: dst.rgba *= fragment_alpha.
        // Fragment shader outputs (0,0,0, mask_sample), blend multiplies dst by it.
        // Used by apply_mask_destructive and selection masking of transform sources.
        let blend_mask_multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::SrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::SrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        // Inverse mask multiply: dst *= (1 - mask_sample). Same shader as
        // mask_multiply but with OneMinusSrcAlpha blend factor.
        // Used by transform source masking (premultiplied output for interpolation).
        let blend_inverse_mask_multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        // Alpha-only mask multiply: dst.a *= mask_sample, dst.rgb unchanged.
        // Correct for straight-alpha destinations where RGB represents the actual
        // color independent of opacity. Color uses dst_factor=One to preserve RGB.
        let blend_alpha_mask_multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::SrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        // Alpha-only inverse mask multiply: dst.a *= (1 - mask_sample), dst.rgb unchanged.
        let blend_alpha_inverse_mask_multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        PaintPipelines {
            composite_rgba: make_pipeline(
                "paint-composite-rgba",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::Rgba8Unorm,
                blend_composite,
            ),
            composite_r8: make_pipeline(
                "paint-composite-r8",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::R8Unorm,
                blend_composite,
            ),
            erase_rgba: make_pipeline(
                "paint-erase-rgba",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::Rgba8Unorm,
                blend_erase_rgba,
            ),
            erase_r8: make_pipeline(
                "paint-erase-r8",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::R8Unorm,
                blend_erase_r8,
            ),
            clear_rgba: make_pipeline(
                "paint-clear-rgba",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::Rgba8Unorm,
                blend_clear,
            ),
            clear_r8: make_pipeline(
                "paint-clear-r8",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::R8Unorm,
                blend_clear,
            ),
            gradient_rgba: make_pipeline(
                "gradient-rgba",
                &gradient_layout,
                &gradient_shader,
                wgpu::TextureFormat::Rgba8Unorm,
                blend_gradient,
            ),
            gradient_r8: make_pipeline(
                "gradient-r8",
                &gradient_layout,
                &gradient_shader,
                wgpu::TextureFormat::R8Unorm,
                blend_gradient,
            ),
            mask_multiply_rgba: make_pipeline(
                "mask-multiply-rgba",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::Rgba8Unorm,
                blend_mask_multiply,
            ),
            mask_multiply_r8: make_pipeline(
                "mask-multiply-r8",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::R8Unorm,
                blend_mask_multiply,
            ),
            inverse_mask_multiply_rgba: make_pipeline(
                "inv-mask-mul-rgba",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::Rgba8Unorm,
                blend_inverse_mask_multiply,
            ),
            inverse_mask_multiply_r8: make_pipeline(
                "inv-mask-mul-r8",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::R8Unorm,
                blend_inverse_mask_multiply,
            ),
            alpha_mask_multiply_rgba: make_pipeline(
                "alpha-mask-mul-rgba",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::Rgba8Unorm,
                blend_alpha_mask_multiply,
            ),
            alpha_inverse_mask_multiply_rgba: make_pipeline(
                "alpha-inv-mask-mul-rgba",
                &paint_layout,
                &paint_shader,
                wgpu::TextureFormat::Rgba8Unorm,
                blend_alpha_inverse_mask_multiply,
            ),
            alpha_mask_multiply_in_frame_rgba: make_pipeline(
                "alpha-mask-mul-in-frame-rgba",
                &paint_layout,
                &apply_mask_bake_shader,
                wgpu::TextureFormat::Rgba8Unorm,
                blend_alpha_mask_multiply,
            ),
            uniform_bgl,
            gradient_uniform_buf,
            gradient_uniform_bind_group,
            default_selection_bind_group,
            selection_bind_group_layout: selection_bgl,
        }
    }

    /// Upload flat R8 pixel data as a temporary GPU texture and return a
    /// selection-slot bind group for it.
    ///
    /// Used by flood fill (fill mask) and selection upload: both need to turn
    /// a `Vec<u8>` of R8 data into a bind group the paint shader can sample.
    pub fn upload_r8_bind_group(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        data: &[u8],
        label: &str,
    ) -> wgpu::BindGroup {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
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
                texture: &tex,
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
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("r8-mask-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        self.create_selection_bind_group(device, &view, &sampler)
    }

    /// Create a bind group for a custom selection mask texture.
    pub fn create_selection_bind_group(
        &self,
        device: &wgpu::Device,
        selection_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint-selection-bg"),
            layout: &self.selection_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(selection_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    fn composite_pipeline(&self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        match format {
            wgpu::TextureFormat::R8Unorm => &self.composite_r8,
            _ => &self.composite_rgba,
        }
    }

    fn erase_pipeline(&self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        match format {
            wgpu::TextureFormat::R8Unorm => &self.erase_r8,
            _ => &self.erase_rgba,
        }
    }

    fn clear_pipeline(&self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        match format {
            wgpu::TextureFormat::R8Unorm => &self.clear_r8,
            _ => &self.clear_rgba,
        }
    }

    fn gradient_pipeline(&self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        match format {
            wgpu::TextureFormat::R8Unorm => &self.gradient_r8,
            _ => &self.gradient_rgba,
        }
    }

    fn mask_multiply_pipeline(&self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        match format {
            wgpu::TextureFormat::R8Unorm => &self.mask_multiply_r8,
            _ => &self.mask_multiply_rgba,
        }
    }

    fn inverse_mask_multiply_pipeline(&self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        match format {
            wgpu::TextureFormat::R8Unorm => &self.inverse_mask_multiply_r8,
            _ => &self.inverse_mask_multiply_rgba,
        }
    }

    fn alpha_mask_multiply_pipeline(&self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        match format {
            // R8 has only one channel, so alpha-only and all-channel are equivalent.
            wgpu::TextureFormat::R8Unorm => &self.mask_multiply_r8,
            _ => &self.alpha_mask_multiply_rgba,
        }
    }

    fn alpha_inverse_mask_multiply_pipeline(
        &self,
        format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        match format {
            wgpu::TextureFormat::R8Unorm => &self.inverse_mask_multiply_r8,
            _ => &self.alpha_inverse_mask_multiply_rgba,
        }
    }

    fn alpha_mask_multiply_in_frame_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.alpha_mask_multiply_in_frame_rgba
    }
}

/// Uniform data sent to the paint_circle shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PaintUniforms {
    origin: [f32; 2],        // Quad origin in canvas pixels
    size: [f32; 2],          // Quad size in canvas pixels
    target_offset: [f32; 2], // Canvas-space offset of target's (0,0) pixel
    target_size: [f32; 2],   // Target texture pixel dimensions (vertex NDC)
    canvas_size: [f32; 2],   // Document canvas size (fragment selection UV)
    canvas_origin: [f32; 2], // Plane offset of the canvas window (selection UV)
    center: [f32; 2],        // Circle center in canvas pixels
    radius: f32,             // Circle radius (0 = solid fill)
    softness: f32,           // Soft edge width in pixels
    color: [f32; 4],         // RGBA paint color (straight alpha)
    mask_offset: [f32; 2],   // Mask texture plane-space offset (bake path only)
    mask_size: [f32; 2],     // Mask texture pixel size; 0 = no footprint
}

/// Uniform data sent to the gradient shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct GradientUniforms {
    origin: [f32; 2],        // Quad origin in canvas pixels
    size: [f32; 2],          // Quad size in canvas pixels
    target_offset: [f32; 2], // Canvas-space offset of target's (0,0) pixel
    target_size: [f32; 2],   // Target texture pixel dimensions (vertex NDC)
    canvas_size: [f32; 2],   // Document canvas size (fragment selection UV)
    canvas_origin: [f32; 2], // Plane offset of the canvas window (selection UV)
    start: [f32; 2],         // Gradient start point in canvas pixels
    end: [f32; 2],           // Gradient end point in canvas pixels
    color0: [f32; 4],        // Start color (RGBA, straight alpha)
    color1: [f32; 4],        // End color (RGBA, straight alpha)
}

/// Convert u8 RGBA color + opacity to f32 array for the shader.
fn color_to_float(color: [u8; 4], opacity: f32) -> [f32; 4] {
    [
        color[0] as f32 / 255.0,
        color[1] as f32 / 255.0,
        color[2] as f32 / 255.0,
        color[3] as f32 / 255.0 * opacity,
    ]
}
