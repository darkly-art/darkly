//! GPU paint target: a texture you can paint on via GPU render passes.
//!
//! Works for both RGBA8 layer textures and R8 mask textures.
//! Each operation is a self-contained render pass — no persistent state between calls.

use crate::gpu::atlas::LayerTexture;

/// A GPU texture you can paint on. Lightweight handle — no owned GPU state.
///
/// Brush coordinates are passed in **canvas space**. The target's `offset_x`/
/// `offset_y` describe where its (0, 0) pixel sits in canvas coordinates;
/// each public paint method subtracts that offset internally before issuing
/// GPU commands. This mirrors Krita's `KisPaintDevice`, where image-space
/// coordinates flow into the device and iterators handle the translation
/// via `KisPaintDevice::x()`/`y()`. Callers don't need to know whether a
/// layer is canvas-aligned or off-canvas.
pub struct GpuPaintTarget<'a> {
    pub texture: &'a wgpu::Texture,
    pub view: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    /// Texture pixel dimensions.
    pub width: u32,
    pub height: u32,
    /// Canvas-space offset of pixel (0, 0). Subtracted from canvas coords
    /// before each paint operation.
    pub offset_x: i32,
    pub offset_y: i32,
}

impl<'a> GpuPaintTarget<'a> {
    /// Wrap a layer texture as a paint target. The dimensions and offset
    /// come from the texture itself; the canvas args are kept only for
    /// callers that haven't been migrated yet (they're ignored).
    pub fn from_layer(tex: &'a LayerTexture, _canvas_width: u32, _canvas_height: u32) -> Self {
        GpuPaintTarget {
            texture: &tex.texture,
            view: &tex.view,
            format: wgpu::TextureFormat::Rgba8Unorm,
            width: tex.width,
            height: tex.height,
            offset_x: tex.offset_x,
            offset_y: tex.offset_y,
        }
    }

    /// Wrap a mask texture as a paint target. See `from_layer`.
    pub fn from_mask(tex: &'a LayerTexture, _canvas_width: u32, _canvas_height: u32) -> Self {
        GpuPaintTarget {
            texture: &tex.texture,
            view: &tex.view,
            format: wgpu::TextureFormat::R8Unorm,
            width: tex.width,
            height: tex.height,
            offset_x: tex.offset_x,
            offset_y: tex.offset_y,
        }
    }

