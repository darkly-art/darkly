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

pub const FLAG_CANVAS_SPACE: u32 = 1;
pub const FLAG_INVERT_COLOR: u32 = 2;

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
    pub _pad: [u32; 2],
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
            _pad: [0; 2],
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
    invert_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    prim_buf: wgpu::Buffer,
    prim_capacity: usize,
    sampler: wgpu::Sampler,
    /// 1×1 dummy texture — bound when no invert primitives are present,
    /// avoiding allocation of a viewport-sized snapshot texture.
    dummy_view: wgpu::TextureView,
    /// Viewport-sized snapshot for invert primitives (allocated on demand).
    snapshot: Option<wgpu::Texture>,
    snapshot_view: Option<wgpu::TextureView>,
    snapshot_size: (u32, u32),
    surface_format: wgpu::TextureFormat,
    primitives: Vec<OverlayPrimitive>,
    time: f32,
    /// Cached bind group from prepare(), valid until next prepare() call.
    bind_group: Option<wgpu::BindGroup>,
    /// Partition counts set by prepare().
    solid_count: u32,
    invert_count: u32,
}

impl ToolOverlay {
    pub fn new(
        device: &wgpu::Device,
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
                    // 3: sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
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

        let invert_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("overlay-invert"),
                layout: Some(&pipeline_layout),
                vertex: vertex_state,
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_invert"),
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

        ToolOverlay {
            solid_pipeline,
            invert_pipeline,
            bind_group_layout,
            uniform_buf,
            prim_buf,
            prim_capacity: initial_cap,
            sampler,
            dummy_view,
            snapshot: None,
            snapshot_view: None,
            snapshot_size: (0, 0),
            surface_format,
            primitives: Vec::new(),
            time: 0.0,
            bind_group: None,
            solid_count: 0,
            invert_count: 0,
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
        self.invert_count = 0;
    }

    /// Returns true if the overlay has content to render.
    pub fn has_content(&self) -> bool {
        !self.primitives.is_empty()
    }

    /// Returns true if any primitive uses the invert pipeline.
    pub fn has_invert(&self) -> bool {
        self.invert_count > 0
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
    // Split rendering: prepare() → draw_solid() / encode_invert()
    //
    // Solid primitives are drawn inside the caller's render pass (no extra
    // LoadOp::Load). Invert primitives (if any) get their own pass with a
    // snapshot copy, same as before.
    // -----------------------------------------------------------------------

    /// CPU-side work: partition, upload buffers, build bind group.
    /// Must be called once per frame before draw_solid() or encode_invert().
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
            self.invert_count = 0;
            self.bind_group = None;
            return;
        }

        // Partition: solid first, inverted second.
        self.primitives.sort_by_key(|p| p.flags & FLAG_INVERT_COLOR);
        self.solid_count = self.primitives.iter()
            .filter(|p| p.flags & FLAG_INVERT_COLOR == 0)
            .count() as u32;
        self.invert_count = self.primitives.len() as u32 - self.solid_count;

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

        // Choose texture view: dummy for solid-only, real snapshot for invert.
        let tex_view = if self.invert_count > 0 {
            self.ensure_snapshot(device, viewport_w, viewport_h);
            self.snapshot_view.as_ref().unwrap()
        } else {
            &self.dummy_view
        };

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

    /// Encode a separate render pass for invert primitives.
    /// Copies the current surface to the snapshot texture and draws invert
    /// primitives on top. Only call when has_invert() is true.
    pub fn encode_invert(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface_texture: &wgpu::Texture,
        surface_view: &wgpu::TextureView,
        viewport_w: u32,
        viewport_h: u32,
    ) {
        if self.invert_count == 0 {
            return;
        }

        let bg = self.bind_group.as_ref().expect("prepare() must be called before encode_invert()");

        // Copy surface → snapshot so fs_invert can sample the background.
        encoder.copy_texture_to_texture(
            surface_texture.as_image_copy(),
            self.snapshot.as_ref().unwrap().as_image_copy(),
            wgpu::Extent3d {
                width: viewport_w,
                height: viewport_h,
                depth_or_array_layers: 1,
            },
        );

        // Separate render pass for invert primitives.
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tool-overlay-invert"),
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

        rpass.set_pipeline(&self.invert_pipeline);
        rpass.set_bind_group(0, bg, &[]);
        rpass.draw(0..6, self.solid_count..(self.solid_count + self.invert_count));
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
        _ => f32::MAX,
    }
}
