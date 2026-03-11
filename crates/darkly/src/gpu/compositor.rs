use crate::gpu::atlas::LayerTexture;
use crate::gpu::blend::BlendPipelines;
use crate::gpu::effect::EffectCache;
use crate::gpu::filter::FilterRegistry;
use crate::gpu::overlay::{OverlayPrimitive, ToolOverlay};
use crate::gpu::staging::StagingRing;
use crate::gpu::veil_chain::VeilChain;
use crate::gpu::view::ViewTransform;
use crate::dirty::dirty_pixel_rect;
use crate::document::Document;
use crate::tile::{TileData, TILE_SIZE as TILE_SIZE_USIZE};
use std::sync::LazyLock;

/// Blank (fully transparent) tile data uploaded when a tile has been removed
/// from the grid (e.g. by undo) but the GPU texture still has stale data.
static BLANK_TILE: LazyLock<TileData> = LazyLock::new(TileData::default);

/// Fully opaque (255) mask tile data for uploading full mask tiles.
static FULL_MASK_TILE: LazyLock<[u8; TILE_SIZE_USIZE * TILE_SIZE_USIZE]> =
    LazyLock::new(|| [255u8; TILE_SIZE_USIZE * TILE_SIZE_USIZE]);

use crate::layer::{BlendMode, Layer, LayerId};
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

/// Pre-built GPU objects for a raster layer (P1 — created once, never per-frame).
struct RasterLayerCache {
    /// Uniform buffer holding opacity + blend_mode + show_mask.
    uniform_buf: wgpu::Buffer,
    /// Bind groups for both ping-pong directions (group 0).
    /// bind_groups[src_accum_index]
    bind_groups: [wgpu::BindGroup; 2],
    /// Bind group that reads from the composite cache as background (group 0).
    /// Used when resuming compositing from the cache (avoids cache→accum copy).
    cache_source_bind_group: wgpu::BindGroup,
    /// Mask bind group (group 1). Points to real mask texture or 1x1 white fallback.
    mask_bind_group: wgpu::BindGroup,
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

pub struct Compositor {
    /// Two accumulator textures for ping-pong rendering.
    accum: [wgpu::Texture; 2],
    accum_views: [wgpu::TextureView; 2],
    current_accum: usize,

    /// Cached composite result (GPU-resident). Stores the final composited
    /// image so we can re-composite from a dirty layer upward (P3).
    composite_cache: wgpu::Texture,
    composite_cache_view: wgpu::TextureView,
    /// Index of the lowest layer that the cache is valid through.
    /// None = cache is empty, must composite from scratch.
    cache_valid_through: Option<usize>,

    /// Per-layer GPU textures (one per raster layer).
    layer_textures: HashMap<LayerId, LayerTexture>,

    /// Per-layer mask GPU textures (R8Unorm, one per layer with a mask).
    mask_textures: HashMap<LayerId, LayerTexture>,
    /// Default 1x1 white mask texture (mask_alpha=1.0 = no effect).
    default_mask_view: wgpu::TextureView,
    /// Default mask bind group using the 1x1 white texture.
    default_mask_bind_group: wgpu::BindGroup,

    /// Pre-built GPU objects per raster layer (P1).
    raster_cache: HashMap<LayerId, RasterLayerCache>,
    /// Pre-built GPU objects per filter layer (P1).
    filter_cache: HashMap<LayerId, EffectCache>,

    blend_pipelines: BlendPipelines,
    filter_registry: FilterRegistry,

    present_pipeline: wgpu::RenderPipeline,
    /// Present pipeline targeting the accum format (Rgba8Unorm) for veil input.
    present_to_veil_pipeline: wgpu::RenderPipeline,
    _present_bind_group_layout: wgpu::BindGroupLayout,
    /// Present bind group that reads from composite_cache directly.
    present_cache_bind_group: wgpu::BindGroup,
    /// View transform uniform buffer for the present shader.
    view_uniform_buf: wgpu::Buffer,

    staging: StagingRing,
    sampler: wgpu::Sampler,

    /// Dirty gate — false means nothing changed, skip compositing (P2).
    needs_composite: bool,
    /// When only the view transform changes, skip compositing and only re-present.
    needs_present: bool,
    /// Track lowest dirty layer index for cache invalidation.
    lowest_dirty_layer: Option<usize>,

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

    // --- Frame Scheduler ---
    /// Monotonic frame counter, incremented on each rAF tick.
    /// Systems fire when `frame_count % divisor == 0`.
    frame_count: u64,
    /// Last wall-clock time for dt computation.
    last_wall_time: f32,
}

impl Compositor {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        use crate::tile::TILE_SIZE;

