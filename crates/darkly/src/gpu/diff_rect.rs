//! GPU diff of a layer against its pre-operation snapshot, for undo.
//!
//! A compute shader compares two textures (the snapshot and the current
//! layer) and produces the tight bounding rect of all differing pixels plus
//! one changed-flag per tile of the format's [`TileGrid`]. The result becomes
//! the [`RegionRuns`] an undo entry stores, so an operation's undo costs the
//! tiles it changed without hand-tracking where it painted.
//!
//! The "differs from texture B" predicate, its two-texture bind group, and
//! the decoding into runs are all that's specific here; the dispatch and
//! async readback live in [`PendingReduction`].

use super::reduction::{decode_bounds, PendingReduction, BOUNDS_INIT};
use super::region_runs::{bytes_per_pixel, RegionRuns, TileGrid};
use crate::coord::CanvasRect;

pub struct DiffRectPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

/// An in-flight diff awaiting its readback.
pub struct PendingDiff {
    reduction: PendingReduction,
    /// Canvas-space extent of the layer at the time of `request`. Used to
    /// translate the shader's layer-local result back to canvas coords on
    /// `poll`. It must be captured at request time so the result remains
    /// correct even if the layer grows between request and poll.
    layer_canvas_extent: CanvasRect,
    grid: TileGrid,
    bpp: u32,
}

/// Uniform buffer layout matching the shader's `Params` struct.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    width: u32,
    height: u32,
    tile_w: u32,
    tile_h: u32,
}

impl DiffRectPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("diff-rect-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/diff_rect.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("diff-rect-bgl"),
            entries: &[
                // binding 0: texture A (scratch / pre-stroke)
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // binding 1: texture B (current canvas)
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // binding 2: atomic bounds + tile flags storage buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 3: params uniform
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("diff-rect-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("diff-rect-pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        DiffRectPass {
            pipeline,
            bind_group_layout,
        }
    }

    /// Dispatch the diff compute shader comparing two textures of `format`.
    ///
    /// `before_view` is the pre-operation snapshot, `current_view` is the
    /// layer now; both are aligned so texel `(x, y)` is the same layer pixel.
    /// `layer_canvas_extent` is the canvas-space rect occupied by the layer at
    /// request time, used to translate the layer-local result back to canvas
    /// coords when [`PendingDiff::poll`] resolves, so the result stays correct
    /// even if the layer grows before it is read.
    pub fn request(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        before_view: &wgpu::TextureView,
        current_view: &wgpu::TextureView,
        layer_canvas_extent: CanvasRect,
        format: wgpu::TextureFormat,
    ) -> PendingDiff {
        let width = layer_canvas_extent.width;
        let height = layer_canvas_extent.height;
        let grid = TileGrid::for_format(format);

        let params = Params {
            width,
            height,
            tile_w: grid.tile_w,
            tile_h: grid.tile_h,
        };
        let param_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("diff-rect-params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        param_buf
            .slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(bytemuck::bytes_of(&params));
        param_buf.unmap();

        let mut seed = BOUNDS_INIT.to_vec();
        seed.resize(BOUNDS_INIT.len() + grid.flag_words(width, height), 0);

        let reduction = PendingReduction::dispatch(
            device,
            queue,
            &self.pipeline,
            |storage| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("diff-rect-bg"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(before_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(current_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: storage.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: param_buf.as_entire_binding(),
                        },
                    ],
                })
            },
            &seed,
            width,
            height,
        );

        PendingDiff {
            reduction,
            layer_canvas_extent,
            grid,
            bpp: bytes_per_pixel(format),
        }
    }
}

impl PendingDiff {
    /// Poll for the diff result. Returns `Some(Some(runs))` when ready,
    /// `Some(None)` if the textures are identical (or the readback failed),
    /// or `None` if still pending.
    ///
    /// The runs are in canvas coords: the shader produces layer-local bounds
    /// and tile flags, which are translated using the layer's canvas extent
    /// captured at [`DiffRectPass::request`] time, so the result remains
    /// valid through layer growth between request and poll.
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<Option<RegionRuns>> {
        let Some(words) = self.reduction.poll(device)? else {
            return Some(None);
        };
        let (bounds, flags) = words.split_first_chunk::<4>()?;
        Some(decode_bounds(*bounds).map(|[x, y, w, h]| {
            let extent = self.layer_canvas_extent;
            let tight_bbox =
                CanvasRect::from_xywh(extent.origin.x + x as i32, extent.origin.y + y as i32, w, h);
            RegionRuns::from_tile_flags(flags, self.grid, extent, tight_bbox, self.bpp)
        }))
    }
}
