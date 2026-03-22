//! GPU-authoritative selection mask — owns the R8 texture and bind groups.
//! No persistent CPU cache. Contour extraction runs on async readback data.

use crate::document::SelectionMode;
use crate::mask::RasterizedMask;

/// Reusable pipelines for selection boolean operations.
/// Created once in `DarklyEngine::new()`.
pub(crate) struct SelectionPipelines {
    combine_pipeline: wgpu::RenderPipeline,
    combine_bgl: wgpu::BindGroupLayout,
    mode_buf: wgpu::Buffer,
    sampler: wgpu::Sampler,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CombineParams {
    mode: u32,
    _pad: [u32; 3],
}

impl SelectionPipelines {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("selection-combine"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/selection_combine.wgsl").into(),
            ),
        });

        let combine_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sel-combine-bgl"),
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
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
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
            label: Some("sel-combine-layout"),
            bind_group_layouts: &[&combine_bgl],
            immediate_size: 0,
        });

        let combine_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sel-combine-pipeline"),
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
                    format: wgpu::TextureFormat::R8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::RED,
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

        let mode_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sel-combine-mode"),
            size: std::mem::size_of::<CombineParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sel-combine-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        SelectionPipelines { combine_pipeline, combine_bgl, mode_buf, sampler }
    }

    /// Run the combine shader: reads `selection.textures[current]` + shape → writes
    /// to `selection.textures[1 - current]`, then swaps and rebuilds bind groups.
    pub fn combine(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        selection: &mut GpuSelection,
        shape_data: &[u8],
        mode: CombineMode,
        brush_bgl: &wgpu::BindGroupLayout,
        paint_bgl: &wgpu::BindGroupLayout,
    ) {
        let w = selection.width;
        let h = selection.height;

        // Upload shape to a temp texture.
        let shape_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sel-shape-temp"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &shape_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            shape_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w),
                rows_per_image: None,
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let shape_view = shape_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // Set mode uniform.
        queue.write_buffer(&self.mode_buf, 0, bytemuck::bytes_of(&CombineParams {
            mode: mode as u32,
            _pad: [0; 3],
        }));

        // Create bind group: existing + shape + sampler + mode.
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sel-combine-bg"),
            layout: &self.combine_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&selection.views[selection.current]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&shape_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.mode_buf.as_entire_binding(),
                },
            ],
        });

        // Render to the other ping-pong texture.
        let dst = 1 - selection.current;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sel-combine-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &selection.views[dst],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&self.combine_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // Swap ping-pong and rebuild bind groups.
        selection.current = dst;
        selection.rebuild_bind_groups(device, brush_bgl, paint_bgl, &self.sampler);
    }

    /// Run the combine shader in "invert" mode.
    pub fn invert(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        selection: &mut GpuSelection,
        brush_bgl: &wgpu::BindGroupLayout,
        paint_bgl: &wgpu::BindGroupLayout,
    ) {
        queue.write_buffer(&self.mode_buf, 0, bytemuck::bytes_of(&CombineParams {
            mode: CombineMode::Invert as u32,
            _pad: [0; 3],
        }));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sel-invert-bg"),
            layout: &self.combine_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&selection.views[selection.current]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&selection.views[selection.current]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.mode_buf.as_entire_binding(),
                },
            ],
        });

        let dst = 1 - selection.current;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sel-invert-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &selection.views[dst],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&self.combine_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        selection.current = dst;
        selection.rebuild_bind_groups(device, brush_bgl, paint_bgl, &self.sampler);
    }
}

#[repr(u32)]
pub(crate) enum CombineMode {
    Add = 0,
    Subtract = 1,
    Intersect = 2,
    Invert = 3,
}

impl CombineMode {
    pub fn from_selection_mode(mode: &SelectionMode) -> Self {
        match mode {
            SelectionMode::Add => CombineMode::Add,
            SelectionMode::Subtract => CombineMode::Subtract,
            SelectionMode::Intersect => CombineMode::Intersect,
            SelectionMode::Replace => unreachable!("Replace mode uses direct upload"),
        }
    }
}

// ---------------------------------------------------------------------------
// GpuSelection — persistent selection state (always allocated)
// ---------------------------------------------------------------------------

/// GPU-authoritative selection mask.
///
/// No persistent CPU cache. The GPU texture is the single source of truth.
/// Pixel bounds are tracked from rasterization parameters. CPU reads happen
/// via on-demand blocking readback when needed (copy, transform).
pub(crate) struct GpuSelection {
    /// Ping-pong pair of R8Unorm textures (canvas-sized).
    pub textures: [wgpu::Texture; 2],
    pub views: [wgpu::TextureView; 2],
    /// Index into `textures` for the current selection data.
    pub current: usize,

    /// Bind group for `BrushPipelines::selection_bgl`.
    brush_bind_group: wgpu::BindGroup,
    /// Bind group for `PaintPipelines::selection_bind_group_layout`.
    paint_bind_group: wgpu::BindGroup,

