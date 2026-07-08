//! GPU-accelerated diff bounding rect computation.
//!
//! A compute shader compares two textures (pre-stroke scratch vs post-stroke
//! canvas) and produces the tight bounding rect of all differing pixels using
//! atomic min/max. Used at stroke end to determine the exact undo region
//! without hand-tracking dab positions.
//!
//! The "differs from texture B" predicate + its two-texture bind group are all
//! that's specific here; the atomic min/max machinery and async readback live
//! in [`BboxReduction`](super::bbox::BboxReduction).

use super::bbox::{BboxReduction, PendingBbox};

pub struct DiffRectPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    pending: Option<PendingDiff>,
}

struct PendingDiff {
    bbox: PendingBbox,
    /// Canvas-space extent of the layer at the time of `request`. Used to
    /// translate the shader's layer-local bounding rect back to canvas
    /// coords on `poll` — must be captured at request time so the result
    /// remains correct even if the layer grows between request and poll.
    layer_canvas_extent: crate::coord::CanvasRect,
}

/// Uniform buffer layout matching the shader's `Params` struct.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    width: u32,
    height: u32,
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
                // binding 2: atomic bounds storage buffer
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
            pending: None,
        }
    }

    /// Dispatch the diff compute shader comparing two textures.
    ///
    /// `scratch_view` is the pre-stroke snapshot, `current_view` is the
    /// post-stroke canvas. `layer_canvas_extent` is the canvas-space rect
    /// occupied by the layer at request time — used to translate the
    /// layer-local result back to canvas coords when [`poll`] resolves.
    /// Capturing it at request time keeps the result correct even if the
    /// layer grows before the result is read. Results arrive asynchronously
    /// via [`poll`].
    pub fn request(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scratch_view: &wgpu::TextureView,
        current_view: &wgpu::TextureView,
        layer_canvas_extent: crate::coord::CanvasRect,
    ) {
        let width = layer_canvas_extent.width;
        let height = layer_canvas_extent.height;

        let params = Params { width, height };
        let param_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("diff-rect-params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        {
            let mut mapping = param_buf.slice(..).get_mapped_range_mut();
            mapping.copy_from_slice(bytemuck::bytes_of(&params));
        }
        param_buf.unmap();

        let bbox = BboxReduction::dispatch(
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
                            resource: wgpu::BindingResource::TextureView(scratch_view),
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
            width,
            height,
        );

        self.pending = Some(PendingDiff {
            bbox,
            layer_canvas_extent,
        });
    }

    /// True if a diff result is pending.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Poll for the diff result. Returns `Some(Some(rect))` when ready,
    /// `Some(None)` if the textures are identical, or `None` if still pending.
    ///
    /// The rect is returned in canvas coords: the shader produces
    /// layer-local bounds, which are translated using the layer's canvas
    /// extent captured at [`request`](Self::request) time. The result
    /// therefore remains valid through layer growth between request and poll.
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<Option<crate::coord::CanvasRect>> {
        // Still pending (no request, or the reduction hasn't landed) → None.
        let ready = self.pending.as_mut()?.bbox.poll(device)?;
        // The reduction landed — consume the request and map its texel-local
        // rect into canvas coords via the extent captured at request time.
        let pending = self.pending.take().unwrap();
        Some(ready.map(|[x, y, w, h]| {
            let canvas_x = pending.layer_canvas_extent.origin.x + x as i32;
            let canvas_y = pending.layer_canvas_extent.origin.y + y as i32;
            crate::coord::CanvasRect::from_xywh(canvas_x, canvas_y, w, h)
        }))
    }
}
