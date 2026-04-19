use crate::gpu::atlas::LayerTexture;
use crate::gpu::blend::BlendPipelines;
use crate::gpu::content_bounds::ContentBoundsPass;
use crate::gpu::overlay::{OverlayPrimitive, ToolOverlay};
use crate::gpu::veil_chain::VeilChain;
use crate::gpu::view::ViewTransform;
use crate::document::{Document, ROOT_ID};
use crate::layer::{BlendMode, Layer, LayerId, LayerNode};
use std::collections::HashMap;

/// Timing helpers — compile to no-ops unless `cfg(feature = "profile")`.
#[cfg(feature = "profile")]
mod perf {
    pub fn time(label: &str) {
        log::trace!("[perf] {label} start");
    }
    pub fn time_end(label: &str) {
        log::trace!("[perf] {label} end");
    }
}
#[cfg(not(feature = "profile"))]
mod perf {
    #[inline(always)]
    pub fn time(_: &str) {}
    #[inline(always)]
    pub fn time_end(_: &str) {}
}

/// A pair of accumulator textures for ping-pong compositing within a group.
struct AccumPair {
    textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
}

/// GPU state for a non-passthrough group (including root).
/// Every group that composites its children to an isolated buffer owns one.
struct GroupState {
    /// Ping-pong accumulator pair for compositing children.
    accum: AccumPair,
    /// Tracks which accumulator is the current "source" (last written).
    current_accum: usize,
    /// Cached final composite result of this group's children.
    composite_cache: wgpu::Texture,
    composite_cache_view: wgpu::TextureView,
    /// Child index through which the cache is valid.
    /// None = cache is empty, must composite from scratch.
    cache_valid_through: Option<usize>,
    /// Uniform buffer holding opacity, blend_mode, show_mask for blending
    /// this group's result into its parent.
    uniform_buf: wgpu::Buffer,
}

/// Pre-built GPU objects for a raster layer.
struct RasterLayerCache {
    /// Uniform buffer holding opacity + blend_mode + show_mask.
    uniform_buf: wgpu::Buffer,
}

/// Uniforms for raster layer compositing.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct BlendUniforms {
    opacity: f32,
    blend_mode: u32,
    show_mask: u32,
    _pad1: f32,
}

/// GPU state for a passthrough group that has a mask (Photoshop-style).
/// Stores a snapshot texture for the parent accumulator and a uniform buffer
/// for the lerp pass.
struct PassthroughMaskState {
    /// Snapshot of the parent accumulator before compositing this group's children.
    snapshot: wgpu::Texture,
    snapshot_view: wgpu::TextureView,
    /// Uniform buffer for the mask lerp shader (show_mask flag).
    uniform_buf: wgpu::Buffer,
}

pub struct Compositor {
    /// Per-group GPU state. Every non-passthrough group (including root)
    /// owns a GroupState with its own accumulators and composite cache.
    /// Root's state lives at group_state[ROOT_ID].
    group_state: HashMap<LayerId, GroupState>,

    /// Per-layer GPU textures (one per raster layer).
    layer_textures: HashMap<LayerId, LayerTexture>,

    /// Per-layer/group mask GPU textures (R8Unorm, one per entity with a mask).
    mask_textures: HashMap<LayerId, LayerTexture>,
    /// Default mask bind group using the 1x1 white texture.
    default_mask_bind_group: wgpu::BindGroup,

    /// Resolved mask bind group per entity. Points to the real mask texture
    /// when mask_enabled/show_mask, or absent (falls back to default_mask_bind_group).
    /// Single source of truth — no other struct caches a mask bind group.
    mask_bind_groups: HashMap<LayerId, wgpu::BindGroup>,

    /// Pre-built GPU objects per raster layer.
    raster_cache: HashMap<LayerId, RasterLayerCache>,

    blend_pipelines: BlendPipelines,

    // --- Passthrough Group Mask (Photoshop-style snapshot-lerp) ---
    mask_lerp_pipeline: wgpu::RenderPipeline,
    /// Per-group GPU state for passthrough groups with masks.
    passthrough_mask_state: HashMap<LayerId, PassthroughMaskState>,

    present_pipeline: wgpu::RenderPipeline,
    /// Present pipeline targeting the accum format (Rgba8Unorm) for veil input.
    present_to_veil_pipeline: wgpu::RenderPipeline,
    _present_bind_group_layout: wgpu::BindGroupLayout,
    /// Present bind group that reads from root's composite_cache.
    present_cache_bind_group: wgpu::BindGroup,
    /// View transform uniform buffer for the present shader.
    view_uniform_buf: wgpu::Buffer,

    sampler: wgpu::Sampler,

    /// Dirty gate — false means nothing changed, skip compositing.
    needs_composite: bool,
    /// When only the view transform changes, skip compositing and only re-present.
    needs_present: bool,

    canvas_width: u32,
    canvas_height: u32,
    /// Padded (tile-aligned) render target dimensions — used for shader UV
    /// computations in the transform pass, which must match the actual
    /// accumulator texture size.
    padded_width: u32,
    padded_height: u32,

    veil_chain: VeilChain,

    // --- Floating Content Transform ---
    transform_pass: crate::gpu::transform::TransformPass,

    // --- Tool Overlay ---
    tool_overlay: ToolOverlay,
    /// Cached view transform for overlay forward matrix computation.
    cached_view_transform: ViewTransform,

