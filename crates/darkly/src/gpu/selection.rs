//! GPU-side state for the document's global selection: ping-pong R8 textures,
//! the shared selection-mask bind group, and the boolean-op render pipelines.
//!
//! The selection itself is a typed [`crate::document::Modifier`] attached at
//! the document root, with its pixel-level metadata (`active`, `pixel_bounds`,
//! `cpu_cache`) on [`SelectionModifier`]. What lives here is purely the GPU
//! realisation: textures, bind group, and the shaders that mutate them.
//!
//! Both the brush and paint pipelines sample the mask through a single shared
//! [`selection_mask_bgl`], so one bind group serves every consumer.
//!
//! Ping-pong: combine/invert ops can't read+write the same texture in a single
//! render pass, so we keep two R8 textures and swap which is "current". The
//! bind group always references the current one and is rebuilt after a swap.

use crate::document::SelectionMode;
use crate::layer::LayerId;

/// Reusable GPU pipelines for selection boolean operations.
/// Created once in `DarklyEngine::new()`.
pub struct SelectionPipelines {
    combine_pipeline: wgpu::RenderPipeline,
    combine_bgl: wgpu::BindGroupLayout,
    mode_buf: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// Recyclable R8 texture used as `combine`'s `shape_tex` binding.
    /// Sized to exactly the selection's `(width, height)` because the
    /// combine shader samples with UV in `[0, 1]` over the full texture
    /// extent — a larger scratch would shift the UV mapping and miss the
    /// uploaded shape data. Re-allocated only when the selection's
    /// dimensions change (canvas resize, document load).
    shape_scratch: Option<ShapeScratch>,
}

struct ShapeScratch {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
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
            bind_group_layouts: &[Some(&combine_bgl)],
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

        SelectionPipelines {
            combine_pipeline,
            combine_bgl,
            mode_buf,
            sampler,
            shape_scratch: None,
        }
    }

    /// Ensure `shape_scratch` is exactly `(w, h)` and return its view. The
    /// combine shader's UV must map `(0, 1)` to the uploaded data, so the
    /// scratch dimensions cannot be larger than the selection — when the
    /// selection's dims change we reallocate.
    fn ensure_shape_scratch(&mut self, device: &wgpu::Device, w: u32, h: u32) -> &ShapeScratch {
        let needs_alloc = match &self.shape_scratch {
            Some(s) => s.width != w || s.height != h,
            None => true,
        };
        if needs_alloc {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sel-shape-scratch"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.shape_scratch = Some(ShapeScratch {
                texture,
                view,
                width: w,
                height: h,
            });
        }
        self.shape_scratch.as_ref().unwrap()
    }

    /// Run the combine shader: reads `state.textures[current]` + shape → writes
    /// to `state.textures[1 - current]`, then swaps and rebuilds bind groups.
    pub fn combine(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        state: &mut SelectionState,
        shape_data: &[u8],
        mode: CombineMode,
    ) {
        let w = state.width;
        let h = state.height;

        self.ensure_shape_scratch(device, w, h);
        let scratch = self.shape_scratch.as_ref().expect("just ensured");
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &scratch.texture,
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
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let shape_view = &scratch.view;

        queue.write_buffer(
            &self.mode_buf,
            0,
            bytemuck::bytes_of(&CombineParams {
                mode: mode as u32,
                _pad: [0; 3],
            }),
        );

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sel-combine-bg"),
            layout: &self.combine_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&state.views[state.current]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(shape_view),
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

        let dst = 1 - state.current;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sel-combine-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &state.views[dst],
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

        state.current = dst;
        state.rebuild_bind_group(device);
    }

    /// Run the combine shader in "invert" mode.
    pub fn invert(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        state: &mut SelectionState,
    ) {
        queue.write_buffer(
            &self.mode_buf,
            0,
            bytemuck::bytes_of(&CombineParams {
                mode: CombineMode::Invert as u32,
                _pad: [0; 3],
            }),
        );

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sel-invert-bg"),
            layout: &self.combine_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&state.views[state.current]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&state.views[state.current]),
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

        let dst = 1 - state.current;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sel-invert-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &state.views[dst],
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

        state.current = dst;
        state.rebuild_bind_group(device);
    }
}

