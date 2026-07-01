/// Shared GPU pipeline for an effect type (filter or veil).
/// Arc-wrapped so multiple instances of the same effect share them.
pub struct EffectPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
}

impl std::fmt::Debug for EffectPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectPipeline").finish_non_exhaustive()
    }
}

use super::filter::FilterEffect;

/// Cached GPU objects for an effect instance.
/// Created once at instance creation, never in the render loop.
/// Used by both filters (layer-level) and veils (viewport-level).
pub struct EffectCache {
    /// One uniform buffer per pass.
    pub uniform_bufs: Vec<wgpu::Buffer>,
    /// One bind group per pass, per ping-pong direction.
    /// Indexed as bind_groups[pass_index][ping_pong_src].
    pub bind_groups: Vec<[wgpu::BindGroup; 2]>,
    /// Optional auxiliary textures (e.g., noise texture, intermediate render targets).
    pub aux_textures: Vec<wgpu::Texture>,
    pub aux_views: Vec<wgpu::TextureView>,
    /// Optional auxiliary pipelines (e.g., blit pipeline for upscale passes).
    /// Veils that render at a lower internal resolution use this for the
    /// upscale blit back to viewport size.
    pub aux_pipelines: Vec<wgpu::RenderPipeline>,
}

impl EffectCache {
    /// An empty cache holding no resources. The starting state a `FilterEffect`
    /// fills in `ensure`, and the throwaway a parameter-free filter's
    /// `render` reads from on the destructive region path.
    pub fn empty() -> Self {
        EffectCache {
            uniform_bufs: Vec::new(),
            bind_groups: Vec::new(),
            aux_textures: Vec::new(),
            aux_views: Vec::new(),
            aux_pipelines: Vec::new(),
        }
    }
}

/// Build a render pipeline from a passthrough blit shader.
/// Used by veils and the compositor's final blit to surface.
pub fn create_blit_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
) -> EffectPipeline {
    create_filter_pipeline(
        device,
        format,
        label,
        include_str!("../../shaders/blit.wgsl"),
        "fs_blit",
    )
}

/// Build a render pipeline for the multi-tap soft downscale shader.
/// Used by the veil chain to feed reduced-resolution veils with a
/// properly anti-aliased input — single-tap bilinear (blit) aliases
/// hard at any downscale ratio worse than ~0.7 because it's a fixed
/// 2×2 box filter regardless of the source/destination ratio.
pub fn create_downscale_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
) -> EffectPipeline {
    create_filter_pipeline(
        device,
        format,
        label,
        include_str!("../../shaders/downscale.wgsl"),
        "fs_downscale",
    )
}

/// Shared pipeline builder for texture+sampler shaders that share the
/// blit bind-group layout (binding 0 = texture, binding 1 = sampler).
fn create_filter_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
    shader_source: &str,
    fragment_entry: &str,
) -> EffectPipeline {
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(&format!("{label}-bgl")),
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

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(&format!("{label}-layout")),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&format!("{label}-shader")),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(fragment_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format,
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

    EffectPipeline {
        pipeline,
        bind_group_layout,
    }
}

/// A `textureLoad`-based filter that runs over a node region with an optional
/// selection mask — the shared home for masked, parameter-free region filters
/// (the destructive-filter substrate). One object holds the four pipelines
/// a node filter needs: plain vs. masked × RGBA8 (layer) vs. R8 (mask), so a
/// single registration serves layers and masks alike. The compositor drives it
/// via `filter_node_region` (see `compositor.rs`).
///
/// Deliberately distinct from [`OrthoTransformPass`](super::ortho_transform::OrthoTransformPass)'s
/// own four-pipeline builder: that pass carries a params uniform (xform code,
/// src dims, `has_mask`); this builder is parameter-free. What the two share is
/// the region *plumbing*, not the pass construction.
pub struct MaskedFilterPipeline {
    plain_rgba: wgpu::RenderPipeline,
    plain_r8: wgpu::RenderPipeline,
    masked_rgba: wgpu::RenderPipeline,
    masked_r8: wgpu::RenderPipeline,
    /// `[src texture]` — used by the plain entry point.
    plain_bgl: wgpu::BindGroupLayout,
    /// `[src texture, mask texture]` — used by the masked entry point.
    masked_bgl: wgpu::BindGroupLayout,
}

