use crate::atlas::LayerTexture;
use crate::blend::BlendPipelines;
use crate::filter::{FilterLayerCache, FilterRegistry};
use crate::filters::noise;
use crate::staging::StagingRing;
use darkly_core::dirty::dirty_pixel_rect;
use darkly_core::document::Document;
use darkly_core::layer::{BlendMode, Layer, LayerId};
use std::collections::HashMap;

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
    /// Present bind group that reads from composite_cache directly.
    present_cache_bind_group: wgpu::BindGroup,

    staging: StagingRing,
    sampler: wgpu::Sampler,

    /// Dirty gate — false means nothing changed, skip compositing (P2).
    needs_composite: bool,
    /// Track lowest dirty layer index for cache invalidation.
    lowest_dirty_layer: Option<usize>,

    canvas_width: u32,
    canvas_height: u32,
}

impl Compositor {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        // Use Rgba8Unorm for accumulators (linear color space for blending)
        let accum_format = wgpu::TextureFormat::Rgba8Unorm;

        let make_accum = |label: &str| -> (wgpu::Texture, wgpu::TextureView) {
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

        // Filter registry — register all filter types
        let mut filter_registry = FilterRegistry::new();
        filter_registry.register(
            noise::FILTER_TYPE,
            Box::new(noise::NoiseHandler::new(device, accum_format)),
        );

        // Present pipeline: blit accumulator to surface
        let present_bind_group_layout =
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
                ],
            });

        let present_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("present-pipeline-layout"),
                bind_group_layouts: &[&present_bind_group_layout],
                push_constant_ranges: &[],
            });

        let present_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/present.wgsl").into(),
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
            layout: &present_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&composite_cache_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
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
            present_cache_bind_group,
            staging,
            sampler,
            needs_composite: true,
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
        params: &dyn darkly_core::layer::FilterParams,
    ) {
        if self.filter_cache.contains_key(&layer_id) {
            return;
        }

        let type_id = params.filter_type_id();
        let handler = self
            .filter_registry
            .get(type_id)
            .unwrap_or_else(|| panic!("Unknown filter type: {type_id}"));

        let cache = handler.create_instance(
            device,
            queue,
            params,
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
    }

    /// Invalidate the composite cache from the given layer index upward.
    fn invalidate_cache_from(&mut self, layer_index: usize) {
        match self.cache_valid_through {
            Some(valid) if valid >= layer_index => {
                // Cache is only valid through below the dirty layer
                self.cache_valid_through = if layer_index > 0 {
                    Some(layer_index - 1)
                } else {
                    None
                };
            }
            None => {} // Already fully invalid
            _ => {}    // Cache is valid below the dirty layer, fine
        }
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

    /// Access the filter registry for param creation.
    pub fn filter_registry(&self) -> &FilterRegistry {
        &self.filter_registry
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
        // 1. Check if any dirty regions exist before scanning layers.
        let has_dirty = doc.dirty.values().any(|d| !d.is_empty());

        // P2: Dirty gate — if nothing changed and no dirty tiles, skip entirely.
        // The browser compositor keeps displaying the last presented surface frame;
        // no surface acquisition, no command encoder, no GPU work at all.
        if !self.needs_composite && !has_dirty {
            return;
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
                        if let Some(tile) = raster.tiles.get(tx, ty) {
                            self.staging.upload_tile(
                                queue,
                                tile.data(),
                                &layer_tex.texture,
                                tx as u32,
                                ty as u32,
                            );
                        }
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

        // 4. Acquire surface — only when we actually have work to do.
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
                Layer::Filter(filter) => {
                    let type_id = filter.params.filter_type_id();
                    let handler = match self.filter_registry.get(type_id) {
                        Some(h) => h,
                        None => continue,
                    };
                    let cache = match self.filter_cache.get(&filter.id) {
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

                    for pass in 0..handler.pass_count() as usize {
                        let src = self.current_accum;
                        let dst = 1 - src;

                        let is_last_pass = pass == handler.pass_count() as usize - 1;
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
                            rpass.set_pipeline(handler.pipeline());
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

        // 6. Present: blit composite_cache to surface
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

        // 7. Clear dirty regions, reset flag
        for dirty in doc.dirty.values_mut() {
            dirty.clear();
        }
        self.needs_composite = false;
    }
}