    // --- Content Bounds (GPU compute) ---
    content_bounds: ContentBoundsPass,

    // --- Frame Scheduler ---
    /// Monotonic frame counter, incremented on each rAF tick.
    /// Systems fire when `frame_count % divisor == 0`.
    frame_count: u64,
    /// Last wall-clock time for dt computation.
    last_wall_time: f32,
}

impl Compositor {
    /// Create an accumulator texture at padded canvas dimensions.
    fn make_accum_texture(
        device: &wgpu::Device,
        padded_w: u32,
        padded_h: u32,
        label: &str,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: padded_w,
                height: padded_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        (tex, view)
    }

    /// Create a GroupState (accum pair + composite cache + uniforms).
    fn create_group_state(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        padded_w: u32,
        padded_h: u32,
        group_id: LayerId,
    ) -> GroupState {
        let (a0, v0) = Self::make_accum_texture(device, padded_w, padded_h, &format!("accum-{group_id}-0"));
        let (a1, v1) = Self::make_accum_texture(device, padded_w, padded_h, &format!("accum-{group_id}-1"));
        let (cache, cache_view) = Self::make_accum_texture(device, padded_w, padded_h, &format!("cache-{group_id}"));

        let uniforms = BlendUniforms {
            opacity: 1.0,
            blend_mode: BlendMode::Normal as u32,
            show_mask: 0,
            _pad1: 0.0,
        };
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("group-uniforms-{group_id}")),
            size: std::mem::size_of::<BlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        GroupState {
            accum: AccumPair {
                textures: [a0, a1],
                views: [v0, v1],
            },
            current_accum: 0,
            composite_cache: cache,
            composite_cache_view: cache_view,
            cache_valid_through: None,
            uniform_buf,
        }
    }

    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        is_software: bool,
    ) -> Self {
        // Accumulator dimensions match layer textures exactly (no tile padding).
        let padded_w = width;
        let padded_h = height;

        let accum_format = wgpu::TextureFormat::Rgba8Unorm;

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("darkly-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let blend_pipelines = BlendPipelines::new(device, accum_format);

        // Create default 1x1 white mask texture (mask_alpha=1.0 = no effect)
        let default_mask_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("default-mask-1x1"),
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
                texture: &default_mask_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(1), rows_per_image: None },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        let default_mask_view = default_mask_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let default_mask_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("default-mask-bg"),
            layout: &blend_pipelines.mask_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&default_mask_view),
            }],
        });

        // --- Mask lerp pipeline (passthrough groups with masks) ---
        // Reuses the blend BGL for group 0 (before, after, sampler, uniforms)
        // and the mask BGL for group 1 (mask texture).
        let mask_lerp_pipeline = {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("mask-lerp-shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../../shaders/mask_lerp.wgsl").into(),
                ),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("mask-lerp-pipeline-layout"),
                bind_group_layouts: &[
                    &blend_pipelines.bind_group_layout,
                    &blend_pipelines.mask_bind_group_layout,
                ],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mask-lerp-pipeline"),
                layout: Some(&layout),
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
            })
        };
        // View transform uniform buffer (present shader binding 2)
        let view_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view-transform-uniform"),
            size: std::mem::size_of::<ViewTransform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let identity = ViewTransform::identity();
        queue.write_buffer(&view_uniform_buf, 0, bytemuck::bytes_of(&identity));

        // Present pipeline: blit accumulator to surface
        let _present_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("present-bgl"),
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

        let present_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("present-pipeline-layout"),
                bind_group_layouts: &[&_present_bind_group_layout],
                immediate_size: 0,
            });

        let present_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/present.wgsl").into(),
            ),
        });

        let present_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present-pipeline"),
            layout: Some(&present_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &present_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &present_shader,
                entry_point: Some("fs_present"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
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

        let accum_format = wgpu::TextureFormat::Rgba8Unorm;
        let present_to_veil_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("present-to-veil-pipeline"),
                layout: Some(&present_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &present_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &present_shader,
                    entry_point: Some("fs_present"),
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

        // Create root GroupState (root is always a non-passthrough group)
        let root_state = Self::create_group_state(
            device, queue, padded_w, padded_h, ROOT_ID,
        );

        // Present bind group reads from root's composite cache
        let present_cache_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present-bg-cache"),
            layout: &_present_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&root_state.composite_cache_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: view_uniform_buf.as_entire_binding(),
                },
            ],
        });

        let mut group_state = HashMap::new();
        group_state.insert(ROOT_ID, root_state);

        let veil_chain = VeilChain::new(
            device,
            sampler.clone(),
            surface_format,
            accum_format,
            is_software,
        );

        let tool_overlay = ToolOverlay::new(device, queue, surface_format);

        let transform_pass = crate::gpu::transform::TransformPass::new(device, accum_format);
        let content_bounds = ContentBoundsPass::new(device);

        Compositor {
            group_state,
            layer_textures: HashMap::new(),
            mask_textures: HashMap::new(),
            default_mask_bind_group,
            mask_bind_groups: HashMap::new(),
            raster_cache: HashMap::new(),
            blend_pipelines,
            mask_lerp_pipeline,
            passthrough_mask_state: HashMap::new(),
            present_pipeline,
            present_to_veil_pipeline,
            _present_bind_group_layout,
            present_cache_bind_group,
            view_uniform_buf,
            sampler,
            needs_composite: true,
            needs_present: false,
            canvas_width: width,
            canvas_height: height,
            padded_width: padded_w,
            padded_height: padded_h,
            veil_chain,
            transform_pass,
            content_bounds,
            tool_overlay,
            cached_view_transform: identity,
            frame_count: 0,
            last_wall_time: 0.0,
        }
    }

    /// Create GPU texture + uniform buffer for a new raster layer.
    /// Called once when a layer is added, never in the render loop.
    pub fn ensure_raster_layer(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, layer_id: LayerId) {
        if self.layer_textures.contains_key(&layer_id) {
            return;
        }

        let layer_tex = LayerTexture::new(device, self.canvas_width, self.canvas_height);

        let uniforms = BlendUniforms {
            opacity: 1.0,
            blend_mode: BlendMode::Normal as u32,
            show_mask: 0,
            _pad1: 0.0,
        };

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("blend-uniforms-{layer_id}")),
            size: std::mem::size_of::<BlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        self.raster_cache.insert(
            layer_id,
            RasterLayerCache {
                uniform_buf,
            },
        );
        self.layer_textures.insert(layer_id, layer_tex);
    }

    /// Ensure a non-passthrough group has GPU state allocated.
    /// Called when a group is created or switches from passthrough to normal.
    pub fn ensure_group_state(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, group_id: LayerId) {
        if self.group_state.contains_key(&group_id) {
            return;
        }
        let gs = Self::create_group_state(
            device, queue, self.padded_width, self.padded_height, group_id,
        );
        self.group_state.insert(group_id, gs);
    }

    /// Update a group's blend uniforms (opacity, blend_mode).
    pub fn update_group_uniforms(
        &mut self,
        queue: &wgpu::Queue,
        group_id: LayerId,
        opacity: f32,
        blend_mode: BlendMode,
        show_mask: bool,
    ) {
        if let Some(gs) = self.group_state.get(&group_id) {
            let uniforms = BlendUniforms {
                opacity,
                blend_mode: blend_mode as u32,
                show_mask: show_mask as u32,
                _pad1: 0.0,
            };
            queue.write_buffer(&gs.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        }
        // Also update passthrough mask state uniform (show_mask only).
        if let Some(pms) = self.passthrough_mask_state.get(&group_id) {
            let val = show_mask as u32;
            queue.write_buffer(&pms.uniform_buf, 0, bytemuck::bytes_of(&val));
        }
    }

    /// Mark that recompositing is needed.
    pub fn mark_dirty(&mut self) {
        self.needs_composite = true;
        // Invalidate all group caches
        for gs in self.group_state.values_mut() {
            gs.cache_valid_through = None;
        }
        // Invalidate all layer content bounds — pixels may have changed.
        self.content_bounds.invalidate_all();
    }

    /// Mark that only the present pass needs to re-run (view transform changed).
    /// Skips compositing when there are no dirty tiles or layer changes.
    pub fn mark_needs_present(&mut self) {
        self.needs_present = true;
    }

    // --- Content Bounds (GPU compute) ---

    /// Return cached content bounds for a layer: `[x, y, w, h]`.
    /// Returns `None` if bounds haven't been computed yet or were invalidated.
    pub fn content_bounds(&self, layer_id: LayerId) -> Option<[u32; 4]> {
        self.content_bounds.get(layer_id)
    }

    /// Request async content bounds computation for a layer.
    /// Results arrive on the next frame — retrieve via [`content_bounds`].
    pub fn request_content_bounds(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        is_mask: bool,
    ) {
        let (view, w, h) = if is_mask {
            match self.mask_textures.get(&layer_id) {
                Some(t) => (&t.view, self.canvas_width, self.canvas_height),
                None => return,
            }
        } else {
            match self.layer_textures.get(&layer_id) {
                Some(t) => (&t.view, self.canvas_width, self.canvas_height),
                None => return,
            }
        };
        self.content_bounds.request(device, queue, view, w, h, is_mask, layer_id);
    }

    /// Poll pending content bounds computations. Call once per frame.
    /// Returns layer IDs whose bounds just became available.
    pub fn poll_content_bounds(&mut self, device: &wgpu::Device) -> Vec<LayerId> {
        self.content_bounds.poll(device)
    }

    /// True if any content bounds computations are in flight.
    pub fn has_pending_content_bounds(&self) -> bool {
        self.content_bounds.has_pending()
    }

    /// True if a content bounds computation is in flight for a specific layer.
    pub fn is_content_bounds_pending(&self, layer_id: LayerId) -> bool {
        self.content_bounds.is_pending(layer_id)
    }

    // --- Paint Target Accessors (Phase 2: GPU brush) ---

    /// Get a reference to a layer's GPU texture.
    pub fn layer_texture(&self, layer_id: LayerId) -> Option<&LayerTexture> {
        self.layer_textures.get(&layer_id)
    }

    /// Get a reference to a layer's mask GPU texture.
    pub fn mask_texture(&self, layer_id: LayerId) -> Option<&LayerTexture> {
        self.mask_textures.get(&layer_id)
    }

    /// Canvas width in pixels (unpadded).
    pub fn canvas_width(&self) -> u32 {
        self.canvas_width
    }

    /// Canvas height in pixels (unpadded).
    pub fn canvas_height(&self) -> u32 {
        self.canvas_height
    }

    /// Unified frame scheduler. Called once per rAF tick.
    ///
    /// Systems fire at fractional rates of the master clock (rAF rate):
    /// - Veils: every `veil_divisor`-th frame (default 2 = 50% = 30fps at 60hz)
    /// - Overlay: every `overlay_divisor`-th frame (default 4 = 25% = 15fps at 60hz)
    ///
    /// Integer divisors guarantee alignment — a divisor-4 tick always coincides
    /// with a divisor-2 tick, so systems never force extra frame renders.
    pub fn update_animations(&mut self, queue: &wgpu::Queue, wall_time: f32) {
        let dt = if self.last_wall_time > 0.0 {
            (wall_time - self.last_wall_time).max(0.0)
        } else {
            0.0
        };
        self.last_wall_time = wall_time;
        self.frame_count += 1;

        if dt == 0.0 {
            return;
        }

        let veil_divisor = crate::config::get_i64("animation.veil_divisor") as u64;
        let overlay_divisor = crate::config::get_i64("animation.overlay_divisor") as u64;

        let veil_fires = veil_divisor > 0
            && self.veil_chain.needs_animation()
            && self.frame_count % veil_divisor == 0;

        let overlay_fires = overlay_divisor > 0
            && self.tool_overlay.needs_animation()
            && self.frame_count % overlay_divisor == 0;

        if veil_fires {
            self.veil_chain.update_veils(queue, dt * veil_divisor as f32);
        }

        if overlay_fires {
            self.tool_overlay.advance_time(dt * overlay_divisor as f32);
        }

        if veil_fires || overlay_fires {
            self.needs_present = true;
        }
    }

    /// Returns true if any animations need continuous frames (veils or overlay).
    pub fn needs_animation(&self) -> bool {
        self.tool_overlay.needs_animation()
            || self.veil_chain.needs_animation()
    }

    /// Update the view transform uniform buffer.
    pub fn update_view_transform(&mut self, queue: &wgpu::Queue, transform: &ViewTransform) {
        queue.write_buffer(&self.view_uniform_buf, 0, bytemuck::bytes_of(transform));
        self.cached_view_transform = *transform;
    }

    /// Update a raster layer's uniforms (called when opacity, blend mode, or show_mask changes).
    pub fn update_raster_uniforms(
        &mut self,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        opacity: f32,
        blend_mode: BlendMode,
    ) {
        self.update_raster_uniforms_full(queue, layer_id, opacity, blend_mode, false);
    }

    /// Update a raster layer's uniforms including the show_mask flag.
    pub fn update_raster_uniforms_full(
        &mut self,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        opacity: f32,
        blend_mode: BlendMode,
        show_mask: bool,
    ) {
        if let Some(cache) = self.raster_cache.get(&layer_id) {
            let uniforms = BlendUniforms {
                opacity,
                blend_mode: blend_mode as u32,
                show_mask: show_mask as u32,
                _pad1: 0.0,
            };
            queue.write_buffer(&cache.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        }
    }

    /// Create or remove the mask GPU texture for a layer.
    /// Rebuilds the mask bind group to point to the real texture or the default fallback.
    pub fn set_layer_mask(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        has_mask: bool,
    ) {
        if has_mask {
            if !self.mask_textures.contains_key(&layer_id) {
                // new_mask() initializes the texture to white (255 = reveal all).
                let mask_tex = LayerTexture::new_mask(device, queue, self.canvas_width, self.canvas_height);
                let mask_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("mask-bg-{layer_id}")),
                    layout: &self.blend_pipelines.mask_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&mask_tex.view),
                    }],
                });
                self.mask_textures.insert(layer_id, mask_tex);
                self.mask_bind_groups.insert(layer_id, mask_bg);
                // Passthrough groups: create PassthroughMaskState if needed.
                if !self.passthrough_mask_state.contains_key(&layer_id) {
                    let (snapshot, snapshot_view) = Self::make_accum_texture(
                        device, self.padded_width, self.padded_height,
                        &format!("pt-snapshot-{layer_id}"),
                    );
                    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(&format!("pt-lerp-uniforms-{layer_id}")),
                        size: 4,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    self.passthrough_mask_state.insert(layer_id, PassthroughMaskState {
                        snapshot,
                        snapshot_view,
                        uniform_buf,
                    });
                }
            }
        } else {
            self.mask_textures.remove(&layer_id);
            self.mask_bind_groups.remove(&layer_id);
            self.passthrough_mask_state.remove(&layer_id);
        }
    }

    /// Update the mask bind group to use real or default texture based on mask_enabled/show_mask.
    /// GIMP optimization: dormant masks (exists but disabled and not shown) use the default.
    pub fn update_mask_binding(
        &mut self,
        device: &wgpu::Device,
        layer_id: LayerId,
        mask_enabled: bool,
        show_mask: bool,
    ) {
        let use_real = (mask_enabled || show_mask) && self.mask_textures.contains_key(&layer_id);
        if use_real {
            let view = &self.mask_textures[&layer_id].view;
            let mask_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("mask-bg-{layer_id}")),
                layout: &self.blend_pipelines.mask_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                }],
            });
            self.mask_bind_groups.insert(layer_id, mask_bg);
        } else {
            self.mask_bind_groups.remove(&layer_id);
        }
    }

    /// Look up the resolved mask bind group for an entity, falling back to
    /// the default (1x1 white = no masking) when no real mask is active.
    fn mask_bind_group(&self, layer_id: LayerId) -> &wgpu::BindGroup {
        self.mask_bind_groups.get(&layer_id).unwrap_or(&self.default_mask_bind_group)
    }

    /// Get the composited output texture (root group's composite cache).
    /// Used by the color picker for readback.
    pub fn composited_texture(&self) -> &wgpu::Texture {
        &self.group_state[&ROOT_ID].composite_cache
    }

    pub fn accum_format(&self) -> wgpu::TextureFormat {
        wgpu::TextureFormat::Rgba8Unorm
    }

    pub fn veil_chain(&self) -> &VeilChain {
        &self.veil_chain
    }

    pub fn veil_chain_mut(&mut self) -> &mut VeilChain {
        &mut self.veil_chain
    }

    // --- Tool Overlay ---

    /// Replace the current overlay primitives.
    pub fn set_overlay_primitives(&mut self, prims: Vec<OverlayPrimitive>) {
        self.tool_overlay.set_primitives(prims);
        self.needs_present = true;
    }

    /// Clear all overlay primitives.
    pub fn clear_overlay(&mut self) {
        self.tool_overlay.clear_primitives();
        self.needs_present = true;
    }

    /// Advance overlay animation time.
    pub fn update_overlay_time(&mut self, dt: f32) {
        self.tool_overlay.advance_time(dt);
    }

    /// Upload a mask texture for KIND_MASKED_STAMP overlay primitives.
    pub fn set_overlay_mask(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) {
        self.tool_overlay.set_mask_texture(device, queue, width, height, rgba);
        self.needs_present = true;
    }

    /// Drop the uploaded mask (fall back to 1×1 white).
    pub fn clear_overlay_mask(&mut self) {
        self.tool_overlay.clear_mask_texture();
        self.needs_present = true;
    }

    /// Ensure the overlay's preview-mask texture exists at the given size;
    /// returns a view for a brush node to render into.
    pub fn ensure_overlay_preview_mask(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> &wgpu::TextureView {
        self.tool_overlay.ensure_preview_mask(device, width, height)
    }

    /// Route the preview-mask texture as the active overlay mask binding.
    pub fn use_overlay_preview_mask(&mut self) {
        self.tool_overlay.use_preview_mask_as_mask();
        self.needs_present = true;
    }

    /// Unbind the preview mask (falls back to 1×1 white default).
    pub fn clear_overlay_preview_mask(&mut self) {
        self.tool_overlay.clear_preview_mask();
        self.needs_present = true;
    }

    /// Borrow the overlay's preview-mask Texture (None if never allocated).
    pub fn overlay_preview_mask_texture(&self) -> Option<&wgpu::Texture> {
        self.tool_overlay.preview_mask_texture()
    }

    /// Immutable access to the tool overlay for test assertions.
    pub fn tool_overlay_ref(&self) -> &ToolOverlay {
        &self.tool_overlay
    }

    /// CPU-side hit test on overlay primitives.
    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize> {
        self.tool_overlay.hit_test(screen_x, screen_y)
    }

    // --- Floating Content (Transform) ---

    /// Set up floating content for GPU preview. Uploads flat RGBA pixel data
    /// as a texture and creates bind groups for compositing.
    pub fn set_floating_content(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba_data: &[u8],
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        target_layer: LayerId,
        target_is_mask: bool,
    ) {
        let root = self.group_state.get(&ROOT_ID).expect("root GroupState missing");
        self.transform_pass.set_floating_content(
            device,
            queue,
            &self.sampler,
            &root.accum.views,
            &root.composite_cache_view,
            rgba_data,
            source_origin,
            source_width,
            source_height,
            self.padded_width,
            self.padded_height,
            target_layer,
            target_is_mask,
        );
        self.mark_dirty();
    }

    /// Set floating content by copying directly from a layer's GPU texture.
    /// GPU→GPU copy — no CPU tiles involved. Looks up the texture from
    /// `layer_textures` / `mask_textures` by `target_layer` and `target_is_mask`.
    pub fn set_floating_content_from_gpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        target_layer: LayerId,
        target_is_mask: bool,
    ) {
        let layer_texture = if target_is_mask {
            match self.mask_textures.get(&target_layer) {
                Some(t) => &t.texture,
                None => return,
            }
        } else {
            match self.layer_textures.get(&target_layer) {
                Some(t) => &t.texture,
                None => return,
            }
        };
        let root = self.group_state.get(&ROOT_ID).expect("root GroupState missing");
        self.transform_pass.set_floating_content_from_gpu(
            device,
            queue,
            encoder,
            &self.sampler,
            &root.accum.views,
            &root.composite_cache_view,
            layer_texture,
            source_origin,
            source_width,
            source_height,
            self.padded_width,
            self.padded_height,
            target_layer,
            target_is_mask,
        );
        self.mark_dirty();
    }

    /// Update the floating content's affine transform matrix for real-time preview.
    pub fn update_floating_matrix(
        &mut self,
        queue: &wgpu::Queue,
        matrix: &crate::gpu::transform::Affine2D,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
    ) {
        self.transform_pass.update_matrix(
            queue,
            matrix,
            source_origin,
            source_width,
            source_height,
            self.padded_width,
            self.padded_height,
        );
        self.mark_dirty();
    }

    /// Render the transform directly onto the target layer/mask texture.
    /// Used by commit_floating() to replace CPU-side rasterize_to_tiles().
    pub fn commit_floating_to_texture(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        matrix: &crate::gpu::transform::Affine2D,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
    ) {
        let (layer_id, is_mask) = match &self.transform_pass.active {
            Some(s) => (s.target_layer, s.target_is_mask),
            None => return,
        };

        let (texture, view, format) = if is_mask {
            match self.mask_textures.get(&layer_id) {
                Some(t) => (&t.texture, &t.view, wgpu::TextureFormat::R8Unorm),
                None => return,
            }
        } else {
            match self.layer_textures.get(&layer_id) {
                Some(t) => (&t.texture, &t.view, wgpu::TextureFormat::Rgba8Unorm),
                None => return,
            }
        };

        self.transform_pass.commit_to_texture(
            device, encoder, queue, texture, view, format,
            matrix, source_origin, source_width, source_height,
            self.padded_width, self.padded_height,
        );
    }

    /// Remove floating content GPU state.
    pub fn clear_floating_content(&mut self) {
        self.transform_pass.clear();
        self.mark_dirty();
    }

    /// Get a reference to the transform source texture and its view.
    /// Returns None if no floating content is active.
    pub fn transform_source_texture(&self) -> Option<(&wgpu::Texture, &wgpu::TextureView)> {
        self.transform_pass.active.as_ref().map(|s| (&s.source_texture, &s.source_view))
    }

    /// Check if floating content is active.
    pub fn has_floating_content(&self) -> bool {
        self.transform_pass.active.is_some()
    }

    /// Run the present pass, veil chain, and final blit to surface.
    /// Solid overlay primitives are drawn at the end of the final render
    /// pass (present or veil-blit) to avoid a separate LoadOp::Load pass.
    fn present_and_veils(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
    ) {
        if !self.veil_chain.has_visible() {
            // No veils — present directly to surface.
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: surface_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(&self.present_pipeline);
            rpass.set_bind_group(0, &self.present_cache_bind_group, &[]);
            rpass.draw(0..3, 0..1);
            // Draw solid overlay primitives in the same pass.
            self.tool_overlay.draw_solid(&mut rpass);
            return;
        }

        self.veil_chain.encode(
            encoder,
            surface_view,
            &self.present_to_veil_pipeline,
            &self.present_cache_bind_group,
            &self.tool_overlay,
        );
    }

    /// Composite layer tree to offscreen target. GPU textures are authoritative —
    /// no CPU tile upload needed. Returns true if GPU work was submitted.
    pub fn render_offscreen(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &mut Document,
    ) -> bool {
        if !self.needs_composite {
            return false;
        }

        let scissor = (0, 0, self.canvas_width, self.canvas_height);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("composite"),
        });

        self.compose_group(&mut encoder, device, ROOT_ID, &doc.root.children, scissor);

        queue.submit(std::iter::once(encoder.finish()));

        self.needs_composite = false;
        true
    }

    /// Create a dynamic blend bind group for compositing a layer into a group.
    fn create_blend_bind_group(
        &self,
        device: &wgpu::Device,
        bg_view: &wgpu::TextureView,
        layer_view: &wgpu::TextureView,
        uniform_buf: &wgpu::Buffer,
        label: &str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.blend_pipelines.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(bg_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(layer_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform_buf.as_entire_binding(),
                },
            ],
        })
    }

    /// Recursively composite a group's children into its GroupState.
    ///
    /// For passthrough groups, children are inlined into the parent's accum
    /// (same as the old flat loop). For normal groups, children composite
    /// into the group's own accum pair, then the result is blended into the
    /// parent.
    fn compose_group(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        group_id: LayerId,
        children: &[LayerNode],
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;

        // Reset group's accum state for a fresh composite.
        {
            let gs = self.group_state.get_mut(&group_id).expect("GroupState missing");
            gs.current_accum = 0;
            gs.cache_valid_through = None;
            let _rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear-accum"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &gs.accum.views[0],
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

        // Inline children into this group's accumulators.
        self.compose_children(encoder, device, group_id, children, scissor);

        // Copy final accum to this group's composite cache.
        let gs = self.group_state.get(&group_id).expect("GroupState missing");
        let src_accum = gs.current_accum;
        let origin = wgpu::Origin3d { x: scissor_x, y: scissor_y, z: 0 };
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &gs.accum.textures[src_accum],
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &gs.composite_cache,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: scissor_w,
                height: scissor_h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Composite a list of children into the parent group's accumulators.
    /// Handles passthrough groups by recursing with the same parent group_id.
    fn compose_children(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        parent_group: LayerId,
        children: &[LayerNode],
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;

        for node in children {
            if !node.visible() {
                continue;
            }
            match node {
                LayerNode::Layer(Layer::Raster(raster)) => {
                    let layer_view = match self.layer_textures.get(&raster.id) {
                        Some(t) => &t.view,
                        None => continue,
                    };
                    let uniform_buf_ptr = match self.raster_cache.get(&raster.id) {
                        Some(c) => &c.uniform_buf,
                        None => continue,
                    };

                    // Ping-pong: read from current accum, write to the other.
                    let gs = self.group_state.get_mut(&parent_group).unwrap();
                    let src = gs.current_accum;
                    let dst = 1 - src;
                    gs.current_accum = dst;

                    let bind_group = self.create_blend_bind_group(
                        device,
                        &self.group_state[&parent_group].accum.views[src],
                        layer_view,
                        uniform_buf_ptr,
                        "blend-raster",
                    );

                    {
                        let gs = &self.group_state[&parent_group];
                        let mask_bg = self.mask_bind_group(raster.id);
                        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("blend-raster"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &gs.accum.views[dst],
                                resolve_target: None,
                                depth_slice: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            ..Default::default()
                        });
                        rpass.set_scissor_rect(scissor_x, scissor_y, scissor_w, scissor_h);
                        rpass.set_pipeline(self.blend_pipelines.pipeline());
                        rpass.set_bind_group(0, &bind_group, &[]);
                        rpass.set_bind_group(1, mask_bg, &[]);
                        rpass.draw(0..3, 0..1);
                    }

                    // Floating content pass: composite transformed source on
                    // top of the layer we just blended.
                    if let Some(ts) = &self.transform_pass.active {
                        if ts.target_layer == raster.id {
                            let gs = self.group_state.get_mut(&parent_group).unwrap();
                            let src = gs.current_accum;
                            let dst = 1 - src;
                            gs.current_accum = dst;

                            let gs = &self.group_state[&parent_group];
                            let mut rpass = encoder.begin_render_pass(
                                &wgpu::RenderPassDescriptor {
                                    label: Some("transform-blend"),
                                    color_attachments: &[Some(
                                        wgpu::RenderPassColorAttachment {
                                            view: &gs.accum.views[dst],
                                            resolve_target: None,
                                            depth_slice: None,
                                            ops: wgpu::Operations {
                                                load: wgpu::LoadOp::Load,
                                                store: wgpu::StoreOp::Store,
                                            },
                                        },
                                    )],
                                    ..Default::default()
                                },
                            );
                            rpass.set_scissor_rect(
                                scissor_x, scissor_y, scissor_w, scissor_h,
                            );
                            rpass.set_pipeline(&self.transform_pass.pipeline);
                            rpass.set_bind_group(0, &ts.bind_groups[src], &[]);
                            rpass.draw(0..3, 0..1);
                        }
                    }
                }

                LayerNode::Group(g) => {
                    if g.passthrough {
                        let has_active_mask = g.has_mask
                            && g.mask_enabled
                            && self.mask_textures.contains_key(&g.id);

                        if has_active_mask || g.show_mask {
                            // Photoshop-style passthrough + mask:
                            // 1. Snapshot parent accum before children.
                            // 2. Composite children (passthrough into parent).
                            // 3. Lerp between snapshot and result using mask.
                            self.compose_passthrough_masked(
                                encoder, device, parent_group, g, scissor,
                            );
                        } else {
                            // Pure passthrough — inline children into parent.
                            self.compose_children(
                                encoder, device, parent_group, &g.children, scissor,
                            );
                        }
                    } else {
                        // Normal group: composite into its own isolated buffer,
                        // then blend the result into the parent.
                        // GroupState must be pre-created via ensure_group_state().
                        if !self.group_state.contains_key(&g.id) {
                            continue;
                        }
                        self.compose_group(encoder, device, g.id, &g.children, scissor);

                        // Blend group's composite cache into parent's accumulators.
                        let gs_parent = self.group_state.get_mut(&parent_group).unwrap();
                        let src = gs_parent.current_accum;
                        let dst = 1 - src;
                        gs_parent.current_accum = dst;

                        let gs_child = &self.group_state[&g.id];
                        let bind_group = self.create_blend_bind_group(
                            device,
                            &self.group_state[&parent_group].accum.views[src],
                            &gs_child.composite_cache_view,
                            &gs_child.uniform_buf,
                            "blend-group",
                        );

                        let gs_parent = &self.group_state[&parent_group];
                        let child_mask_bg = self.mask_bind_group(g.id);
                        {
                            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("blend-group"),
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: &gs_parent.accum.views[dst],
                                    resolve_target: None,
                                    depth_slice: None,
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Load,
                                        store: wgpu::StoreOp::Store,
                                    },
                                })],
                                ..Default::default()
                            });
                            rpass.set_scissor_rect(scissor_x, scissor_y, scissor_w, scissor_h);
                            rpass.set_pipeline(self.blend_pipelines.pipeline());
                            rpass.set_bind_group(0, &bind_group, &[]);
                            rpass.set_bind_group(1, child_mask_bg, &[]);
                            rpass.draw(0..3, 0..1);
                        }
                    }
                }
            }
        }
    }

    /// Composite a passthrough group whose mask is active.
    ///
    /// Snapshots the parent accumulator, composites children (passthrough),
    /// then lerps between the snapshot and the result using the group mask.
    fn compose_passthrough_masked(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        parent_group: LayerId,
        group: &crate::layer::LayerGroup,
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;
        let group_id = group.id;

        // PassthroughMaskState must exist (created when the mask was added).
        if !self.passthrough_mask_state.contains_key(&group_id) {
            // Fallback: just inline children without mask.
            self.compose_children(encoder, device, parent_group, &group.children, scissor);
            return;
        }

        // 1. Copy current parent accum (the "before" state) into the snapshot.
        let gs = self.group_state.get(&parent_group).expect("parent GroupState missing");
        let before_idx = gs.current_accum;
        let origin = wgpu::Origin3d { x: scissor_x, y: scissor_y, z: 0 };
        let copy_size = wgpu::Extent3d {
            width: scissor_w,
            height: scissor_h,
            depth_or_array_layers: 1,
        };
        let pms = &self.passthrough_mask_state[&group_id];
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.group_state[&parent_group].accum.textures[before_idx],
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &pms.snapshot,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            copy_size,
        );

        // 2. Composite children into parent accumulators (passthrough).
        self.compose_children(encoder, device, parent_group, &group.children, scissor);

        // 3. Lerp pass: mix(snapshot, current_accum, mask).
        //    Write the lerp result into the ping-pong "other" accumulator.
        let gs = self.group_state.get_mut(&parent_group).unwrap();
        let after_idx = gs.current_accum;
        let dst = 1 - after_idx;
        gs.current_accum = dst;

        let pms = &self.passthrough_mask_state[&group_id];

        // Create lerp bind group (group 0): before=snapshot, after=current_accum, sampler, uniforms.
        let lerp_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mask-lerp-bg"),
            layout: &self.blend_pipelines.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&pms.snapshot_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &self.group_state[&parent_group].accum.views[after_idx],
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: pms.uniform_buf.as_entire_binding(),
                },
            ],
        });

        {
            let gs = &self.group_state[&parent_group];
            let group_mask_bg = self.mask_bind_group(group_id);
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mask-lerp"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &gs.accum.views[dst],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_scissor_rect(scissor_x, scissor_y, scissor_w, scissor_h);
            rpass.set_pipeline(&self.mask_lerp_pipeline);
            rpass.set_bind_group(0, &lerp_bind_group, &[]);
            rpass.set_bind_group(1, group_mask_bg, &[]);
            rpass.draw(0..3, 0..1);
        }
    }

    /// Whether any rendering work is pending (composite, present, veils).
    fn has_pending_work(&self, _doc: &Document) -> bool {
        self.needs_composite
            || self.needs_present
            || self.veil_chain.needs_present()
    }

    /// Clear present-related dirty flags after a frame.
    fn finish_present(&mut self) {
        self.needs_present = false;
        self.veil_chain.clear_needs_present();
    }

    /// Composite layers if needed, then present to an arbitrary texture view.
    ///
    /// This is the backend-agnostic rendering entry point. Any frontend
    /// (WASM surface, native window, CEF hole-punch, headless test) can
    /// provide a `TextureView` and get the composited + veiled result.
    pub fn render_to_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_view: &wgpu::TextureView,
        doc: &mut Document,
    ) {
        if !self.has_pending_work(doc) {
            return;
        }

        self.render_offscreen(device, queue, doc);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("present-to-view"),
        });
        self.present_and_veils(&mut encoder, target_view);
        queue.submit(std::iter::once(encoder.finish()));

        self.finish_present();
    }

    /// Upload dirty tiles, composite changed layers, present to a surface.
    ///
    /// Convenience wrapper around `render_to_view` that handles surface
    /// acquisition and presentation. Used by the WASM frontend.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: &wgpu::Surface,
        surface_config: &wgpu::SurfaceConfiguration,
        doc: &mut Document,
    ) {
        perf::time("render-total");

        if !self.has_pending_work(doc) {
            perf::time_end("render-total");
            return;
        }

        perf::time("offscreen");
        self.render_offscreen(device, queue, doc);
        perf::time_end("offscreen");

        // Acquire surface and present composite_cache → veils → surface.
        let output = match surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost) => {
                surface.configure(device, surface_config);
                perf::time_end("render-total");
                return;
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                log::error!("Out of GPU memory");
                perf::time_end("render-total");
                return;
            }
            Err(e) => {
                log::warn!("Surface error: {e:?}");
                perf::time_end("render-total");
                return;
            }
        };
        let surface_view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        perf::time("present");
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("present"),
        });

        // Prepare overlay CPU-side work (upload, bind group) before render passes.
        if self.tool_overlay.has_content() {
            let vt = self.cached_view_transform;
            let vw = self.veil_chain.viewport_size().0;
            let vh = self.veil_chain.viewport_size().1;
            self.tool_overlay.prepare(device, queue, &vt, vw, vh);
        }

        // Present + veils. Solid overlay primitives are drawn at the end
        // of the final pass (no separate LoadOp::Load pass needed).
        self.present_and_veils(&mut encoder, &surface_view);

        // Snapshot-sampling overlay primitives (invert + soft-contrast) need a
        // separate pass with a surface→snapshot copy. Hit by rect-select and
        // the brush-stamp preview.
        if self.tool_overlay.has_snapshot() {
            let vw = self.veil_chain.viewport_size().0;
            let vh = self.veil_chain.viewport_size().1;
            self.tool_overlay.encode_snapshot(
                &mut encoder, &output.texture, &surface_view, vw, vh,
            );
        }

        queue.submit(std::iter::once(encoder.finish()));
        output.present();
        perf::time_end("present");

        self.finish_present();
        perf::time_end("render-total");
    }
}
