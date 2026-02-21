use std::collections::HashMap;

use wgpu::util::DeviceExt;

use darkly_core::document::Document;
use darkly_core::layer::{FilterType, Layer, LayerId};

use crate::atlas::{create_layer_texture, LAYER_FORMAT};
use crate::blend::BlendPipelines;
use crate::filter::FilterPipelines;
use crate::staging::StagingRing;

/// Uniform for composite shader — must match Uniforms in composite.wgsl
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CompositeUniforms {
    opacity: f32,
    blend_mode: u32,
    _pad0: u32,
    _pad1: u32,
}

/// Cached GPU objects for a single raster layer (P1).
/// Created once in ensure_layer_texture(), never in the render loop.
struct LayerCache {
    #[allow(dead_code)] // Kept alive so bind groups referencing it remain valid
    uniform_buf: wgpu::Buffer,
    /// Two bind groups: [0] for when current_accum=0 (src=0,dst=1),
    ///                   [1] for when current_accum=1 (src=1,dst=0).
    bind_groups: [wgpu::BindGroup; 2],
}

/// Orchestrates the full render pipeline:
/// 1. Upload dirty tiles to per-layer GPU textures
/// 2. Composite layers bottom-to-top with blend modes (ping-pong accumulators)
/// 3. Apply filter layers to the accumulator
/// 4. Present final result to the surface
///
/// Performance: follows P1 (zero GPU allocation in render loop),
/// P2 (skip compositing when nothing changed).
pub struct Compositor {
    /// Three accumulator textures: two for compositing ping-pong, one for blur temp
    #[allow(dead_code)]
    accum: [wgpu::Texture; 3],
    accum_views: [wgpu::TextureView; 3],
    current_accum: usize,

    /// Per-layer GPU textures (one per raster layer)
    layer_textures: HashMap<LayerId, wgpu::Texture>,
    layer_views: HashMap<LayerId, wgpu::TextureView>,

    /// Per-layer cached GPU objects (P1)
    layer_cache: HashMap<LayerId, LayerCache>,

    blend_pipelines: BlendPipelines,
    filter_pipelines: FilterPipelines,
    present_pipeline: wgpu::RenderPipeline,
    #[allow(dead_code)] // Kept alive so present bind groups remain valid
    present_bgl: wgpu::BindGroupLayout,
    /// Cached present bind groups: [0] blits accum_views[0], [1] blits accum_views[1]
    present_bind_groups: [wgpu::BindGroup; 2],
    staging: StagingRing,
    sampler: wgpu::Sampler,

    /// P2: dirty gate — false means nothing changed, skip compositing
    needs_composite: bool,

    canvas_width: u32,
    canvas_height: u32,
}

impl Compositor {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat, width: u32, height: u32) -> Self {
        let blend_pipelines = BlendPipelines::new(device, LAYER_FORMAT);

        // Create 3 accumulator textures
        let accum: [wgpu::Texture; 3] = std::array::from_fn(|i| {
            create_layer_texture(device, width, height, &format!("accum_{i}"))
        });
        let accum_views: [wgpu::TextureView; 3] = std::array::from_fn(|i| {
            accum[i].create_view(&wgpu::TextureViewDescriptor::default())
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Create filter pipelines with cached blur bind groups (P1)
        let filter_pipelines = FilterPipelines::new(device, LAYER_FORMAT, &accum_views, &sampler);

        // Present pipeline: blit accumulator to surface
        let (present_pipeline, present_bgl) = Self::create_present_pipeline(device, surface_format);

        // Pre-build present bind groups for both possible accumulators (P1)
        let present_bind_groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("present_bg_{i}")),
                layout: &present_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&accum_views[i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        });

