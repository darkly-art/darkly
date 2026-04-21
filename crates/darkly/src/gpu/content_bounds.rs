//! GPU-accelerated content bounds computation.
//!
//! A compute shader scans a texture and produces the tight bounding rect of
//! all non-transparent pixels using atomic min/max. The result is 16 bytes
//! (4× u32) read back asynchronously — no full-texture readback required.
//!
//! The compositor owns a [`ContentBoundsPass`] and exposes cached per-layer
//! bounds. Bounds are invalidated on [`mark_dirty`](super::compositor::Compositor::mark_dirty)
//! and recomputed lazily when a consumer requests them.

use crate::layer::LayerId;
use std::collections::HashMap;

/// Initial values for the atomic bounds buffer: min = MAX, max = 0.
/// If min_x > max_x after dispatch, the texture is fully transparent.
const BOUNDS_INIT: [u32; 4] = [u32::MAX, u32::MAX, 0, 0];

/// GPU compute pipeline + per-layer cache for content bounds.
pub struct ContentBoundsPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,

    /// Cached content bounds per layer: `[x, y, w, h]`.
    cached: HashMap<LayerId, [u32; 4]>,

    /// Generation counter per layer — incremented on invalidation.
    /// Pending results whose generation doesn't match are discarded.
    generation: HashMap<LayerId, u64>,

    /// In-flight compute dispatches awaiting buffer mapping.
    pending: Vec<PendingBounds>,
}

struct PendingBounds {
    layer_id: LayerId,
    gen: u64,
    staging: wgpu::Buffer,
    rx: Option<std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
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
                include_str!("../../../../shaders/content_bounds.wgsl").into(),
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
            bind_group_layouts: &[&bind_group_layout],
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
            generation: HashMap::new(),
            pending: Vec::new(),
        }
    }

    /// Return cached content bounds for a layer, if available.
    /// Returns `[x, y, w, h]` or `None` if not yet computed or invalidated.
    pub fn get(&self, layer_id: LayerId) -> Option<[u32; 4]> {
        self.cached.get(&layer_id).copied()
    }

    /// True if a bounds computation is in flight for this layer.
    pub fn is_pending(&self, layer_id: LayerId) -> bool {
        let gen = self.generation.get(&layer_id).copied().unwrap_or(0);
        self.pending
            .iter()
            .any(|p| p.layer_id == layer_id && p.gen == gen)
    }

    /// Invalidate cached bounds for a specific layer.
    pub fn invalidate(&mut self, layer_id: LayerId) {
        self.cached.remove(&layer_id);
        *self.generation.entry(layer_id).or_insert(0) += 1;
    }

    /// Invalidate cached bounds for all layers.
    pub fn invalidate_all(&mut self) {
        self.cached.clear();
        for gen in self.generation.values_mut() {
            *gen += 1;
        }
    }

    /// Remove all state for a layer (when it's deleted).
    pub fn remove_layer(&mut self, layer_id: LayerId) {
        self.cached.remove(&layer_id);
        self.generation.remove(&layer_id);
    }

    /// True if any results are pending.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Dispatch a compute shader to calculate content bounds for a layer.
    ///
    /// `is_mask` controls which channel is checked: alpha for RGBA layers,
    /// red for R8 masks.
    ///
    /// Results arrive asynchronously — call [`poll`] each frame.
    pub fn request(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        is_mask: bool,
        layer_id: LayerId,
    ) {
        let gen = self.generation.get(&layer_id).copied().unwrap_or(0);

        // Don't queue duplicate requests for the same generation.
        if self
            .pending
            .iter()
            .any(|p| p.layer_id == layer_id && p.gen == gen)
        {
            return;
        }

        // Storage buffer for atomic results (16 bytes).
        let storage_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("content-bounds-storage"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        // Initialize: min = MAX, max = 0.
        {
            let mut mapping = storage_buf.slice(..).get_mapped_range_mut();
            mapping.copy_from_slice(bytemuck::bytes_of(&BOUNDS_INIT));
        }
        storage_buf.unmap();

        // Staging buffer for CPU readback (16 bytes).
        let staging_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("content-bounds-staging"),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Params uniform.
        let params = Params {
            width,
            height,
            use_r_channel: if is_mask { 1 } else { 0 },
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

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("content-bounds-bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: storage_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: param_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("content-bounds-compute"),
        });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("content-bounds"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, Some(&bind_group), &[]);
            let wg_x = width.div_ceil(16);
            let wg_y = height.div_ceil(16);
            pass.dispatch_workgroups(wg_x, wg_y, 1);
        }

        // Copy storage → staging for CPU readback.
        encoder.copy_buffer_to_buffer(&storage_buf, 0, &staging_buf, 0, 16);
        queue.submit([encoder.finish()]);

        self.pending.push(PendingBounds {
            layer_id,
            gen,
            staging: staging_buf,
            rx: None,
        });
    }

    /// Poll pending computations. Call once per frame.
    ///
    /// Returns the list of layer IDs whose bounds just became available.
    pub fn poll(&mut self, device: &wgpu::Device) -> Vec<LayerId> {
        // Begin mapping for newly submitted requests.
        for p in &mut self.pending {
            if p.rx.is_none() {
                let slice = p.staging.slice(..);
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                slice.map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
                p.rx = Some(rx);
            }
        }

        if !self.pending.is_empty() {
            let _ = device.poll(wgpu::PollType::Poll);
        }

        let mut completed = Vec::new();
        let mut i = 0;
        while i < self.pending.len() {
            let ready = self.pending[i]
                .rx
                .as_ref()
                .and_then(|rx| rx.try_recv().ok());

            match ready {
                Some(Ok(())) => {
                    let p = self.pending.swap_remove(i);
                    let current_gen = self.generation.get(&p.layer_id).copied().unwrap_or(0);

                    if p.gen == current_gen {
                        // Read the 4× u32 result.
                        let slice = p.staging.slice(..);
                        let mapped = slice.get_mapped_range();
                        let raw: [u32; 4] = *bytemuck::from_bytes(&mapped[..16]);
                        drop(mapped);
                        p.staging.unmap();

                        let [min_x, min_y, max_x, max_y] = raw;
                        if min_x <= max_x && min_y <= max_y {
                            let bounds = [min_x, min_y, max_x - min_x + 1, max_y - min_y + 1];
                            self.cached.insert(p.layer_id, bounds);
                        }
                        // If min_x > max_x: fully transparent — no cached entry.

                        completed.push(p.layer_id);
                    } else {
                        // Stale result — generation changed since dispatch.
                        p.staging.unmap();
                    }
                }
                Some(Err(e)) => {
                    log::error!("content bounds buffer mapping failed: {e}");
                    self.pending.swap_remove(i);
                }
                None => {
                    i += 1;
                }
            }
        }
        completed
    }
}
