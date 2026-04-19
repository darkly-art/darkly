use crate::gpu::view::ViewTransform;

// ---------------------------------------------------------------------------
// Primitive kinds (must match shaders/overlay.wgsl)
// ---------------------------------------------------------------------------

pub const KIND_LINE: u32 = 0;
pub const KIND_CIRCLE: u32 = 1;
pub const KIND_RECT: u32 = 2;
pub const KIND_DASHED_LINE: u32 = 3;
pub const KIND_FILLED_RECT: u32 = 4;
pub const KIND_FILLED_CIRCLE: u32 = 5;
pub const KIND_ELLIPSE: u32 = 6;
pub const KIND_FILLED_ELLIPSE: u32 = 7;
/// Rotated rect sampled from the bound mask texture. Coverage comes from the
/// mask's red channel — greyscale softness, speckles, textured tips all work
/// by construction. p0 = center, p1 = half-extent, rotation in radians.
pub const KIND_MASKED_STAMP: u32 = 8;

pub const FLAG_CANVAS_SPACE: u32 = 1;
pub const FLAG_INVERT_COLOR: u32 = 2;
pub const FLAG_SOFT_CONTRAST: u32 = 4;

/// Mask of flags that require snapshot-texture sampling (shared pipeline).
const FLAG_SNAPSHOT_MASK: u32 = FLAG_INVERT_COLOR | FLAG_SOFT_CONTRAST;

// ---------------------------------------------------------------------------
// GPU structs (must match shaders/overlay.wgsl layout exactly)
// ---------------------------------------------------------------------------

/// 64-byte SDF primitive descriptor, uploaded to a storage buffer.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct OverlayPrimitive {
    pub color: [f32; 4],
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub thickness: f32,
    pub dash_len: f32,
    pub dash_offset: f32,
    pub corner_radius: f32,
    pub kind: u32,
    pub flags: u32,
    /// Mode-dependent scalar parameter. For FLAG_SOFT_CONTRAST: tint strength
    /// in [0, 1] (typical 0.15); ignored otherwise.
    pub mode_param: f32,
    /// Rotation in radians. Used by KIND_MASKED_STAMP to orient the mask UVs.
    pub rotation: f32,
}

impl OverlayPrimitive {
    pub fn new(kind: u32, flags: u32, p0: [f32; 2], p1: [f32; 2]) -> Self {
        OverlayPrimitive {
            color: [1.0, 1.0, 1.0, 1.0],
            p0,
            p1,
            thickness: 1.0,
            dash_len: 0.0,
            dash_offset: 0.0,
            corner_radius: 0.0,
            kind,
            flags,
            mode_param: 0.0,
            rotation: 0.0,
        }
    }
}

/// Uniform block for overlay rendering (must match shader).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct OverlayUniforms {
    screen_size: [f32; 2],
    time: f32,
    _pad: f32,
    fwd_row0: [f32; 4],
    fwd_row1: [f32; 4],
    fwd_row2: [f32; 4],
    inv_row0: [f32; 4],
    inv_row1: [f32; 4],
    inv_row2: [f32; 4],
}

// ---------------------------------------------------------------------------
// ToolOverlay
// ---------------------------------------------------------------------------

pub struct ToolOverlay {
    solid_pipeline: wgpu::RenderPipeline,
    /// Snapshot-sampling pipeline: handles invert + soft-contrast primitives.
    snapshot_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    prim_buf: wgpu::Buffer,
    prim_capacity: usize,
    sampler: wgpu::Sampler,
    /// 1×1 dummy texture — bound when no snapshot-sampling primitives are
    /// present, avoiding allocation of a viewport-sized snapshot texture.
    dummy_view: wgpu::TextureView,
    /// Viewport-sized snapshot for snapshot-sampling primitives (allocated on demand).
    snapshot: Option<wgpu::Texture>,
    snapshot_view: Option<wgpu::TextureView>,
    snapshot_size: (u32, u32),
    /// 1×1 white fallback mask — bound when the user hasn't uploaded one.
    /// With it, KIND_MASKED_STAMP degrades to a solid rectangle.
    dummy_white_mask_view: wgpu::TextureView,
    /// User-uploaded mask texture (set via set_mask_texture). Sampled by
    /// KIND_MASKED_STAMP primitives to get the stamp shape + softness.
    mask: Option<wgpu::Texture>,
    mask_view: Option<wgpu::TextureView>,
    /// Preview mask texture owned by the overlay and used as a render
    /// target by brush nodes' `render_preview`. Separate from `mask` so
    /// CPU uploads and GPU renders don't stomp each other. Allocated on
    /// demand via `ensure_preview_mask`.
    preview_mask: Option<wgpu::Texture>,
    preview_mask_view: Option<wgpu::TextureView>,
    preview_mask_size: (u32, u32),
    surface_format: wgpu::TextureFormat,
    primitives: Vec<OverlayPrimitive>,
    time: f32,
    /// Cached bind group from prepare(), valid until next prepare() call.
    bind_group: Option<wgpu::BindGroup>,
    /// Partition counts set by prepare().
    solid_count: u32,
    snapshot_count: u32,
}