#[repr(u32)]
pub enum CombineMode {
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
// SelectionState — GPU resources for the global selection (compositor-owned)
// ---------------------------------------------------------------------------

/// The single bind group layout for sampling the selection mask, shared by
/// every consumer (brush + paint pipelines, and the cached [`SelectionState`]
/// bind group). One layout means one bind group: a `wgpu::BindGroup` is tied to
/// the exact layout it was built against, so without sharing each pipeline would
/// need its own (structurally identical) copy.
///
/// Visibility is `FRAGMENT | COMPUTE` — the union of where the mask is sampled:
/// paint reads it in a fragment shader, brush nodes also read it from compute
/// shaders. A render pipeline may legally use a layout whose visibility is a
/// superset of the stages it actually uses, so this single layout serves both.
pub fn selection_mask_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("selection-mask-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

/// Ping-pong R8 textures + the selection mask bind group for the document's
/// global selection. Allocated by the compositor when the selection modifier is
/// first needed; lives until the document is dropped.
pub struct SelectionState {
    pub textures: [wgpu::Texture; 2],
    pub views: [wgpu::TextureView; 2],
    /// Index into `textures` for the current (read) selection data.
    pub current: usize,
    /// The selection mask is consumed by both the brush and paint pipelines.
    /// Both share a single [`selection_mask_bgl`] layout, so one bind group
    /// serves every consumer — see that function's note on visibility.
    bind_group: wgpu::BindGroup,
    /// Cloned at construction (cheap, Arc-backed) so reallocating ops can
    /// rebuild `bind_group` against the new texture view without the caller
    /// threading the layout through.
    bgl: wgpu::BindGroupLayout,
    /// Constant across the state's lifetime; built once instead of per-rebuild.
    sampler: wgpu::Sampler,
    /// Modifier id this state is paired with (for region-store and undo
    /// keying — the document's selection modifier id).
    pub modifier_id: LayerId,
    pub width: u32,
    pub height: u32,
}

impl SelectionState {
    pub fn new(
        device: &wgpu::Device,
        modifier_id: LayerId,
        width: u32,
        height: u32,
        bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let textures = std::array::from_fn(|i| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(if i == 0 { "sel-tex-0" } else { "sel-tex-1" }),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
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

        let bind_group = Self::make_bg(device, &views[0], &sampler, bgl);

        SelectionState {
            textures,
            views,
            current: 0,
            bind_group,
            bgl: bgl.clone(),
            sampler,
            modifier_id,
            width,
            height,
        }
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.textures[self.current]
    }

    /// The selection mask bind group, usable by both the brush and paint
    /// pipelines (they share [`selection_mask_bgl`]).
    pub fn selection_bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Borrow the current selection texture as a `CanvasFrame`. The selection
    /// texture is window-sized; its `canvas_extent` is window-local `(0, 0,
    /// w, h)` (the plane anchoring is realized by [`Self::resize`], not by a
    /// non-zero extent origin — see CLAUDE.md selection notes).
    pub fn canvas_frame(&self) -> crate::gpu::atlas::CanvasFrame<'_> {
        crate::gpu::atlas::CanvasFrame {
            texture: self.texture(),
            canvas_extent: crate::coord::CanvasRect::from_xywh(0, 0, self.width, self.height),
        }
    }

    /// Re-realize the window-sized selection mask for a moved/resized canvas
    /// window (crop / canvas resize).
    ///
    /// The mask is a window-sized R8 texture whose pixel `(0, 0)` represents
    /// the plane position `old_origin`. When the window moves to `new_rect`,
    /// allocate fresh ping-pong textures at the new dimensions, clear them to
    /// "unselected", and copy the **plane-overlap** of the old and new windows
    /// — preserving which *plane* pixels stay selected. Selection outside the
    /// new window is clipped away. Same overlap-anchored copy as
    /// [`Compositor::resize_node_texture`], generalized to a window that may
    /// move in any direction (so the overlap is clipped, not assumed to grow).
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        old_origin: crate::coord::CanvasPoint,
        new_rect: crate::coord::CanvasRect,
    ) {
        use crate::coord::CanvasRect;
        let new_w = new_rect.width;
        let new_h = new_rect.height;

        let new_textures: [wgpu::Texture; 2] = std::array::from_fn(|i| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(if i == 0 { "sel-tex-0" } else { "sel-tex-1" }),
                size: wgpu::Extent3d {
                    width: new_w,
                    height: new_h,
                    depth_or_array_layers: 1,
                },
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
        let new_views = [
            new_textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            new_textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sel-resize"),
        });
        // Clear the new "current" texture (index 0) to unselected (0). The
        // other ping-pong side is overwritten by the next combine pass.
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sel-resize-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &new_views[0],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }

        let old_rect = CanvasRect::new(old_origin, self.width, self.height);
        if let Some(ov) = old_rect.intersect(new_rect) {
            let src_x = (ov.origin.x - old_origin.x) as u32;
            let src_y = (ov.origin.y - old_origin.y) as u32;
            let dst_x = (ov.origin.x - new_rect.origin.x) as u32;
            let dst_y = (ov.origin.y - new_rect.origin.y) as u32;
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures[self.current],
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: src_x,
                        y: src_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &new_textures[0],
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: dst_x,
                        y: dst_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: ov.width,
                    height: ov.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        queue.submit([encoder.finish()]);

        self.textures = new_textures;
        self.views = new_views;
        self.current = 0;
        self.width = new_w;
        self.height = new_h;

        self.rebuild_bind_group(device);
    }

    /// Replace the selection with a tight-bounds rasterized R8 region. Clears
    /// the previous active region first, then writes the new one.
    pub fn upload_replace(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        old_bounds: Option<crate::coord::WindowRect>,
        mask: &crate::mask::RasterizedMask,
    ) {
        if let Some(bounds) = old_bounds {
            let ow = bounds.width;
            let oh = bounds.height;
            let zeros = vec![0u8; (ow * oh) as usize];
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures[self.current],
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: bounds.x0() as u32,
                        y: bounds.y0() as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &zeros,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ow),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: ow,
                    height: oh,
                    depth_or_array_layers: 1,
                },
            );
        }

        if mask.width > 0 && mask.height > 0 {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures[self.current],
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: mask.x,
                        y: mask.y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &mask.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(mask.width),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: mask.width,
                    height: mask.height,
                    depth_or_array_layers: 1,
                },
            );
        }

        self.rebuild_bind_group(device);
    }

    /// Replace the selection with a full-canvas R8 buffer (magic wand, mask-
    /// to-selection).
    pub fn upload_replace_full(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
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
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );

        self.rebuild_bind_group(device);
    }

    /// Zero out the previously-active region (clear). `bounds` are window-local
    /// (the selection texture is window-sized; see `crate::coord`).
    pub fn clear_region(&mut self, queue: &wgpu::Queue, bounds: Option<crate::coord::WindowRect>) {
        if let Some(bounds) = bounds {
            let ow = bounds.width;
            let oh = bounds.height;
            let zeros = vec![0u8; (ow * oh) as usize];
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures[self.current],
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: bounds.x0() as u32,
                        y: bounds.y0() as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &zeros,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ow),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: ow,
                    height: oh,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Rebuild the bind group after a ping-pong swap or texture reallocation,
    /// re-pointing it at the now-current view. Uses the layout + sampler the
    /// state owns, so callers never thread them through.
    fn rebuild_bind_group(&mut self, device: &wgpu::Device) {
        self.bind_group =
            Self::make_bg(device, &self.views[self.current], &self.sampler, &self.bgl);
    }

    fn make_bg(
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        layout: &wgpu::BindGroupLayout,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sel-bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }
}