        // Pad accumulator dimensions to tile boundaries so they match layer
        // textures exactly. The composite shader samples both with the same
        // UVs, so any size mismatch causes painting offset / wrapping.
        let ts = TILE_SIZE as u32;
        let padded_w = ((width + ts - 1) / ts) * ts;
        let padded_h = ((height + ts - 1) / ts) * ts;

        // Use Rgba8Unorm for accumulators (linear color space for blending)
        let accum_format = wgpu::TextureFormat::Rgba8Unorm;

        let make_accum = |label: &str| -> (wgpu::Texture, wgpu::TextureView) {
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
                format: accum_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (tex, view)
        };

        let (accum0, accum_view0) = make_accum("accum-0");
        let (accum1, accum_view1) = make_accum("accum-1");

        let (composite_cache, composite_cache_view) = make_accum("composite-cache");

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

        let filter_registry = FilterRegistry::new();

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

        // Present bind group that reads from composite cache
        let present_cache_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present-bg-cache"),
            layout: &_present_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&composite_cache_view),
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

        let staging = StagingRing::new();

        let veil_chain = VeilChain::new(
            device,
            sampler.clone(),
            surface_format,
            accum_format,
        );

        let tool_overlay = ToolOverlay::new(device, surface_format);

        let transform_pass = crate::gpu::transform::TransformPass::new(device, accum_format);

