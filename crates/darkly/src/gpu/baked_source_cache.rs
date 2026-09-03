//! Cache of baked procedural texture tiles for brush graphs.
//!
//! A brush `noise` node with a static field bakes that field once into a
//! texture (running the existing `fbm_tile` shader over a tile) and then
//! samples it like any `@group(3)` graph texture, turning an ~80-hash fBm
//! kernel re-run per fragment per overlapping dab into a single
//! `textureSample`. This cache owns the bake render pipelines and the baked
//! tiles, keyed by the field-defining [`BakeSpec`]. Two brushes (or two
//! nodes) with an equal spec share one tile.
//!
//! Lifecycle: this is **Compositor** state; a baked tile is derived from and
//! fully rebuildable from the graph, never stored in the Document. It lives
//! beside [`crate::gpu::texture_registry::TextureRegistry`] on
//! `BrushPipelines`, reached through `&self` at bind-group build time (hence
//! the interior-mutable tile map).
//!
//! The bake is a **render pass**, not a GPU readback, so there is no
//! No-Blocking-GPU-Readbacks concern (the `fbm_tile` field is a pure,
//! binding-free function; the void noise pass is the same technique).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::brush::texture_source::{BakeChannels, BakeKind, BakeSpec};
use crate::gpu::texture_registry::GpuTexture;

/// Uniform fed to the bake shader. Mirrors `BakeParams` in
/// `shaders/brush/bake_source.wgsl` (std140-friendly: 32 bytes).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct BakeParams {
    seed: u32,
    octaves: i32,
    gain: f32,
    warp: f32,
    field_span: f32,
    channels: u32,
    _pad0: u32,
    _pad1: u32,
}

/// The bake render pipelines: the uniform bind-group layout and one pipeline
/// per output format. Built lazily on the first actual bake (see
/// [`BakedSourceCache`]), so an engine whose brushes never bake noise does no
/// GPU pipeline work for this feature.
struct BakePipes {
    bind_group_layout: wgpu::BindGroupLayout,
    rgba_pipeline: wgpu::RenderPipeline,
    grayscale_pipeline: wgpu::RenderPipeline,
}

impl BakePipes {
    fn build(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("baked-source-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("baked-source-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // WGSL has no `#include`; concatenate the fBm math (which owns the
        // noise algorithm) ahead of the bake shader: the void noise pattern.
        // The field is not reimplemented here; the bake runs `fbm_tile`.
        let fbm2d_src = include_str!("../../shaders/lib/fbm2d.wgsl");
        let bake_src = include_str!("../../shaders/brush/bake_source.wgsl");
        let full_src = format!("{fbm2d_src}\n{bake_src}");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("baked-source-shader"),
            source: wgpu::ShaderSource::Wgsl(full_src.into()),
        });

        let make_pipeline = |format: wgpu::TextureFormat, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                    entry_point: Some("fs_main"),
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
        Self {
            bind_group_layout,
            rgba_pipeline: make_pipeline(wgpu::TextureFormat::Rgba8Unorm, "baked-source-rgba"),
            grayscale_pipeline: make_pipeline(wgpu::TextureFormat::R8Unorm, "baked-source-r8"),
        }
    }
}

/// Owns the bake render pipelines and the cache of baked tiles.
pub struct BakedSourceCache {
    /// Bake pipelines, built lazily on the first bake. An engine whose brushes
    /// never bake noise (the common case, and every non-painting test) does no
    /// GPU work here at all.
    pipes: RefCell<Option<BakePipes>>,
    /// Baked tiles keyed by field-defining spec. `RefCell` because the cache
    /// is reached through `&self` at bind-group build time (mirrors
    /// `TextureRegistry::layouts`). `Arc` so a tile outlives the transient
    /// bind group and is shared across brushes with an equal spec.
    tiles: RefCell<HashMap<BakeSpec, Arc<GpuTexture>>>,
}

impl Default for BakedSourceCache {
    fn default() -> Self {
        Self::new()
    }
}

impl BakedSourceCache {
    pub fn new() -> Self {
        Self {
            pipes: RefCell::new(None),
            tiles: RefCell::new(HashMap::new()),
        }
    }

    /// Return the baked tile for `spec`, baking and caching it on first use.
    pub fn get_or_bake(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        spec: &BakeSpec,
    ) -> Arc<GpuTexture> {
        if let Some(tile) = self.tiles.borrow().get(spec) {
            return tile.clone();
        }
        let tile = self.bake(device, queue, spec);
        self.tiles.borrow_mut().insert(*spec, tile.clone());
        tile
    }

    fn bake(&self, device: &wgpu::Device, queue: &wgpu::Queue, spec: &BakeSpec) -> Arc<GpuTexture> {
        // Build the bake pipelines on first use, then borrow them for the pass.
        if self.pipes.borrow().is_none() {
            *self.pipes.borrow_mut() = Some(BakePipes::build(device));
        }
        let pipes = self.pipes.borrow();
        let pipes = pipes.as_ref().expect("bake pipelines built above");

        let (format, pipeline, channels) = match spec.channels {
            BakeChannels::Grayscale => (
                wgpu::TextureFormat::R8Unorm,
                &pipes.grayscale_pipeline,
                0u32,
            ),
            BakeChannels::Rgba => (wgpu::TextureFormat::Rgba8Unorm, &pipes.rgba_pipeline, 1u32),
        };
        let res = spec.resolution;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("baked-source-tile"),
            size: wgpu::Extent3d {
                width: res,
                height: res,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            // `COPY_SRC` lets a baked tile be read back, used by the seam
            // regression test to verify the field tiles; a nil always-on cost.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let BakeKind::Noise {
            seed,
            octaves,
            warp_q,
            roughness_q,
        } = spec.kind;
        let params = BakeParams {
            seed,
            octaves,
            gain: BakeKind::dequantize(roughness_q),
            warp: BakeKind::dequantize(warp_q),
            field_span: BakeSpec::FIELD_SPAN,
            channels,
            _pad0: 0,
            _pad1: 0,
        };
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("baked-source-params"),
            size: std::mem::size_of::<BakeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&params));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("baked-source-bg"),
            layout: &pipes.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("baked-source-encoder"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("baked-source-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, &bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
        queue.submit(Some(encoder.finish()));

        Arc::new(GpuTexture {
            texture,
            view,
            width: res,
            height: res,
        })
    }
}