    /// Paint a soft circle onto the target via alpha-over blending.
    pub fn composite_circle(
        &self,
        encoder: &mut wgpu::CommandEncoder,
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

    /// Erase a soft circle from the target.
    pub fn erase_circle(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        cx: f32,
        cy: f32,
        radius: f32,
    ) {
        let pipeline = pipelines.erase_pipeline(self.format);
        // Erase uses white color at full alpha — the blend state does the subtracting.
        // For R8 targets, luminance(1,1,1) = 1.0 which reduces toward 0.
        self.draw_circle(
            encoder,
            pipeline,
            pipelines,
            queue,
            cx,
            cy,
            radius,
            [255, 255, 255, 255],
            1.0,
            None,
        );
    }

    /// Paint a soft circle with a custom selection mask bind group.
    pub fn composite_circle_with_selection(
        &self,
        encoder: &mut wgpu::CommandEncoder,
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

    /// Fill a rect with a solid color via alpha-over blending.
    pub fn fill_rect(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        rect: [u32; 4],
        color: [u8; 4],
    ) {
        self.fill_rect_inner(encoder, pipelines, queue, rect, color, None);
    }

    /// Fill a rect with a solid color, masked by a selection bind group.
    /// Used by flood fill: the fill mask texture is bound as the "selection".
    pub fn fill_rect_with_selection(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        rect: [u32; 4],
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

    /// Erase pixels within a selection mask. Full-canvas erase modulated by the
    /// selection texture — used for clear_selection_contents.
    pub fn erase_with_selection(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        selection_bind_group: &wgpu::BindGroup,
    ) {
        let pipeline = pipelines.erase_pipeline(self.format);

        let uniforms = PaintUniforms {
            origin: [0.0, 0.0],
            size: [self.width as f32, self.height as f32],
            canvas_size: [self.width as f32, self.height as f32],
            center: [0.0, 0.0],
            radius: 0.0, // solid fill — coverage from selection only
            softness: 0.0,
            _pad: [0.0; 2],
            color: [1.0, 1.0, 1.0, 1.0], // full erase strength
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
    /// `dst.rgba *= mask_sample` — produces premultiplied output. Use this when
    /// the result will be sampled with bilinear filtering (e.g. transform sources),
    /// where premultiplied data is required for correct interpolation at alpha
    /// edges (see compositing-lessons-learned.md §2).
    ///
    /// **Do not use for straight-alpha destinations** (layer textures, clipboard
    /// staging). Use `multiply_alpha_by_mask` instead — it preserves RGB and only
    /// scales the alpha channel, which is correct for straight-alpha storage.
    pub fn multiply_by_mask(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        mask_bind_group: &wgpu::BindGroup,
    ) {
        let pipeline = pipelines.mask_multiply_pipeline(self.format);

        // Full-target rect, color = black with full alpha.
        // The shader outputs (0, 0, 0, mask_sample) and the blend state
        // computes dst * SrcAlpha = dst * mask_sample.
        let uniforms = PaintUniforms {
            origin: [0.0, 0.0],
            size: [self.width as f32, self.height as f32],
            canvas_size: [self.width as f32, self.height as f32],
            center: [0.0, 0.0],
            radius: 0.0,
            softness: 0.0,
            _pad: [0.0; 2],
            color: [0.0, 0.0, 0.0, 1.0],
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
    /// `dst.rgba *= (1 - mask_sample)` — produces premultiplied output.
    /// Same caveat as `multiply_by_mask`: do not use for straight-alpha
    /// destinations. Use `multiply_alpha_by_inverse_mask` instead.
    pub fn multiply_by_inverse_mask(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        mask_bind_group: &wgpu::BindGroup,
    ) {
        let pipeline = pipelines.inverse_mask_multiply_pipeline(self.format);

        let uniforms = PaintUniforms {
            origin: [0.0, 0.0],
            size: [self.width as f32, self.height as f32],
            canvas_size: [self.width as f32, self.height as f32],
            center: [0.0, 0.0],
            radius: 0.0,
            softness: 0.0,
            _pad: [0.0; 2],
            color: [0.0, 0.0, 0.0, 1.0],
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
    /// compositing-lessons-learned.md §1: in straight alpha, coverage scaling
    /// only affects the alpha channel.
    pub fn multiply_alpha_by_mask(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        mask_bind_group: &wgpu::BindGroup,
    ) {
        let pipeline = pipelines.alpha_mask_multiply_pipeline(self.format);

        let uniforms = PaintUniforms {
            origin: [0.0, 0.0],
            size: [self.width as f32, self.height as f32],
            canvas_size: [self.width as f32, self.height as f32],
            center: [0.0, 0.0],
            radius: 0.0,
            softness: 0.0,
            _pad: [0.0; 2],
            color: [0.0, 0.0, 0.0, 1.0],
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
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        mask_bind_group: &wgpu::BindGroup,
    ) {
        let pipeline = pipelines.alpha_inverse_mask_multiply_pipeline(self.format);

        let uniforms = PaintUniforms {
            origin: [0.0, 0.0],
            size: [self.width as f32, self.height as f32],
            canvas_size: [self.width as f32, self.height as f32],
            center: [0.0, 0.0],
            radius: 0.0,
            softness: 0.0,
            _pad: [0.0; 2],
            color: [0.0, 0.0, 0.0, 1.0],
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

    /// Clear a rect to transparent (RGBA) or full reveal (R8).
    pub fn clear_rect(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        rect: [u32; 4],
    ) {
        let [x, y, w, h] = rect;
        let pipeline = pipelines.clear_pipeline(self.format);

        let color = match self.format {
            wgpu::TextureFormat::R8Unorm => [1.0, 1.0, 1.0, 1.0], // 255 = reveal all
            _ => [0.0, 0.0, 0.0, 0.0],                            // transparent
        };

        let uniforms = PaintUniforms {
            origin: [x as f32, y as f32],
            size: [w as f32, h as f32],
            canvas_size: [self.width as f32, self.height as f32],
            center: [0.0, 0.0],
            radius: 0.0,
            softness: 0.0,
            _pad: [0.0; 2],
            color,
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
            origin: [0.0, 0.0],
            size: [self.width as f32, self.height as f32],
            canvas_size: [self.width as f32, self.height as f32],
            start: [x0, y0],
            end: [x1, y1],
            _pad: [0.0; 2],
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
        pass.set_bind_group(0, &pipelines.gradient_uniform_bind_group, &[]);
        pass.set_bind_group(1, sel, &[]);
        pass.draw(0..3, 0..1);
    }

    // --- Internal ---

    fn fill_rect_inner(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        rect: [u32; 4],
        color: [u8; 4],
        selection: Option<&wgpu::BindGroup>,
    ) {
        let [x, y, w, h] = rect;
        let pipeline = pipelines.composite_pipeline(self.format);

        let uniforms = PaintUniforms {
            origin: [x as f32, y as f32],
            size: [w as f32, h as f32],
            canvas_size: [self.width as f32, self.height as f32],
            center: [0.0, 0.0],
            radius: 0.0, // solid fill — no SDF
            softness: 0.0,
            _pad: [0.0; 2],
            color: color_to_float(color, 1.0),
        };

        self.execute_pass(encoder, pipeline, pipelines, queue, &uniforms, selection);
    }

    fn draw_circle(
        &self,
        encoder: &mut wgpu::CommandEncoder,
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
        // Translate canvas-space input to target-local pixel coords. For
        // canvas-aligned layers this is a no-op.
        let cx = cx - self.offset_x as f32;
        let cy = cy - self.offset_y as f32;

        // Pad the quad by softness + 1 pixel so the SDF falloff isn't clipped.
        let softness = 1.0_f32;
        let pad = softness + 1.0;
        let x0 = (cx - radius - pad).max(0.0);
        let y0 = (cy - radius - pad).max(0.0);
        let x1 = (cx + radius + pad).min(self.width as f32);
        let y1 = (cy + radius + pad).min(self.height as f32);

        let uniforms = PaintUniforms {
            origin: [x0, y0],
            size: [x1 - x0, y1 - y0],
            canvas_size: [self.width as f32, self.height as f32],
            center: [cx, cy],
            radius,
            softness,
            _pad: [0.0; 2],
            color: color_to_float(color, opacity),
        };

        self.execute_pass(encoder, pipeline, pipelines, queue, &uniforms, selection);
    }

    fn execute_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        uniforms: &PaintUniforms,
        selection: Option<&wgpu::BindGroup>,
    ) {
        queue.write_buffer(&pipelines.uniform_buf, 0, bytemuck::bytes_of(uniforms));

        let sel = selection.unwrap_or(&pipelines.default_selection_bind_group);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
        pass.set_bind_group(0, &pipelines.uniform_bind_group, &[]);
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

    pub(crate) uniform_buf: wgpu::Buffer,
    pub(crate) uniform_bind_group: wgpu::BindGroup,

    gradient_uniform_buf: wgpu::Buffer,
    gradient_uniform_bind_group: wgpu::BindGroup,

    /// 1×1 white selection texture — binds when no selection is active.
    pub(crate) default_selection_bind_group: wgpu::BindGroup,
    pub(crate) selection_bind_group_layout: wgpu::BindGroupLayout,
}

impl PaintPipelines {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let paint_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("paint-circle"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/paint_circle.wgsl").into(),
            ),
        });

        let gradient_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gradient"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/gradient.wgsl").into(),
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
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let selection_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paint-selection-bgl"),
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

        let paint_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("paint-pipeline-layout"),
            bind_group_layouts: &[&uniform_bgl, &selection_bgl],
            immediate_size: 0,
        });

        // Gradient uses the same layout (uniform + selection) but a different uniform buffer.
        let gradient_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gradient-pipeline-layout"),
            bind_group_layouts: &[&uniform_bgl, &selection_bgl],
            immediate_size: 0,
        });

        // --- Uniform buffers ---
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("paint-uniforms"),
            size: std::mem::size_of::<PaintUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

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
            uniform_buf,
            uniform_bind_group,
            gradient_uniform_buf,
            gradient_uniform_bind_group,
            default_selection_bind_group,
            selection_bind_group_layout: selection_bgl,
        }
    }

    /// Upload flat R8 pixel data as a temporary GPU texture and return a
    /// selection-slot bind group for it.
    ///
    /// Used by flood fill (fill mask) and selection upload — both need to turn
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
            // R8 has only one channel — alpha-only and all-channel are equivalent.
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
}

/// Uniform data sent to the paint_circle shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PaintUniforms {
    origin: [f32; 2],      // Quad origin in canvas pixels
    size: [f32; 2],        // Quad size in canvas pixels
    canvas_size: [f32; 2], // Unpadded canvas dimensions (viewport is set to match)
    center: [f32; 2],      // Circle center in canvas pixels
    radius: f32,           // Circle radius (0 = solid fill)
    softness: f32,         // Soft edge width in pixels
    _pad: [f32; 2],        // Align color to 16 bytes
    color: [f32; 4],       // RGBA paint color (straight alpha)
}

/// Uniform data sent to the gradient shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct GradientUniforms {
    origin: [f32; 2],      // Quad origin in canvas pixels
    size: [f32; 2],        // Quad size in canvas pixels
    canvas_size: [f32; 2], // Unpadded canvas dimensions (viewport is set to match)
    start: [f32; 2],       // Gradient start point in canvas pixels
    end: [f32; 2],         // Gradient end point in canvas pixels
    _pad: [f32; 2],        // Align colors to 16 bytes
    color0: [f32; 4],      // Start color (RGBA, straight alpha)
    color1: [f32; 4],      // End color (RGBA, straight alpha)
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