        Compositor {
            accum: [accum0, accum1],
            accum_views: [accum_view0, accum_view1],
            current_accum: 0,
            composite_cache,
            composite_cache_view,
            cache_valid_through: None,
            layer_textures: HashMap::new(),
            mask_textures: HashMap::new(),
            default_mask_view,
            default_mask_bind_group,
            raster_cache: HashMap::new(),
            filter_cache: HashMap::new(),
            blend_pipelines,
            filter_registry,
            present_pipeline,
            present_to_veil_pipeline,
            _present_bind_group_layout,
            present_cache_bind_group,
            view_uniform_buf,
            staging,
            sampler,
            needs_composite: true,
            needs_present: false,
            lowest_dirty_layer: None,
            canvas_width: width,
            canvas_height: height,
            padded_width: padded_w,
            padded_height: padded_h,
            veil_chain,
            transform_pass,
            tool_overlay,
            cached_view_transform: identity,
            frame_count: 0,
            last_wall_time: 0.0,
        }
    }

    /// Create GPU texture + uniform buffer + bind groups for a new raster layer.
    /// Called once when a layer is added, never in the render loop (P1).
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

        // Write initial uniforms
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        // Create bind groups for both ping-pong directions
        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("blend-bg-{layer_id}-{i}")),
                layout: &self.blend_pipelines.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&self.accum_views[i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&layer_tex.view),
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
        });

        // Bind group that reads from the composite cache as background source.
        // Used when this is the first layer after a cache resume, avoiding
        // a fullscreen cache→accum texture copy.
        let cache_source_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("blend-bg-{layer_id}-cache")),
            layout: &self.blend_pipelines.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.composite_cache_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&layer_tex.view),
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
        });

        // Default mask bind group (1x1 white — no masking effect)
        let mask_bind_group = self.default_mask_bind_group.clone();

        self.raster_cache.insert(
            layer_id,
            RasterLayerCache {
                uniform_buf,
                bind_groups,
                cache_source_bind_group,
                mask_bind_group,
            },
        );
        self.layer_textures.insert(layer_id, layer_tex);
    }

    /// Create GPU objects for a new filter layer.
    pub fn ensure_filter_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        filter: &dyn crate::gpu::filter::Filter,
    ) {
        if self.filter_cache.contains_key(&layer_id) {
            return;
        }

        let cache = filter.create_cache(
            device,
            queue,
            &self.accum_views,
            &self.sampler,
            self.canvas_width,
            self.canvas_height,
        );

        self.filter_cache.insert(layer_id, cache);
    }

    /// Mark that recompositing is needed.
    pub fn mark_dirty(&mut self) {
        self.needs_composite = true;
        self.cache_valid_through = None;
    }

    /// Mark that only the present pass needs to re-run (view transform changed).
    /// Skips compositing when there are no dirty tiles or layer changes.
    pub fn mark_needs_present(&mut self) {
        self.needs_present = true;
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

    /// Invalidate the composite cache.
    /// There is only one cache texture which stores the full composite of all
    /// layers, so any dirty layer means the entire cache is stale.
    fn invalidate_cache_from(&mut self, _layer_index: usize) {
        self.cache_valid_through = None;
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
                if let Some(cache) = self.raster_cache.get_mut(&layer_id) {
                    cache.mask_bind_group = mask_bg;
                }
            }
        } else {
            self.mask_textures.remove(&layer_id);
            if let Some(cache) = self.raster_cache.get_mut(&layer_id) {
                cache.mask_bind_group = self.default_mask_bind_group.clone();
            }
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
        let view = if use_real {
            &self.mask_textures[&layer_id].view
        } else {
            &self.default_mask_view
        };
        let mask_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("mask-bg-{layer_id}")),
            layout: &self.blend_pipelines.mask_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            }],
        });
        if let Some(cache) = self.raster_cache.get_mut(&layer_id) {
            cache.mask_bind_group = mask_bg;
        }
    }

    /// Access filter registry (immutable) for reading param defs.
    pub fn filter_registry(&self) -> &FilterRegistry {
        &self.filter_registry
    }

    /// Access filter registry for creating new filter instances.
    pub fn filter_registry_mut(&mut self) -> &mut FilterRegistry {
        &mut self.filter_registry
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

    /// CPU-side hit test on overlay primitives.
    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize> {
        self.tool_overlay.hit_test(screen_x, screen_y)
    }

    // --- Floating Content (Transform) ---

    /// Set up floating content for GPU preview. Uploads source tiles as a
    /// texture and creates bind groups for compositing.
    pub fn set_floating_content(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_tiles: &crate::tile::TileGrid,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        target_layer: LayerId,
        target_is_mask: bool,
    ) {
        self.transform_pass.set_floating_content(
            device,
            queue,
            &self.sampler,
            &self.accum_views,
            &self.composite_cache_view,
            source_tiles,
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

    /// Remove floating content GPU state.
    pub fn clear_floating_content(&mut self) {
        self.transform_pass.clear();
        self.mark_dirty();
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

    /// Upload dirty tiles and composite changed layers (no surface present).
    /// Returns true if GPU work was submitted, false if skipped (nothing dirty).
    pub fn render_offscreen(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &mut Document,
    ) -> bool {
        // 1. Check if any dirty regions exist before scanning layers.
        let has_dirty = doc.dirty.values().any(|d| !d.is_empty());
        let has_mask_dirty = doc.mask_dirty.values().any(|d| !d.is_empty());

        if !self.needs_composite && !has_dirty && !has_mask_dirty {
            return false;
        }

        // 2. Upload dirty tiles for each dirty raster layer
        if has_dirty {
            for raster in doc.all_raster_layers() {
                let dirty = match doc.dirty.get(&raster.id) {
                    Some(d) if !d.is_empty() => d,
                    _ => continue,
                };

                let layer_tex = match self.layer_textures.get(&raster.id) {
                    Some(t) => t,
                    None => continue,
                };

                for (tx, ty) in dirty.iter() {
                    if tx < 0 || ty < 0 {
                        continue;
                    }
                    if tx as u32 >= layer_tex.width_in_tiles
                        || ty as u32 >= layer_tex.height_in_tiles
                    {
                        continue;
                    }
                    let tile_data = match raster.tiles.get(tx, ty) {
                        Some(tile) => tile.data(),
                        None => &BLANK_TILE,
                    };
                    self.staging.upload_tile(
                        queue,
                        tile_data,
                        &layer_tex.texture,
                        tx as u32,
                        ty as u32,
                    );
                }

                if let Some(idx) = doc.flat_layer_index(raster.id) {
                    match self.lowest_dirty_layer {
                        Some(current) => {
                            if idx < current {
                                self.lowest_dirty_layer = Some(idx);
                            }
                        }
                        None => self.lowest_dirty_layer = Some(idx),
                    }
                }
                self.needs_composite = true;
            }
        }

        // 2b. Upload dirty mask tiles (f32→u8 conversion, R8Unorm format).
        // Only upload when mask_enabled || show_mask (GIMP's dormant mask optimization).
        if has_mask_dirty {
            for raster in doc.all_raster_layers() {
                let mask_active = raster.mask_enabled || raster.show_mask;
                if !mask_active {
                    continue;
                }

                let dirty = match doc.mask_dirty.get(&raster.id) {
                    Some(d) if !d.is_empty() => d,
                    _ => continue,
                };

                let mask_tex = match self.mask_textures.get(&raster.id) {
                    Some(t) => t,
                    None => continue,
                };

                let mask = match &raster.mask {
                    Some(m) => m,
                    None => continue,
                };

                let ts = TILE_SIZE_USIZE;
                for (tx, ty) in dirty.iter() {
                    if tx < 0 || ty < 0 {
                        continue;
                    }
                    if tx as u32 >= mask_tex.width_in_tiles
                        || ty as u32 >= mask_tex.height_in_tiles
                    {
                        continue;
                    }

                    // Convert f32 mask tile to u8 for R8Unorm upload
                    let u8_data: &[u8] = match mask.get(tx, ty) {
                        Some(tile) => {
                            // Convert f32 → u8 into a temporary buffer
                            // We use a thread-local buffer to avoid allocation per tile
                            thread_local! {
                                static BUF: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(
                                    vec![0u8; 64 * 64]
                                );
                            }
                            BUF.with(|buf| {
                                let mut buf = buf.borrow_mut();
                                let data = tile.data();
                                for i in 0..(ts * ts) {
                                    buf[i] = (data.0[i].clamp(0.0, 1.0) * 255.0) as u8;
                                }
                                // SAFETY: The buf lives in the thread-local and we immediately
                                // use it for the queue write below. The borrow is released
                                // after this closure returns.
                                unsafe {
                                    std::slice::from_raw_parts(buf.as_ptr(), ts * ts)
                                }
                            })
                        }
                        None => &*FULL_MASK_TILE,
                    };

                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &mask_tex.texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: tx as u32 * ts as u32,
                                y: ty as u32 * ts as u32,
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        u8_data,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(ts as u32),
                            rows_per_image: None,
                        },
                        wgpu::Extent3d {
                            width: ts as u32,
                            height: ts as u32,
                            depth_or_array_layers: 1,
                        },
                    );
                }

                if let Some(idx) = doc.flat_layer_index(raster.id) {
                    match self.lowest_dirty_layer {
                        Some(current) if idx < current => self.lowest_dirty_layer = Some(idx),
                        None => self.lowest_dirty_layer = Some(idx),
                        _ => {}
                    }
                }
                self.needs_composite = true;
            }
        }

        if let Some(lowest) = self.lowest_dirty_layer.take() {
            self.invalidate_cache_from(lowest);
        }

        if !self.needs_composite {
            for dirty in doc.dirty.values_mut() {
                dirty.clear();
            }
            for dirty in doc.mask_dirty.values_mut() {
                dirty.clear();
            }
            return false;
        }

        let dirty_rect = dirty_pixel_rect(
            doc.dirty.values(),
            self.canvas_width,
            self.canvas_height,
        );
        let (scissor_x, scissor_y, scissor_w, scissor_h) = dirty_rect
            .unwrap_or((0, 0, self.canvas_width, self.canvas_height));

        let start_layer = match self.cache_valid_through {
            Some(valid_through) => valid_through + 1,
            None => 0,
        };
        let resuming_from_cache = start_layer > 0;
        let mut use_cache_source = resuming_from_cache;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("composite"),
        });

        if !resuming_from_cache {
            {
                let _rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("clear-accum"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.accum_views[0],
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
            self.current_accum = 0;
        }

        let flat = doc.flat_layers();
        let num_layers = flat.len();
        let mut wrote_to_cache = false;

        for layer_idx in start_layer..num_layers {
            let layer = flat[layer_idx];
            if !layer.visible() {
                continue;
            }

            let is_last_layer = layer_idx == num_layers - 1;

            match layer {
                Layer::Raster(raster) => {
                    let cache = match self.raster_cache.get(&raster.id) {
                        Some(c) => c,
                        None => continue,
                    };

                    // Check if floating content targets this layer — if so,
                    // there's one more pass coming, so don't write to cache yet.
                    let has_floating = self.transform_pass.targets_layer(raster.id);
                    let is_raster_last = is_last_layer && !has_floating;

                    let (dst_view, bind_group) = if use_cache_source {
                        use_cache_source = false;
                        let dst = 0;
                        self.current_accum = dst;
                        (&self.accum_views[dst], &cache.cache_source_bind_group)
                    } else if is_raster_last {
                        wrote_to_cache = true;
                        let src = self.current_accum;
                        (&self.composite_cache_view, &cache.bind_groups[src])
                    } else {
                        let src = self.current_accum;
                        let dst = 1 - src;
                        self.current_accum = dst;
                        (&self.accum_views[dst], &cache.bind_groups[src])
                    };

                    {
                        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("blend-raster"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: dst_view,
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
                        rpass.set_bind_group(0, bind_group, &[]);
                        rpass.set_bind_group(1, &cache.mask_bind_group, &[]);
                        rpass.draw(0..3, 0..1);
                    }

                    // Floating content pass: composite transformed source on
                    // top of the layer we just blended.
                    if let Some(ts) = &self.transform_pass.active {
                        if ts.target_layer == raster.id {
                            let src = self.current_accum;
                            let fc_dst_view = if is_last_layer {
                                wrote_to_cache = true;
                                &self.composite_cache_view
                            } else {
                                let dst = 1 - src;
                                self.current_accum = dst;
                                &self.accum_views[dst]
                            };

                            {
                                let mut rpass = encoder.begin_render_pass(
                                    &wgpu::RenderPassDescriptor {
                                        label: Some("transform-blend"),
                                        color_attachments: &[Some(
                                            wgpu::RenderPassColorAttachment {
                                                view: fc_dst_view,
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
                }
                Layer::Filter(fl) => {
                    let cache = match self.filter_cache.get(&fl.id) {
                        Some(c) => c,
                        None => continue,
                    };

                    if use_cache_source {
                        use_cache_source = false;
                        let origin = wgpu::Origin3d {
                            x: scissor_x,
                            y: scissor_y,
                            z: 0,
                        };
                        encoder.copy_texture_to_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &self.composite_cache,
                                mip_level: 0,
                                origin,
                                aspect: wgpu::TextureAspect::All,
                            },
                            wgpu::TexelCopyTextureInfo {
                                texture: &self.accum[0],
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
                        self.current_accum = 0;
                    }

                    for pass in 0..fl.filter.pass_count() as usize {
                        let src = self.current_accum;
                        let dst = 1 - src;

                        let is_last_pass = pass == fl.filter.pass_count() as usize - 1;
                        let dst_view = if is_last_layer && is_last_pass {
                            wrote_to_cache = true;
                            &self.composite_cache_view
                        } else {
                            self.current_accum = dst;
                            &self.accum_views[dst]
                        };

                        {
                            let mut rpass =
                                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                    label: Some("filter-pass"),
                                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                        view: dst_view,
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
                            rpass.set_pipeline(fl.filter.pipeline());
                            rpass.set_bind_group(0, &cache.bind_groups[pass][src], &[]);
                            rpass.draw(0..3, 0..1);
                        }
                    }
                }
            }
        }

        if !wrote_to_cache && start_layer < num_layers {
            let origin = wgpu::Origin3d {
                x: scissor_x,
                y: scissor_y,
                z: 0,
            };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.accum[self.current_accum],
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.composite_cache,
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
        if start_layer < num_layers {
            self.cache_valid_through = Some(num_layers.saturating_sub(1));
        }

        queue.submit(std::iter::once(encoder.finish()));

        for dirty in doc.dirty.values_mut() {
            dirty.clear();
        }
        for dirty in doc.mask_dirty.values_mut() {
            dirty.clear();
        }
        self.needs_composite = false;
        true
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
        let has_dirty = doc.dirty.values().any(|d| !d.is_empty());
        let has_mask_dirty = doc.mask_dirty.values().any(|d| !d.is_empty());
        let veil_needs = self.veil_chain.needs_present();
        if !self.needs_composite && !has_dirty && !has_mask_dirty && !self.needs_present && !veil_needs {
            return;
        }

        if self.needs_composite || has_dirty || has_mask_dirty {
            self.render_offscreen(device, queue, doc);
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("present-to-view"),
        });
        self.present_and_veils(&mut encoder, target_view);
        queue.submit(std::iter::once(encoder.finish()));

        self.needs_present = false;
        self.veil_chain.clear_needs_present();
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

        let has_dirty = doc.dirty.values().any(|d| !d.is_empty());
        let has_mask_dirty = doc.mask_dirty.values().any(|d| !d.is_empty());
        let veil_needs = self.veil_chain.needs_present();
        if !self.needs_composite && !has_dirty && !has_mask_dirty && !self.needs_present && !veil_needs {
            perf::time_end("render-total");
            return;
        }

        // Composite layers into composite_cache if needed.
        if self.needs_composite || has_dirty || has_mask_dirty {
            perf::time("offscreen");
            self.render_offscreen(device, queue, doc);
            perf::time_end("offscreen");
        }

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

        // Invert overlay primitives (if any) need a separate pass with
        // snapshot copy. This path is only hit by rect_select.
        if self.tool_overlay.has_invert() {
            let vw = self.veil_chain.viewport_size().0;
            let vh = self.veil_chain.viewport_size().1;
            self.tool_overlay.encode_invert(
                &mut encoder, &output.texture, &surface_view, vw, vh,
            );
        }

        queue.submit(std::iter::once(encoder.finish()));
        output.present();
        perf::time_end("present");

        self.needs_present = false;
        self.veil_chain.clear_needs_present();
        perf::time_end("render-total");
    }
}
