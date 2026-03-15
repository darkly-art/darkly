//! GPU paint target: a texture you can paint on via GPU render passes.
//!
//! Works for both RGBA8 layer textures and R8 mask textures.
//! Each operation is a self-contained render pass — no persistent state between calls.

use crate::gpu::atlas::LayerTexture;

/// A GPU texture you can paint on. Lightweight handle — no owned GPU state.
pub struct GpuPaintTarget<'a> {
    pub texture: &'a wgpu::Texture,
    pub view: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
}

impl<'a> GpuPaintTarget<'a> {
    /// Wrap a layer texture as a paint target.
    pub fn from_layer(tex: &'a LayerTexture, canvas_width: u32, canvas_height: u32) -> Self {
        GpuPaintTarget {
            texture: &tex.texture,
            view: &tex.view,
            format: wgpu::TextureFormat::Rgba8Unorm,
            width: canvas_width,
            height: canvas_height,
        }
    }

    /// Wrap a mask texture as a paint target.
    pub fn from_mask(tex: &'a LayerTexture, canvas_width: u32, canvas_height: u32) -> Self {
        GpuPaintTarget {
            texture: &tex.texture,
            view: &tex.view,
            format: wgpu::TextureFormat::R8Unorm,
            width: canvas_width,
            height: canvas_height,
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
        self.draw_circle(encoder, pipeline, pipelines, queue, cx, cy, radius, color, opacity);
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
        self.draw_circle(encoder, pipeline, pipelines, queue, cx, cy, radius, [255, 255, 255, 255], 1.0);
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

        self.execute_pass(encoder, pipeline, pipelines, queue, &uniforms);
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

        self.execute_pass(encoder, pipeline, pipelines, queue, &uniforms);
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

        queue.write_buffer(&pipelines.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("paint-target-sel"),
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
        pass.set_bind_group(0, &pipelines.uniform_bind_group, &[]);
        pass.set_bind_group(1, selection_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    // --- Internal ---

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
    ) {
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

        self.execute_pass(encoder, pipeline, pipelines, queue, &uniforms);
    }

    fn execute_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        pipelines: &PaintPipelines,
        queue: &wgpu::Queue,
        uniforms: &PaintUniforms,
    ) {
        queue.write_buffer(&pipelines.uniform_buf, 0, bytemuck::bytes_of(uniforms));

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

        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &pipelines.uniform_bind_group, &[]);
        pass.set_bind_group(1, &pipelines.default_selection_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Pre-built render pipelines for paint operations.
///
/// Four pipeline variants: {composite, erase} × {RGBA8, R8}.
/// Plus a clear pipeline per format (replace blend).
pub struct PaintPipelines {
    composite_rgba: wgpu::RenderPipeline,
    composite_r8: wgpu::RenderPipeline,
    erase_rgba: wgpu::RenderPipeline,
    erase_r8: wgpu::RenderPipeline,
    clear_rgba: wgpu::RenderPipeline,
    clear_r8: wgpu::RenderPipeline,

    pub(crate) uniform_buf: wgpu::Buffer,
    pub(crate) uniform_bind_group: wgpu::BindGroup,

    /// 1×1 white selection texture — binds when no selection is active.
    pub(crate) default_selection_bind_group: wgpu::BindGroup,
    pub(crate) selection_bind_group_layout: wgpu::BindGroupLayout,
}

impl PaintPipelines {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("paint-circle"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/paint_circle.wgsl").into(),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("paint-pipeline-layout"),
            bind_group_layouts: &[&uniform_bgl, &selection_bgl],
            immediate_size: 0,
        });

        // --- Uniform buffer ---
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

        // --- Default selection texture (1×1 white = fully selected) ---
        let sel_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("default-selection"),
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
        let make_pipeline = |label: &str, format: wgpu::TextureFormat, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
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
                    module: &shader,
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

        PaintPipelines {
            composite_rgba: make_pipeline("paint-composite-rgba", wgpu::TextureFormat::Rgba8Unorm, blend_composite),
            composite_r8: make_pipeline("paint-composite-r8", wgpu::TextureFormat::R8Unorm, blend_composite),
            erase_rgba: make_pipeline("paint-erase-rgba", wgpu::TextureFormat::Rgba8Unorm, blend_erase_rgba),
            erase_r8: make_pipeline("paint-erase-r8", wgpu::TextureFormat::R8Unorm, blend_erase_r8),
            clear_rgba: make_pipeline("paint-clear-rgba", wgpu::TextureFormat::Rgba8Unorm, blend_clear),
            clear_r8: make_pipeline("paint-clear-r8", wgpu::TextureFormat::R8Unorm, blend_clear),
            uniform_buf,
            uniform_bind_group,
            default_selection_bind_group,
            selection_bind_group_layout: selection_bgl,
        }
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
}

/// Uniform data sent to the paint_circle shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PaintUniforms {
    origin: [f32; 2],      // Quad origin in canvas pixels
    size: [f32; 2],        // Quad size in canvas pixels
    canvas_size: [f32; 2], // Padded canvas dimensions
    center: [f32; 2],      // Circle center in canvas pixels
    radius: f32,           // Circle radius (0 = solid fill)
    softness: f32,         // Soft edge width in pixels
    _pad: [f32; 2],        // Align color to 16 bytes
    color: [f32; 4],       // RGBA paint color (straight alpha)
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
