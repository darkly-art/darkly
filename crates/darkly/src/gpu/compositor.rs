use crate::gpu::atlas::LayerTexture;
use crate::gpu::blend::BlendPipelines;
use crate::gpu::filter::{FilterLayerCache, FilterRegistry};
use crate::gpu::staging::StagingRing;
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
        web_sys::console::time_with_label(label);
    }
    pub fn time_end(label: &str) {
        web_sys::console::time_end_with_label(label);
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
    filter_cache: HashMap<LayerId, FilterLayerCache>,

    blend_pipelines: BlendPipelines,
    filter_registry: FilterRegistry,

    present_pipeline: wgpu::RenderPipeline,
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
                push_constant_ranges: &[],
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
            multiview: None,
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

    /// Update the view transform uniform buffer.
    pub fn update_view_transform(&self, queue: &wgpu::Queue, transform: &ViewTransform) {
        queue.write_buffer(&self.view_uniform_buf, 0, bytemuck::bytes_of(transform));
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

    /// Access filter registry for creating new filter instances.
    pub fn filter_registry_mut(&mut self) -> &mut FilterRegistry {
        &mut self.filter_registry
    }

    pub fn accum_format(&self) -> wgpu::TextureFormat {
        wgpu::TextureFormat::Rgba8Unorm
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
            for layer in &doc.layers {
                if let Layer::Raster(raster) = layer {
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

                    if let Some(idx) = doc.layer_index(raster.id) {
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

        let num_layers = doc.layers.len();
        let mut wrote_to_cache = false;

        for layer_idx in start_layer..num_layers {
            let layer = &doc.layers[layer_idx];
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

    /// Upload dirty tiles, composite changed layers, present.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: &wgpu::Surface,
        surface_config: &wgpu::SurfaceConfiguration,
        doc: &mut Document,
    ) {
        perf::time("render-total");

        // 1. Check if any dirty regions exist before scanning layers.
        let has_dirty = doc.dirty.values().any(|d| !d.is_empty());

        // P2: Dirty gate — if nothing changed and no dirty tiles, check view-only present.
        if !self.needs_composite && !has_dirty {
            if self.needs_present {
                // View transform changed but no compositing needed — re-present only.
                self.present_only(device, queue, surface, surface_config);
                self.needs_present = false;
            }
            perf::time_end("render-total");
            return;
        }

        // 2. Upload dirty tiles for each dirty raster layer
        perf::time("tile-upload");
        if has_dirty {
            for layer in &doc.layers {
                if let Layer::Raster(raster) = layer {
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
                            // Tile was removed (e.g. by undo) — upload blank to
                            // clear the stale GPU data.
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

                    // Note the lowest dirty layer for cache invalidation
                    if let Some(idx) = doc.layer_index(raster.id) {
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
        }

        perf::time_end("tile-upload");

        // Handle cache invalidation
        if let Some(lowest) = self.lowest_dirty_layer.take() {
            self.invalidate_cache_from(lowest);
        }

        // After tile upload, re-check: if still nothing to composite, bail out.
        if !self.needs_composite {
            // Clear empty dirty regions and return — no GPU work.
            for dirty in doc.dirty.values_mut() {
                dirty.clear();
            }
            perf::time_end("render-total");
            return;
        }

        // 3. Compute dirty bounding rect (P3) — union of all dirty regions in pixel coords.
        // This rect limits all compositing passes via scissor and scoped texture copies.
        let dirty_rect = dirty_pixel_rect(
            doc.dirty.values(),
            self.canvas_width,
            self.canvas_height,
        );

        // If needs_composite was set by a non-tile-dirty source (e.g. layer property change,
        // undo/redo), we need a full-canvas rect since there's no tile-level dirty info.
        let (scissor_x, scissor_y, scissor_w, scissor_h) = dirty_rect
            .unwrap_or((0, 0, self.canvas_width, self.canvas_height));

        #[cfg(feature = "profile")]
        log::info!(
            "scissor: ({scissor_x},{scissor_y} {scissor_w}x{scissor_h}), start_layer will be from cache_valid_through={:?}, layers={}",
            self.cache_valid_through,
            doc.layers.len(),
        );

        // 4. Acquire surface — only when we actually have work to do.
        perf::time("acquire-surface");
        let output = match surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost) => {
                surface.configure(device, surface_config);
                return;
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                log::error!("Out of GPU memory");
                return;
            }
            Err(e) => {
                log::warn!("Surface error: {e:?}");
                return;
            }
        };
        let surface_view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        perf::time_end("acquire-surface");

        // 5. Composite cache (P3): determine start point
        let start_layer = match self.cache_valid_through {
            Some(valid_through) => valid_through + 1,
            None => 0,
        };
        let resuming_from_cache = start_layer > 0;
        // Track whether the first layer after cache resume still needs the
        // cache_source_bind_group (reads from composite_cache instead of accum).
        let mut use_cache_source = resuming_from_cache;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("composite"),
        });

        if !resuming_from_cache {
            // Clear accumulator[0] for fresh composite (fullscreen — first frame or full invalidation)
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
        // When resuming from cache, we DON'T copy cache→accum.
        // Instead, the first blend pass uses cache_source_bind_group which
        // reads directly from composite_cache. This saves one fullscreen copy.

        // 6. Composite layers from start_layer to top.
        // `wrote_to_cache` tracks whether the final result landed in
        // composite_cache (true) or in accum[current_accum] (false).
        perf::time("composite-layers");
        let num_layers = doc.layers.len();
        let mut wrote_to_cache = false;

        for layer_idx in start_layer..num_layers {
            let layer = &doc.layers[layer_idx];
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
                        // First layer after cache resume: read from cache texture.
                        // MUST write to accum (not cache) to avoid read-write hazard.
                        use_cache_source = false;
                        let dst = 0;
                        self.current_accum = dst;
                        (&self.accum_views[dst], &cache.cache_source_bind_group)
                    } else if is_last_layer {
                        // Last layer, not reading from cache: render directly to
                        // composite_cache to skip the post-loop copy.
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

                    // For filters resuming from cache, we need to copy cache→accum
                    // since filter bind groups only reference accum views.
                    // Scope the copy to the dirty rect only.
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
                            // Last pass of last layer: render directly to cache.
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

        // If the final result is in an accumulator, copy only the dirty rect to the cache.
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

        perf::time_end("composite-layers");

        // 6. Present: blit composite_cache to surface
        perf::time("present");
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
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
        }

        perf::time_end("present");

        perf::time("submit+present");
        queue.submit(std::iter::once(encoder.finish()));
        output.present();
        perf::time_end("submit+present");

        // 7. Clear dirty regions, reset flag
        for dirty in doc.dirty.values_mut() {
            dirty.clear();
        }
        self.needs_composite = false;
        self.needs_present = false;
        perf::time_end("render-total");
    }

    /// Re-present the composite cache to the surface without recompositing.
    /// Used when only the view transform changed.
    fn present_only(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: &wgpu::Surface,
        surface_config: &wgpu::SurfaceConfiguration,
    ) {
        let output = match surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost) => {
                surface.configure(device, surface_config);
                return;
            }
            Err(_) => return,
        };
        let surface_view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("present-only"),
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
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
        }

        queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }
}