impl ToolOverlay {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("overlay-bgl"),
                entries: &[
                    // 0: uniforms
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // 1: primitives storage buffer
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // 2: snapshot texture (surface copy for background readback)
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // 3: sampler (shared by snapshot + mask)
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    // 4: mask texture (stamp shape + softness)
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
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

        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("overlay-layout"),
                bind_group_layouts: &[&bind_group_layout],
                immediate_size: 0,
            });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/overlay.wgsl").into(),
            ),
        });

        let vertex_state = wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        };

        // Both pipelines use standard premultiplied alpha blending.
        let alpha_blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let solid_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("overlay-solid"),
                layout: Some(&pipeline_layout),
                vertex: vertex_state.clone(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_solid"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(alpha_blend),
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

        let snapshot_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("overlay-snapshot"),
                layout: Some(&pipeline_layout),
                vertex: vertex_state,
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_snapshot"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(alpha_blend),
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

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay-uniforms"),
            size: std::mem::size_of::<OverlayUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let initial_cap = 64;
        let prim_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay-prims"),
            size: (initial_cap * std::mem::size_of::<OverlayPrimitive>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // 1×1 dummy texture — always available for solid-only bind groups.
        let dummy_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("overlay-dummy"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: surface_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dummy_view = dummy_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // 1×1 white fallback mask — used when no user mask is set. The red
        // channel samples as 1.0 so KIND_MASKED_STAMP becomes a solid rect.
        use wgpu::util::DeviceExt;
        let dummy_mask = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("overlay-dummy-mask"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255u8, 255, 255, 255],
        );
        let dummy_white_mask_view = dummy_mask.create_view(&wgpu::TextureViewDescriptor::default());

        ToolOverlay {
            solid_pipeline,
            snapshot_pipeline,
            bind_group_layout,
            uniform_buf,
            prim_buf,
            prim_capacity: initial_cap,
            sampler,
            dummy_view,
            snapshot: None,
            snapshot_view: None,
            snapshot_size: (0, 0),
            dummy_white_mask_view,
            mask: None,
            mask_view: None,
            preview_mask: None,
            preview_mask_view: None,
            preview_mask_size: (0, 0),
            surface_format,
            primitives: Vec::new(),
            time: 0.0,
            bind_group: None,
            solid_count: 0,
            snapshot_count: 0,
        }
    }

    /// Replace the current set of overlay primitives.
    pub fn set_primitives(&mut self, prims: Vec<OverlayPrimitive>) {
        self.primitives = prims;
    }

    /// Clear all overlay primitives.
    pub fn clear_primitives(&mut self) {
        self.primitives.clear();
        self.solid_count = 0;
        self.snapshot_count = 0;
    }

    /// Upload the stamp mask sampled by KIND_MASKED_STAMP primitives.
    /// Expects RGBA8 pixel data in row-major order (width*height*4 bytes). The
    /// red channel is used as grayscale coverage; other channels are ignored.
    /// Replaces any previously uploaded mask.
    pub fn set_mask_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) {
        assert_eq!(
            rgba.len(),
            (width * height * 4) as usize,
            "overlay mask: expected {} bytes for {width}x{height} RGBA8, got {}",
            width * height * 4,
            rgba.len(),
        );
        use wgpu::util::DeviceExt;
        let tex = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("overlay-mask"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        );
        self.mask_view = Some(tex.create_view(&wgpu::TextureViewDescriptor::default()));
        self.mask = Some(tex);
    }

    /// Clear the user-uploaded mask, falling back to the 1×1 white default.
    pub fn clear_mask_texture(&mut self) {
        self.mask = None;
        self.mask_view = None;
    }

    /// Ensure the preview-mask texture exists at the given dimensions, then
    /// return a view a brush node can render into. Reallocates only when
    /// size changes; otherwise returns the existing view.
    ///
    /// Allocated with RENDER_ATTACHMENT + TEXTURE_BINDING usage (RGBA8Unorm)
    /// so nodes can render into it and the overlay can sample it as a mask.
    pub fn ensure_preview_mask(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> &wgpu::TextureView {
        if self.preview_mask_size != (width, height) || self.preview_mask.is_none() {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("overlay-preview-mask"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            self.preview_mask = Some(tex);
            self.preview_mask_view = Some(view);
            self.preview_mask_size = (width, height);
        }
        self.preview_mask_view.as_ref().unwrap()
    }

    /// Point the overlay's mask binding at the preview-mask texture so
    /// subsequent `prepare()` calls bind it as the KIND_MASKED_STAMP source.
    pub fn use_preview_mask_as_mask(&mut self) {
        if let Some(view) = &self.preview_mask_view {
            self.mask_view = Some(view.clone());
            // Drop any CPU-uploaded texture — preview-mask is now authoritative.
            self.mask = None;
        }
    }

    /// Stop using the preview mask as the overlay mask source (falls back
    /// to the 1×1 white default). Does not free the preview texture.
    pub fn clear_preview_mask(&mut self) {
        self.mask = None;
        self.mask_view = None;
    }

    /// Current preview mask dimensions (0,0 if never allocated).
    pub fn preview_mask_size(&self) -> (u32, u32) {
        self.preview_mask_size
    }

    /// Access the preview-mask texture (for engines that need the Texture,
    /// not just the view, e.g. for a BrushGpuContext's canvas_texture slot).
    pub fn preview_mask_texture(&self) -> Option<&wgpu::Texture> {
        self.preview_mask.as_ref()
    }

    /// Returns true if the overlay has content to render.
    pub fn has_content(&self) -> bool {
        !self.primitives.is_empty()
    }

    /// Returns true if any primitive uses the snapshot-sampling pipeline
    /// (invert or soft-contrast modes).
    pub fn has_snapshot(&self) -> bool {
        self.snapshot_count > 0
    }

    /// Returns true if any primitive is animating (dashed lines).
    pub fn needs_animation(&self) -> bool {
        self.primitives.iter().any(|p| p.kind == KIND_DASHED_LINE && p.dash_len > 0.0)
    }

    /// Advance overlay animation time by the given delta.
    /// Called by the compositor's frame scheduler on overlay-scheduled frames.
    /// No throttle — the frame scheduler handles rate limiting.
    pub fn advance_time(&mut self, dt: f32) {
        self.time += dt;
    }

    /// Ensure the snapshot texture exists at the given viewport size.
    fn ensure_snapshot(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        if self.snapshot_size == (w, h) && self.snapshot.is_some() {
            return;
        }
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("overlay-snapshot"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.surface_format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.snapshot = Some(tex);
        self.snapshot_view = Some(view);
        self.snapshot_size = (w, h);
    }

    // -----------------------------------------------------------------------
    // Split rendering: prepare() → draw_solid() / encode_snapshot()
    //
    // Solid primitives are drawn inside the caller's render pass (no extra
    // LoadOp::Load). Snapshot-sampling primitives (invert + soft-contrast)
    // get their own pass with a surface→snapshot copy.
    // -----------------------------------------------------------------------

    /// CPU-side work: partition, upload buffers, build bind group.
    /// Must be called once per frame before draw_solid() or encode_snapshot().
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view_transform: &ViewTransform,
        viewport_w: u32,
        viewport_h: u32,
    ) {
        if self.primitives.is_empty() {
            self.solid_count = 0;
            self.snapshot_count = 0;
            self.bind_group = None;
            return;
        }

        // Partition: solid first, snapshot-sampling (invert + soft) second.
        self.primitives.sort_by_key(|p| (p.flags & FLAG_SNAPSHOT_MASK) != 0);
        self.solid_count = self.primitives.iter()
            .filter(|p| p.flags & FLAG_SNAPSHOT_MASK == 0)
            .count() as u32;
        self.snapshot_count = self.primitives.len() as u32 - self.solid_count;

        // Grow primitive buffer if needed.
        let count = self.primitives.len();
        if count > self.prim_capacity {
            let new_cap = count.next_power_of_two();
            self.prim_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay-prims"),
                size: (new_cap * std::mem::size_of::<OverlayPrimitive>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.prim_capacity = new_cap;
        }

        // Upload primitives.
        queue.write_buffer(&self.prim_buf, 0, bytemuck::cast_slice(&self.primitives));

        // Upload uniforms.
        let fwd = forward_from_inverse(view_transform);
        let inv = &view_transform.matrix;
        let uniforms = OverlayUniforms {
            screen_size: [viewport_w as f32, viewport_h as f32],
            time: self.time,
            _pad: 0.0,
            fwd_row0: fwd[0],
            fwd_row1: fwd[1],
            fwd_row2: fwd[2],
            inv_row0: inv[0],
            inv_row1: inv[1],
            inv_row2: inv[2],
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        // Choose texture view: dummy for solid-only, real snapshot when any
        // snapshot-sampling primitive (invert or soft-contrast) is present.
        let tex_view = if self.snapshot_count > 0 {
            self.ensure_snapshot(device, viewport_w, viewport_h);
            self.snapshot_view.as_ref().unwrap()
        } else {
            &self.dummy_view
        };

        // Pick mask view: user-uploaded if present, else 1×1 white fallback.
        let mask_view = self.mask_view.as_ref().unwrap_or(&self.dummy_white_mask_view);

        // Build bind group.
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay-bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.uniform_buf,
                        offset: 0,
                        size: None,
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.prim_buf,
                        offset: 0,
                        size: None,
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(tex_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(mask_view),
                },
            ],
        }));
    }

    /// Draw solid overlay primitives into an existing render pass.
    /// Call after prepare(). Does not create a render pass — the caller
    /// provides one (e.g. the final present or veil-blit pass).
    pub fn draw_solid<'a>(&'a self, rpass: &mut wgpu::RenderPass<'a>) {
        if self.solid_count == 0 {
            return;
        }
        let bg = self.bind_group.as_ref().expect("prepare() must be called before draw_solid()");
        rpass.set_pipeline(&self.solid_pipeline);
        rpass.set_bind_group(0, bg, &[]);
        rpass.draw(0..6, 0..self.solid_count);
    }

    /// Encode a separate render pass for snapshot-sampling primitives
    /// (invert + soft-contrast). Copies the current surface to the snapshot
    /// texture and draws them on top. Only call when has_snapshot() is true.
    pub fn encode_snapshot(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface_texture: &wgpu::Texture,
        surface_view: &wgpu::TextureView,
        viewport_w: u32,
        viewport_h: u32,
    ) {
        if self.snapshot_count == 0 {
            return;
        }

        let bg = self.bind_group.as_ref().expect("prepare() must be called before encode_snapshot()");

        // Copy surface → snapshot so fs_snapshot can sample the background.
        encoder.copy_texture_to_texture(
            surface_texture.as_image_copy(),
            self.snapshot.as_ref().unwrap().as_image_copy(),
            wgpu::Extent3d {
                width: viewport_w,
                height: viewport_h,
                depth_or_array_layers: 1,
            },
        );

        // Separate render pass for snapshot-sampling primitives.
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tool-overlay-snapshot"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });

        rpass.set_pipeline(&self.snapshot_pipeline);
        rpass.set_bind_group(0, bg, &[]);
        rpass.draw(0..6, self.solid_count..(self.solid_count + self.snapshot_count));
    }

    /// CPU-side hit test: returns the index of the first primitive hit at the
    /// given screen-space point, if any.
    pub fn hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize> {
        let p = [screen_x, screen_y];
        for (i, prim) in self.primitives.iter().enumerate() {
            // Only test screen-space primitives; canvas-space prims need the
            // view transform which we don't cache here.
            if prim.flags & FLAG_CANVAS_SPACE != 0 {
                continue;
            }
            let dist = cpu_sdf(prim, p);
            if dist <= prim.thickness * 0.5 + 4.0 {
                return Some(i);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Helper: compute forward (canvas → screen) from inverse (screen → canvas)
// ---------------------------------------------------------------------------

fn forward_from_inverse(vt: &ViewTransform) -> [[f32; 4]; 3] {
    let m = &vt.matrix;
    let m00 = m[0][0];
    let m01 = m[0][1];
    let m10 = m[1][0];
    let m11 = m[1][1];
    let tx = m[2][0];
    let ty = m[2][1];

    let det = m00 * m11 - m10 * m01;
    if det.abs() < 1e-12 {
        return [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
        ];
    }
    let inv_det = 1.0 / det;

    let f00 = m11 * inv_det;
    let f01 = -m10 * inv_det;
    let f10 = -m01 * inv_det;
    let f11 = m00 * inv_det;
    let ftx = -(f00 * tx + f01 * ty);
    let fty = -(f10 * tx + f11 * ty);

    [
        [f00, f01, 0.0, 0.0],
        [f10, f11, 0.0, 0.0],
        [ftx, fty, 0.0, 0.0],
    ]
}

// ---------------------------------------------------------------------------
// CPU-side SDF for hit testing — delegates to shared sdf module
// ---------------------------------------------------------------------------

fn cpu_sdf(prim: &OverlayPrimitive, p: [f32; 2]) -> f32 {
    use crate::sdf;
    match prim.kind {
        KIND_LINE | KIND_DASHED_LINE => {
            sdf::sdf_segment(p[0], p[1], prim.p0[0], prim.p0[1], prim.p1[0], prim.p1[1])
        }
        KIND_CIRCLE => {
            sdf::sdf_circle(p[0], p[1], prim.p0[0], prim.p0[1], prim.p1[0]).abs()
        }
        KIND_FILLED_CIRCLE => {
            sdf::sdf_circle(p[0], p[1], prim.p0[0], prim.p0[1], prim.p1[0])
        }
        KIND_RECT | KIND_FILLED_RECT => {
            let cx = (prim.p0[0] + prim.p1[0]) * 0.5;
            let cy = (prim.p0[1] + prim.p1[1]) * 0.5;
            let hw = (prim.p1[0] - prim.p0[0]) * 0.5;
            let hh = (prim.p1[1] - prim.p0[1]) * 0.5;
            let d = sdf::sdf_rounded_rect(p[0], p[1], cx, cy, hw, hh, prim.corner_radius);
            if prim.kind == KIND_RECT { d.abs() } else { d }
        }
        KIND_ELLIPSE => {
            // p0 = center, p1 = [rx, ry]
            sdf::sdf_ellipse(p[0], p[1], prim.p0[0], prim.p0[1], prim.p1[0], prim.p1[1]).abs()
        }
        KIND_FILLED_ELLIPSE => {
            // p0 = center, p1 = [rx, ry] — interior is signed-negative
            sdf::sdf_ellipse(p[0], p[1], prim.p0[0], prim.p0[1], prim.p1[0], prim.p1[1])
        }
        _ => f32::MAX,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_utils::test_device;

    fn make_overlay() -> ToolOverlay {
        let (device, queue) = test_device();
        ToolOverlay::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm)
    }

    #[test]
    fn partition_groups_solid_and_snapshot() {
        let (device, queue) = test_device();
        let mut overlay = ToolOverlay::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);

        // Mix: one solid line, one invert rect, one soft-contrast filled ellipse.
        // Deliberately interleave so the sort has work to do.
        let solid = OverlayPrimitive::new(KIND_LINE, 0, [0.0, 0.0], [100.0, 0.0]);
        let invert = OverlayPrimitive::new(KIND_RECT, FLAG_INVERT_COLOR, [10.0, 10.0], [50.0, 50.0]);
        let soft = {
            let mut p = OverlayPrimitive::new(
                KIND_FILLED_ELLIPSE,
                FLAG_SOFT_CONTRAST,
                [200.0, 200.0], [40.0, 30.0],
            );
            p.mode_param = 0.15;
            p
        };
        overlay.set_primitives(vec![soft, solid, invert]);

        let vt = ViewTransform::identity();
        overlay.prepare(&device, &queue, &vt, 512, 512);

        assert_eq!(overlay.solid_count, 1, "one solid primitive");
        assert_eq!(overlay.snapshot_count, 2, "invert + soft share the snapshot batch");
        assert!(overlay.has_snapshot(), "snapshot pass required");
        assert!(overlay.has_content());

        // Partition ordering: solid first, snapshot-sampling second.
        assert_eq!(overlay.primitives[0].flags & FLAG_SNAPSHOT_MASK, 0);
        assert_ne!(overlay.primitives[1].flags & FLAG_SNAPSHOT_MASK, 0);
        assert_ne!(overlay.primitives[2].flags & FLAG_SNAPSHOT_MASK, 0);
    }

    #[test]
    fn partition_solid_only_skips_snapshot() {
        let (device, queue) = test_device();
        let mut overlay = ToolOverlay::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);

        overlay.set_primitives(vec![
            OverlayPrimitive::new(KIND_LINE, 0, [0.0, 0.0], [1.0, 1.0]),
            OverlayPrimitive::new(KIND_FILLED_RECT, 0, [0.0, 0.0], [10.0, 10.0]),
        ]);

        let vt = ViewTransform::identity();
        overlay.prepare(&device, &queue, &vt, 256, 256);

        assert_eq!(overlay.solid_count, 2);
        assert_eq!(overlay.snapshot_count, 0);
        assert!(!overlay.has_snapshot());
        assert!(overlay.snapshot.is_none(), "no snapshot texture allocated when unused");
    }

    #[test]
    fn filled_ellipse_cpu_sdf_interior_vs_exterior() {
        // Purely a CPU-side hit-test sanity check for the new kind.
        let mut prim = OverlayPrimitive::new(
            KIND_FILLED_ELLIPSE,
            0,
            [100.0, 100.0], [30.0, 20.0],
        );
        prim.thickness = 0.0;

        let center = cpu_sdf(&prim, [100.0, 100.0]);
        let edge_x = cpu_sdf(&prim, [130.0, 100.0]);
        let outside = cpu_sdf(&prim, [200.0, 100.0]);

        assert!(center < 0.0, "center is interior: {center}");
        assert!(edge_x.abs() < 1.0, "edge is near zero: {edge_x}");
        assert!(outside > 0.0, "outside is positive: {outside}");
    }

    #[test]
    fn clear_primitives_resets_counts() {
        let mut overlay = make_overlay();
        overlay.set_primitives(vec![
            OverlayPrimitive::new(KIND_LINE, 0, [0.0, 0.0], [10.0, 10.0]),
        ]);
        overlay.solid_count = 1; // simulate post-prepare state
        overlay.snapshot_count = 0;

        overlay.clear_primitives();
        assert_eq!(overlay.solid_count, 0);
        assert_eq!(overlay.snapshot_count, 0);
        assert!(!overlay.has_content());
    }

    #[test]
    fn overlay_primitive_is_64_bytes() {
        // The WGSL struct is declared 64 bytes, std430-aligned. Any deviation
        // will cause a shader-side aliasing bug.
        assert_eq!(std::mem::size_of::<OverlayPrimitive>(), 64);
    }

    /// GPU integration: render a soft-contrast filled ellipse over a pure
    /// red surface and verify desaturation — the interior should shift
    /// toward grey (R drops significantly, G and B rise from 0).
    #[test]
    fn soft_contrast_desaturates_bg() {
        use crate::gpu::test_utils::{create_test_texture_with_format, readback_texture};

        let (device, queue) = test_device();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut overlay = ToolOverlay::new(&device, &queue, format);

        // Saturated red surface: only desaturation has anything to shift.
        const W: u32 = 64;
        const H: u32 = 16;
        let mut pixels = vec![0u8; (W * H * 4) as usize];
        for i in (0..pixels.len()).step_by(4) {
            pixels[i] = 255;     // R = 1
            pixels[i+1] = 0;
            pixels[i+2] = 0;
            pixels[i+3] = 255;
        }
        let (surface_tex, surface_view) =
            create_test_texture_with_format(&device, &queue, W, H, &pixels, format);

        // Full-strength desat over the center.
        let mut prim = OverlayPrimitive::new(
            KIND_FILLED_ELLIPSE,
            FLAG_SOFT_CONTRAST,
            [(W as f32) * 0.5, (H as f32) * 0.5],
            [12.0, 6.0],
        );
        prim.mode_param = 1.0;  // fully grey
        overlay.set_primitives(vec![prim]);

        let vt = ViewTransform::identity();
        overlay.prepare(&device, &queue, &vt, W, H);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("soft-contrast-desat-test"),
        });
        overlay.encode_snapshot(&mut encoder, &surface_tex, &surface_view, W, H);
        queue.submit([encoder.finish()]);

        let out = readback_texture(&device, &queue, &surface_tex, format, W, H);
        let px = |x: u32, y: u32| -> [u8; 4] {
            let i = ((y * W + x) * 4) as usize;
            [out[i], out[i+1], out[i+2], out[i+3]]
        };

        let inside = px(32, 8);   // center of the ellipse
        let outside = px(2, 8);   // well outside

        assert_eq!(outside, [255, 0, 0, 255], "red bg unchanged outside ellipse");

        // Combined shift at mode_param=1:
        //   lum_shift toward white (red has lum=0.21 < 0.5): mix(red, white, 1) = white
        //   desat toward bg_gray=(0.21..): mix(white, 0.21, 0.5) ≈ 0.605 = (154, 154, 154).
        // Key property: R drops hard (dominant-channel desat) AND G/B rise.
        assert!(inside[0] < 200, "R should drop substantially: got {inside:?}");
        assert!(inside[1] > 100, "G should rise well off 0: got {inside:?}");
        assert!(inside[2] > 100, "B should rise well off 0: got {inside:?}");
        // All channels should land close (approximately grey).
        let spread = (inside[0] as i32 - inside[1] as i32).abs();
        assert!(spread < 15, "result should be near-grey: {inside:?}");
    }

    /// GPU integration: KIND_MASKED_STAMP — coverage comes from the uploaded
    /// mask texture. Upload a mask with two clearly-separated regions (mostly
    /// black left, mostly white right) and verify over a red surface that
    /// the white-mask region gets desaturated (R drops, G/B rise) while
    /// the black-mask region leaves the surface unchanged.
    #[test]
    fn masked_stamp_uses_mask_red_as_coverage() {
        use crate::gpu::test_utils::{create_test_texture_with_format, readback_texture};

        let (device, queue) = test_device();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut overlay = ToolOverlay::new(&device, &queue, format);

        // Red surface: desaturation shifts R→0.21 gray, G/B rise from 0.
        const W: u32 = 64;
        const H: u32 = 16;
        let mut bg = vec![0u8; (W * H * 4) as usize];
        for i in (0..bg.len()).step_by(4) {
            bg[i] = 255;      // R
            bg[i+3] = 255;    // A
        }
        let (surface_tex, surface_view) =
            create_test_texture_with_format(&device, &queue, W, H, &bg, format);

        // 16×1 mask: left half black (coverage 0), right half white (coverage 1).
        // A wide mask avoids linear-filter bleed near the sample points.
        let mut mask = vec![0u8; 16 * 1 * 4];
        for x in 8..16 {
            let i = x * 4;
            mask[i] = 255; mask[i+1] = 255; mask[i+2] = 255; mask[i+3] = 255;
        }
        overlay.set_mask_texture(&device, &queue, 16, 1, &mask);

        // Stamp in screen space; half-extent 20 × 6 → UV.x in [0, 1] across
        // screen_x in [12, 52]. Sample at x=16 (far left, mask=0) and x=48
        // (far right, mask=1).
        let mut prim = OverlayPrimitive::new(
            KIND_MASKED_STAMP,
            FLAG_SOFT_CONTRAST,
            [(W as f32) * 0.5, (H as f32) * 0.5],
            [20.0, 6.0],
        );
        prim.mode_param = 0.6;
        overlay.set_primitives(vec![prim]);

        let vt = ViewTransform::identity();
        overlay.prepare(&device, &queue, &vt, W, H);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("masked-stamp-test"),
        });
        overlay.encode_snapshot(&mut encoder, &surface_tex, &surface_view, W, H);
        queue.submit([encoder.finish()]);

        let out = readback_texture(&device, &queue, &surface_tex, format, W, H);
        let px = |x: u32, y: u32| -> [u8; 4] {
            let i = ((y * W + x) * 4) as usize;
            [out[i], out[i+1], out[i+2], out[i+3]]
        };

        let left = px(16, 8);     // inside stamp, mask ≈ 0 → coverage ≈ 0
        let right = px(48, 8);    // inside stamp, mask ≈ 1 → coverage ≈ 1
        let outside = px(2, 8);   // outside stamp bounds

        assert_eq!(outside, [255, 0, 0, 255], "red bg unchanged outside stamp");

        // Left half: mask=0 → no desaturation → still pure red.
        assert!(left[0] >= 250 && left[1] <= 5 && left[2] <= 5,
            "left half (mask=0) should stay pure red: got {left:?}");

        // Right half: mask=1 with strength=0.6 → 60% desat.
        // R goes from 255 toward ~54 (red's gray point): 255*0.4 + 54*0.6 ≈ 134.
        // G/B go from 0 toward ~54: 0*0.4 + 54*0.6 ≈ 32.
        assert!(right[0] < 200, "R should fall: got {right:?}");
        assert!(right[1] > 15, "G should rise from 0: got {right:?}");
        assert!(right[2] > 15, "B should rise from 0: got {right:?}");
    }
}
