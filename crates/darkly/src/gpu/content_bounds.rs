//! GPU-accelerated content bounds computation.
//!
//! A compute shader scans a texture and produces the tight bounding rect of
//! all non-transparent pixels using atomic min/max. The result is 16 bytes
//! (4× u32) read back asynchronously — no full-texture readback required.
//!
//! The compositor owns a [`ContentBoundsPass`] and exposes cached per-layer
//! bounds. A cached result records the [`Stamp`] it was computed under and is
//! compared against the live [`Revisions`] whenever it is read, so nothing has
//! to push an invalidation when the inputs move.

use super::bbox::{BboxReduction, PendingBbox};
use super::revisions::{Revisions, Tick};
use crate::layer::LayerId;
use std::collections::HashMap;

/// What a bounds result was computed from: the document state (which decides
/// the layer's extent) and that layer's own pixels.
///
/// The document half reproduces the coarse invalidation the pass had when
/// `mark_dirty` cleared every entry; narrowing it to the pixel tick alone is a
/// one-line change here and nowhere else.
#[derive(Copy, Clone, PartialEq, Eq)]
struct Stamp {
    document: Tick,
    node_pixels: Tick,
}

impl Stamp {
    fn current(revisions: &Revisions, layer_id: LayerId) -> Self {
        Stamp {
            document: revisions.document(),
            node_pixels: revisions.node_pixels(layer_id),
        }
    }
}

/// GPU compute pipeline + per-layer cache for content bounds.
pub struct ContentBoundsPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,

    /// Cached content bounds per layer: `[x, y, w, h]`, with the stamp they
    /// were computed under. `None` bounds is a resolved *empty* result, not a
    /// miss.
    cached: HashMap<LayerId, (Stamp, Option<[u32; 4]>)>,

    /// In-flight compute dispatches awaiting buffer mapping.
    pending: Vec<PendingBounds>,
}

struct PendingBounds {
    layer_id: LayerId,
    stamp: Stamp,
    bbox: PendingBbox,
}

/// Uniform buffer layout matching the shader's `Params` struct.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    width: u32,
    height: u32,
    use_r_channel: u32,
    _pad: u32,
}

impl ContentBoundsPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("content-bounds-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../shaders/content_bounds.wgsl").into(),
            ),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("content-bounds-bgl"),
            entries: &[
                // binding 0: source texture
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
                // binding 1: atomic bounds storage buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 2: params uniform
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
            label: Some("content-bounds-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("content-bounds-pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        ContentBoundsPass {
            pipeline,
            bind_group_layout,
            cached: HashMap::new(),
            pending: Vec::new(),
        }
    }

    /// The entry for a layer, if one was computed under the current stamp.
    /// The comparison happens here, so no consumer can read a stale result by
    /// forgetting to check one.
    fn current_entry(&self, revisions: &Revisions, layer_id: LayerId) -> Option<&Option<[u32; 4]>> {
        let (stamp, bounds) = self.cached.get(&layer_id)?;
        (*stamp == Stamp::current(revisions, layer_id)).then_some(bounds)
    }

    /// Return current content bounds for a layer: `[x, y, w, h]`. `None` if
    /// not yet computed, stale, or resolved empty.
    pub fn get(&self, revisions: &Revisions, layer_id: LayerId) -> Option<[u32; 4]> {
        self.current_entry(revisions, layer_id).copied().flatten()
    }

    /// True once the current stamp has resolved, including an empty result.
    /// Distinguishes "no content" from "not computed yet", so a caller does
    /// not requeue a terminal computation forever.
    pub fn is_resolved(&self, revisions: &Revisions, layer_id: LayerId) -> bool {
        self.current_entry(revisions, layer_id).is_some()
    }

    /// True if a bounds computation for the layer's current stamp is in
    /// flight. A dispatch for a superseded stamp does not count — its result
    /// will be discarded, so a fresh one is still warranted.
    pub fn is_pending(&self, revisions: &Revisions, layer_id: LayerId) -> bool {
        let stamp = Stamp::current(revisions, layer_id);
        self.pending
            .iter()
            .any(|p| p.layer_id == layer_id && p.stamp == stamp)
    }

    /// Remove all state for a layer (when it's deleted).
    pub fn remove_layer(&mut self, layer_id: LayerId) {
        self.cached.remove(&layer_id);
    }

    /// True if any results are pending.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Dispatch a compute shader to calculate content bounds for a node's
    /// texture.
    ///
    /// `r_channel` selects which texel channel is treated as coverage: alpha
    /// for RGBA targets, red for R8 targets. Driven by the texture's format,
    /// not by node kind.
    ///
    /// Results arrive asynchronously — call [`poll`] each frame.
    #[allow(clippy::too_many_arguments)]
    pub fn request(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        revisions: &Revisions,
        texture_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        r_channel: bool,
        layer_id: LayerId,
    ) {
        let stamp = Stamp::current(revisions, layer_id);

        // Don't queue duplicate requests for the same stamp.
        if self
            .pending
            .iter()
            .any(|p| p.layer_id == layer_id && p.stamp == stamp)
        {
            return;
        }

        // Params uniform.
        let params = Params {
            width,
            height,
            use_r_channel: if r_channel { 1 } else { 0 },
            _pad: 0,
        };
        let param_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("content-bounds-params"),
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
                    label: Some("content-bounds-bg"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(texture_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: storage.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: param_buf.as_entire_binding(),
                        },
                    ],
                })
            },
            width,
            height,
        );

        self.pending.push(PendingBounds {
            layer_id,
            stamp,
            bbox,
        });
    }

    /// Poll pending computations. Call once per frame.
    ///
    /// Returns the list of layer IDs whose bounds just became available.
    pub fn poll(&mut self, device: &wgpu::Device, revisions: &Revisions) -> Vec<LayerId> {
        let mut completed = Vec::new();
        let mut i = 0;
        while i < self.pending.len() {
            match self.pending[i].bbox.poll(device) {
                Some(result) => {
                    let p = self.pending.swap_remove(i);
                    if p.stamp == Stamp::current(revisions, p.layer_id) {
                        // Store even an empty result, so callers do not
                        // requeue the same terminal computation indefinitely.
                        self.cached.insert(p.layer_id, (p.stamp, result));
                        completed.push(p.layer_id);
                    }
                    // Stale result (its inputs moved since dispatch) → drop it.
                    // Don't increment i — swap_remove moved the last element here.
                }
                None => {
                    i += 1;
                }
            }
        }
        completed
    }
}