    /// Cached tight pixel bounds. Set from rasterization params on Replace,
    /// cleared on boolean ops / invert (recomputed from readback when needed).
    pub pixel_bounds: Option<[u32; 4]>,
    /// True when a selection is logically active.
    pub active: bool,

    pub width: u32,
    pub height: u32,
}

impl GpuSelection {
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        brush_bgl: &wgpu::BindGroupLayout,
        paint_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let textures = std::array::from_fn(|i| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(if i == 0 { "sel-tex-0" } else { "sel-tex-1" }),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let views = [
            textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sel-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let brush_bind_group = Self::create_brush_bind_group(device, &views[0], &sampler, brush_bgl);
        let paint_bind_group = Self::create_paint_bind_group(device, &views[0], &sampler, paint_bgl);

        GpuSelection {
            textures,
            views,
            current: 0,
            brush_bind_group,
            paint_bind_group,
            pixel_bounds: None,
            active: false,
            width,
            height,
        }
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.textures[self.current]
    }

    pub fn brush_bind_group(&self) -> &wgpu::BindGroup {
        &self.brush_bind_group
    }

    pub fn paint_bind_group(&self) -> &wgpu::BindGroup {
        &self.paint_bind_group
    }

    /// Upload a tight-bounds R8 region (Replace mode).
    /// Clears the previous selection region, writes the new subregion.
    pub fn upload_replace(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mask: &RasterizedMask,
        brush_bgl: &wgpu::BindGroupLayout,
        paint_bgl: &wgpu::BindGroupLayout,
    ) {
        // Clear the old selection region on GPU (if any).
        if let Some([ox, oy, ow, oh]) = self.pixel_bounds {
            let zeros = vec![0u8; (ow * oh) as usize];
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures[self.current],
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: ox, y: oy, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &zeros,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ow),
                    rows_per_image: None,
                },
                wgpu::Extent3d { width: ow, height: oh, depth_or_array_layers: 1 },
            );
        }

        // Write the new shape subregion.
        if mask.width > 0 && mask.height > 0 {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures[self.current],
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: mask.x, y: mask.y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &mask.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(mask.width),
                    rows_per_image: None,
                },
                wgpu::Extent3d { width: mask.width, height: mask.height, depth_or_array_layers: 1 },
            );
        }

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sel-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        self.rebuild_bind_groups(device, brush_bgl, paint_bgl, &sampler);

        self.pixel_bounds = Some([mask.x, mask.y, mask.width, mask.height]);
        self.active = true;
    }

    /// Upload full-canvas R8 data (used by magic wand, mask-to-selection).
    pub fn upload_replace_full(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        brush_bgl: &wgpu::BindGroupLayout,
        paint_bgl: &wgpu::BindGroupLayout,
    ) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.textures[self.current],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width),
                rows_per_image: None,
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sel-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        self.rebuild_bind_groups(device, brush_bgl, paint_bgl, &sampler);

        self.pixel_bounds = crate::mask::pixel_bounds_r8(data, self.width, self.height);
        self.active = true;
    }

    /// Clear the selection: zero the active region, mark inactive.
    pub fn clear(&mut self, queue: &wgpu::Queue) {
        if let Some([ox, oy, ow, oh]) = self.pixel_bounds {
            let zeros = vec![0u8; (ow * oh) as usize];
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures[self.current],
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: ox, y: oy, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &zeros,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ow),
                    rows_per_image: None,
                },
                wgpu::Extent3d { width: ow, height: oh, depth_or_array_layers: 1 },
            );
        }
        self.pixel_bounds = None;
        self.active = false;
    }

    /// Blocking readback of the full selection texture. Used by operations
    /// that need CPU access (copy, transform bounds).
    pub fn blocking_readback(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
        crate::gpu::test_utils::readback_texture(
            device, queue, self.texture(),
            wgpu::TextureFormat::R8Unorm, self.width, self.height,
        )
    }

    /// Rebuild bind groups after a ping-pong swap.
    fn rebuild_bind_groups(
        &mut self,
        device: &wgpu::Device,
        brush_bgl: &wgpu::BindGroupLayout,
        paint_bgl: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
    ) {
        self.brush_bind_group = Self::create_brush_bind_group(
            device, &self.views[self.current], sampler, brush_bgl,
        );
        self.paint_bind_group = Self::create_paint_bind_group(
            device, &self.views[self.current], sampler, paint_bgl,
        );
    }

    fn create_brush_bind_group(
        device: &wgpu::Device, view: &wgpu::TextureView,
        sampler: &wgpu::Sampler, layout: &wgpu::BindGroupLayout,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sel-brush-bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        })
    }

    fn create_paint_bind_group(
        device: &wgpu::Device, view: &wgpu::TextureView,
        sampler: &wgpu::Sampler, layout: &wgpu::BindGroupLayout,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sel-paint-bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        })
    }
}
