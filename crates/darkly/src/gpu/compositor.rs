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
use crate::tile::TileData;
use std::sync::LazyLock;

/// Blank (fully transparent) tile data uploaded when a tile has been removed
/// from the grid (e.g. by undo) but the GPU texture still has stale data.
static BLANK_TILE: LazyLock<TileData> = LazyLock::new(TileData::default);
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
    /// Uniform buffer holding opacity + blend_mode.
    uniform_buf: wgpu::Buffer,
    /// Bind groups for both ping-pong directions.
    /// bind_groups[src_accum_index]
    bind_groups: [wgpu::BindGroup; 2],
    /// Bind group that reads from the composite cache as background.
    /// Used when resuming compositing from the cache (avoids cache→accum copy).
    cache_source_bind_group: wgpu::BindGroup,
}

/// Uniforms for raster layer compositing.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct BlendUniforms {
    opacity: f32,
    blend_mode: u32,
    _pad0: f32,
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

    veil_chain: VeilChain,

    // --- Tool Overlay ---
    tool_overlay: ToolOverlay,
    /// Cached view transform for overlay forward matrix computation.
    cached_view_transform: ViewTransform,
    /// Last wall-clock time for overlay animation dt computation.
    last_anim_time: f32,
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

        Compositor {
            accum: [accum0, accum1],
            accum_views: [accum_view0, accum_view1],
            current_accum: 0,
            composite_cache,
            composite_cache_view,
            cache_valid_through: None,
            layer_textures: HashMap::new(),
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
            veil_chain,
            tool_overlay,
            cached_view_transform: identity,
            last_anim_time: 0.0,
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
            _pad0: 0.0,
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

        self.raster_cache.insert(
            layer_id,
            RasterLayerCache {
                uniform_buf,
                bind_groups,
                cache_source_bind_group,
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

    /// Update all animations (veils + overlay). Called once per frame.
    pub fn update_animations(&mut self, queue: &wgpu::Queue, wall_time: f32) {
        // Compute dt for overlay before handing wall_time to the veil chain.
        let dt = if self.last_anim_time > 0.0 {
            (wall_time - self.last_anim_time).max(0.0)
        } else {
            0.0
        };
        self.last_anim_time = wall_time;

        // Update overlay animation time only when dashed lines are animating.
        // update_time returns true at ~10fps to avoid excessive GPU work.
        if dt > 0.0 && self.tool_overlay.needs_animation() {
            if self.tool_overlay.update_time(dt) {
                self.needs_present = true;
            }
        }

        // Update veil chain animations.
        self.veil_chain.update_time(queue, wall_time);
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

    /// Update a raster layer's uniforms (called when opacity or blend mode changes).
    pub fn update_raster_uniforms(
        &mut self,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        opacity: f32,
        blend_mode: BlendMode,
    ) {
        if let Some(cache) = self.raster_cache.get(&layer_id) {
            let uniforms = BlendUniforms {
                opacity,
                blend_mode: blend_mode as u32,
                _pad0: 0.0,
                _pad1: 0.0,
            };
            queue.write_buffer(&cache.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
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
        self.tool_overlay.update_time(dt);
    }

    /// CPU-side hit test on overlay primitives.
    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize> {
        self.tool_overlay.hit_test(screen_x, screen_y)
    }

    /// Run the present pass, veil chain, and final blit to surface.
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
            return;
        }

        self.veil_chain.encode(
            encoder,
            surface_view,
            &self.present_to_veil_pipeline,
            &self.present_cache_bind_group,
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

        if !self.needs_composite && !has_dirty {
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

        if let Some(lowest) = self.lowest_dirty_layer.take() {
            self.invalidate_cache_from(lowest);
        }

        if !self.needs_composite {
            for dirty in doc.dirty.values_mut() {
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

                    let (dst_view, bind_group) = if use_cache_source {
                        use_cache_source = false;
                        let dst = 0;
                        self.current_accum = dst;
                        (&self.accum_views[dst], &cache.cache_source_bind_group)
                    } else if is_last_layer {
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
                        rpass.draw(0..3, 0..1);
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
        let veil_needs = self.veil_chain.needs_present();
        if !self.needs_composite && !has_dirty && !self.needs_present && !veil_needs {
            return;
        }

        if self.needs_composite || has_dirty {
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
        let veil_needs = self.veil_chain.needs_present();
        if !self.needs_composite && !has_dirty && !self.needs_present && !veil_needs {
            perf::time_end("render-total");
            return;
        }

        // Composite layers into composite_cache if needed.
        if self.needs_composite || has_dirty {
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
        self.present_and_veils(&mut encoder, &surface_view);

        // Tool overlay (on top of surface, after veils)
        if self.tool_overlay.has_content() {
            let vt = self.cached_view_transform;
            let vw = self.veil_chain.viewport_size().0;
            let vh = self.veil_chain.viewport_size().1;
            self.tool_overlay.encode(
                device, queue, &mut encoder, &output.texture, &surface_view, &vt, vw, vh,
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