impl std::fmt::Debug for MaskedFilterPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MaskedFilterPipeline")
            .finish_non_exhaustive()
    }
}

/// A `textureLoad` source binding (no sampler / hardware filtering) — the
/// inputs of a parameter-free region filter are read by integer texel index.
fn load_tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

impl MaskedFilterPipeline {
    /// Build the plain+masked × RGBA8+R8 pipelines from one shader source. The
    /// plain entry point reads `[src]`; the masked entry point reads
    /// `[src, mask]` and blends to the original where the mask is unselected.
    pub fn new(
        device: &wgpu::Device,
        label: &str,
        shader_source: &str,
        plain_entry: &str,
        masked_entry: &str,
    ) -> MaskedFilterPipeline {
        let plain_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{label}-plain-bgl")),
            entries: &[load_tex_entry(0)],
        });
        let masked_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{label}-masked-bgl")),
            entries: &[load_tex_entry(0), load_tex_entry(1)],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&format!("{label}-shader")),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let make =
            |bgl: &wgpu::BindGroupLayout, entry: &str, format: wgpu::TextureFormat, lbl: &str| {
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(lbl),
                    bind_group_layouts: &[Some(bgl)],
                    immediate_size: 0,
                });
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(lbl),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(entry),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
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

        let rgba = wgpu::TextureFormat::Rgba8Unorm;
        let r8 = wgpu::TextureFormat::R8Unorm;
        MaskedFilterPipeline {
            plain_rgba: make(
                &plain_bgl,
                plain_entry,
                rgba,
                &format!("{label}-plain-rgba"),
            ),
            plain_r8: make(&plain_bgl, plain_entry, r8, &format!("{label}-plain-r8")),
            masked_rgba: make(
                &masked_bgl,
                masked_entry,
                rgba,
                &format!("{label}-masked-rgba"),
            ),
            masked_r8: make(&masked_bgl, masked_entry, r8, &format!("{label}-masked-r8")),
            plain_bgl,
            masked_bgl,
        }
    }

    /// Run the filter over a `src_view`, writing the result to `out_view` (same
    /// dims). With `mask_view` present, only mask-selected texels take the
    /// filtered value; elsewhere the original passes through. `format` selects
    /// the RGBA8 (layer) or R8 (mask) pipeline.
    pub fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        mask_view: Option<&wgpu::TextureView>,
        out_view: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let is_r8 = format == wgpu::TextureFormat::R8Unorm;
        let (pipeline, bind_group) = match mask_view {
            Some(mv) => {
                let pipeline = if is_r8 {
                    &self.masked_r8
                } else {
                    &self.masked_rgba
                };
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("masked-filter-bg"),
                    layout: &self.masked_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(src_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(mv),
                        },
                    ],
                });
                (pipeline, bind_group)
            }
            None => {
                let pipeline = if is_r8 {
                    &self.plain_r8
                } else {
                    &self.plain_rgba
                };
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("plain-filter-bg"),
                    layout: &self.plain_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(src_view),
                    }],
                });
                (pipeline, bind_group)
            }
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("masked-filter-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: out_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// A `MaskedFilterPipeline` is a parameter-free [`FilterEffect`]: `ensure` has
/// nothing to build, and `render` delegates straight to the inherent method,
/// ignoring the cache. This is all invert (and any future parameter-free
/// filter) needs to slot into the filter registry.
impl FilterEffect for MaskedFilterPipeline {
    fn ensure(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _params: &[super::params::ParamValue],
        _cache: &mut EffectCache,
    ) {
    }

    fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        mask: Option<&wgpu::TextureView>,
        out: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        _cache: &EffectCache,
    ) {
        MaskedFilterPipeline::render(self, device, encoder, src, mask, out, format);
    }
}

/// Create a bind group for a simple blit (texture + sampler).
pub fn create_blit_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