        Compositor {
            accum,
            accum_views,
            current_accum: 0,
            layer_textures: HashMap::new(),
            layer_views: HashMap::new(),
            layer_cache: HashMap::new(),
            blend_pipelines,
            filter_pipelines,
            present_pipeline,
            present_bgl,
            present_bind_groups,
            staging: StagingRing::new(),
            sampler,
            needs_composite: true,
            canvas_width: width,
            canvas_height: height,
        }
    }

    fn create_present_pipeline(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let fullscreen_src = include_str!("../../../shaders/fullscreen.wgsl");
        let present_src = include_str!("../../../shaders/present.wgsl");

        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present_vs"),
            source: wgpu::ShaderSource::Wgsl(fullscreen_src.into()),
        });
        let fs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present_fs"),
            source: wgpu::ShaderSource::Wgsl(present_src.into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("present_bgl"),
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

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("present_layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs_module,
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

        (pipeline, bgl)
    }

    /// Ensure a GPU texture + cached bind groups exist for the given raster layer.
    /// Called once when a layer is added, never in the render loop (P1).
    pub fn ensure_layer_texture(&mut self, device: &wgpu::Device, layer_id: LayerId, opacity: f32, blend_mode: darkly_core::layer::BlendMode) {
        if self.layer_textures.contains_key(&layer_id) {
            return;
        }

        let tex = create_layer_texture(
            device,
            self.canvas_width,
            self.canvas_height,
            &format!("layer_{layer_id}"),
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());

        // Create uniform buffer with COPY_DST so we can update via write_buffer (P1)
        let uniforms = CompositeUniforms {
            opacity,
            blend_mode: blend_mode.as_u32(),
            _pad0: 0,
            _pad1: 0,
        };
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("layer_{layer_id}_uniforms")),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create two bind groups: one per ping-pong direction
        // [0]: src=accum[0], dst=accum[1]
        // [1]: src=accum[1], dst=accum[0]
        let bind_groups = std::array::from_fn(|src| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("layer_{layer_id}_bg_{src}")),
                layout: &self.blend_pipelines.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&self.accum_views[src]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
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

        self.layer_views.insert(layer_id, view);
        self.layer_textures.insert(layer_id, tex);
        self.layer_cache.insert(layer_id, LayerCache { uniform_buf, bind_groups });
    }

    /// Mark that recompositing is needed (P2).
    /// Called by mutation paths (paint, set_opacity, set_blend_mode, undo, redo, add_layer).
    pub fn mark_dirty(&mut self) {
        self.needs_composite = true;
    }

    /// Whether anything needs rendering (tiles to upload or compositing needed).
    pub fn needs_render(&self, doc: &Document) -> bool {
        if self.needs_composite {
            return true;
        }
        doc.dirty.values().any(|d| !d.is_empty())
    }

    /// Full render: upload dirty tiles, composite (if needed), present.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: &wgpu::Surface,
        _surface_config: &wgpu::SurfaceConfiguration,
        doc: &mut Document,
    ) {
        // 1. Upload dirty tiles to per-layer GPU textures
        for layer in &doc.layers {
            if let Layer::Raster(raster) = layer {
                self.ensure_layer_texture(device, raster.id, raster.opacity, raster.blend_mode);

                if let Some(dirty) = doc.dirty.get(&raster.id) {
                    if !dirty.is_empty() {
                        let texture = &self.layer_textures[&raster.id];
                        for &(tx, ty) in &dirty.tiles {
                            if let Some(tile) = raster.tiles.get(tx, ty) {
                                self.staging.upload_tile(queue, &tile.data, texture, tx, ty);
                            }
                        }
                        // Tiles uploaded → need to recomposite
                        self.needs_composite = true;
                    }
                }
            }
        }

        // 2. P2: dirty gate — skip compositing if nothing changed
        if !self.needs_composite {
            self.present_only(device, queue, surface);
            doc.clear_dirty();
            return;
        }

        // 3. Begin compositing
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("compositor"),
        });

        // Clear accum[0] to transparent
        self.current_accum = 0;
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear_accum"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.accum_views[0],
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
        }

        // 4. Composite each layer bottom-to-top
        // Destructure to satisfy borrow checker: we need &mut filter_pipelines
        // while also reading accum_views, sampler, layer_cache, blend_pipelines
        let accum_views = &self.accum_views;
        let _sampler = &self.sampler;
        let blend_pipelines = &self.blend_pipelines;
        let layer_cache = &self.layer_cache;
        let filter_pipelines = &mut self.filter_pipelines;
        let mut current_accum = self.current_accum;

        for layer in &doc.layers {
            if !layer.visible() {
                continue;
            }

            match layer {
                Layer::Raster(raster) => {
                    let cache = match layer_cache.get(&raster.id) {
                        Some(c) => c,
                        None => continue,
                    };

                    let src = current_accum;
                    let dst = if src == 0 { 1 } else { 0 };

                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("composite_pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &accum_views[dst],
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

                        pass.set_pipeline(blend_pipelines.get(raster.blend_mode));
                        // Use cached bind group for this ping-pong direction (P1)
                        pass.set_bind_group(0, &cache.bind_groups[src], &[]);
                        pass.draw(0..3, 0..1);
                    }

                    current_accum = dst;
                }

                Layer::Filter(filter) => {
                    match filter.filter_type {
                        FilterType::GaussianBlur => {
                            let src = current_accum;
                            let dst = if src == 0 { 1 } else { 0 };
                            let temp = 2;

                            filter_pipelines.run_blur_indexed(
                                queue,
                                &mut encoder,
                                src,
                                &accum_views[temp],
                                &accum_views[dst],
                                &filter.params,
                            );

                            current_accum = dst;
                        }
                    }
                }
            }
        }

        self.current_accum = current_accum;

        // 5. Present: blit current accumulator to surface
        let frame = surface
            .get_current_texture()
            .expect("failed to get surface texture");
        let frame_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &frame_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            pass.set_pipeline(&self.present_pipeline);
            // Use cached present bind group (P1)
            pass.set_bind_group(0, &self.present_bind_groups[self.current_accum], &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        // 6. Clear dirty state
        doc.clear_dirty();
        self.needs_composite = false;
    }

    /// Lightweight present when nothing changed (P2) — just blit last frame.
    fn present_only(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: &wgpu::Surface,
    ) {
        let frame = surface
            .get_current_texture()
            .expect("failed to get surface texture");
        let frame_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("present_only"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present_only_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &frame_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            pass.set_pipeline(&self.present_pipeline);
            pass.set_bind_group(0, &self.present_bind_groups[self.current_accum], &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}
