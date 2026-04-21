//! GPU-accelerated diff bounding rect computation.
//!
//! A compute shader compares two textures (pre-stroke scratch vs post-stroke
//! canvas) and produces the tight bounding rect of all differing pixels using
//! atomic min/max. Used at stroke end to determine the exact undo region
//! without hand-tracking dab positions.

/// Initial values for the atomic bounds buffer: min = MAX, max = 0.
/// If min_x > max_x after dispatch, the textures are identical.
const BOUNDS_INIT: [u32; 4] = [u32::MAX, u32::MAX, 0, 0];

pub struct DiffRectPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    pending: Option<PendingDiff>,
}

struct PendingDiff {
    staging: wgpu::Buffer,
    rx: Option<std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
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
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/diff_rect.wgsl").into(),
            ),
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
            bind_group_layouts: &[&bind_group_layout],
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
    /// post-stroke canvas. Results arrive asynchronously via [`poll`].
    pub fn request(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scratch_view: &wgpu::TextureView,
        current_view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        // Storage buffer for atomic results (16 bytes).
        let storage_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("diff-rect-storage"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        {
            let mut mapping = storage_buf.slice(..).get_mapped_range_mut();
            mapping.copy_from_slice(bytemuck::bytes_of(&BOUNDS_INIT));
        }
        storage_buf.unmap();

        // Staging buffer for CPU readback (16 bytes).
        let staging_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("diff-rect-staging"),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Params uniform.
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

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                    resource: storage_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: param_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("diff-rect-compute"),
        });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("diff-rect"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, Some(&bind_group), &[]);
            let wg_x = width.div_ceil(16);
            let wg_y = height.div_ceil(16);
            pass.dispatch_workgroups(wg_x, wg_y, 1);
        }

        // Copy storage -> staging for CPU readback.
        encoder.copy_buffer_to_buffer(&storage_buf, 0, &staging_buf, 0, 16);
        queue.submit([encoder.finish()]);

        self.pending = Some(PendingDiff {
            staging: staging_buf,
            rx: None,
        });
    }

    /// True if a diff result is pending.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Poll for the diff result. Returns `Some([x, y, w, h])` when ready,
    /// `Some([0,0,0,0])` if the textures are identical, or `None` if still pending.
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<Option<[u32; 4]>> {
        let pending = self.pending.as_mut()?;

        // Begin mapping if not started.
        if pending.rx.is_none() {
            let slice = pending.staging.slice(..);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            slice.map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
            pending.rx = Some(rx);
        }

        let _ = device.poll(wgpu::PollType::Poll);

        let ready = pending.rx.as_ref().unwrap().try_recv().ok();
        match ready {
            Some(Ok(())) => {
                let p = self.pending.take().unwrap();
                let slice = p.staging.slice(..);
                let mapped = slice.get_mapped_range();
                let raw: [u32; 4] = *bytemuck::from_bytes(&mapped[..16]);
                drop(mapped);
                p.staging.unmap();

                let [min_x, min_y, max_x, max_y] = raw;
                if min_x <= max_x && min_y <= max_y {
                    // +1 because max is inclusive pixel coordinate.
                    Some(Some([min_x, min_y, max_x - min_x + 1, max_y - min_y + 1]))
                } else {
                    // Textures are identical — no diff.
                    Some(None)
                }
            }
            Some(Err(e)) => {
                log::error!("diff rect buffer mapping failed: {e}");
                self.pending = None;
                Some(None)
            }
            None => None,
        }
    }
}
