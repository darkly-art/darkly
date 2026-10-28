//! Shared async GPU reduction into a small `u32` buffer.
//!
//! A compute shader visits every texel of a `width × height` grid and folds
//! what it finds into a storage buffer of atomics seeded by the caller. The
//! buffer is copied to a `MAP_READ` staging buffer and read back
//! asynchronously, so the caller gets a few words instead of a full-texture
//! readback.
//!
//! This module owns the machinery every reduction shares: seeding the storage
//! buffer, the staging buffer, the `div_ceil(16)` dispatch tiling, and the
//! `map_async` → poll round-trip. A reduction whose result feeds another GPU
//! pass rather than the CPU uses the first and third alone
//! ([`seeded_storage`], [`encode_over_grid`]) and never leaves the GPU, as
//! [`TipStretch`](super::tip_stretch::TipStretch) does. What stays per-caller is the shader (with
//! its bind-group layout) and the meaning of the words it produces. The most
//! common shape, a tight bounding box via atomic min/max, has its seed and
//! decoding here ([`BOUNDS_INIT`], [`decode_bounds`]): both
//! [`DiffRectPass`](super::diff_rect::DiffRectPass) and
//! [`ContentBoundsPass`](super::content_bounds::ContentBoundsPass) start their
//! result with it. The CPU-side twin of the bounds reduction is
//! [`changed_pixels_bbox`](crate::engine::rendering::changed_pixels_bbox).

/// Seed for a bounding-box reduction's four atomics `[min_x, min_y, max_x,
/// max_y]`: min = MAX, max = 0, so after dispatch `min_x > max_x` means no
/// texel matched.
pub const BOUNDS_INIT: [u32; 4] = [u32::MAX, u32::MAX, 0, 0];

/// Decode a bounding-box reduction's four words into `[x, y, w, h]`,
/// converting the shader's inclusive `[min, max]` to origin + size. `None`
/// when no texel matched. Coordinates are texel-local; callers translate.
pub fn decode_bounds([min_x, min_y, max_x, max_y]: [u32; 4]) -> Option<[u32; 4]> {
    // +1 because max is an inclusive pixel coordinate.
    (min_x <= max_x && min_y <= max_y).then(|| [min_x, min_y, max_x - min_x + 1, max_y - min_y + 1])
}

/// A storage buffer of atomics holding `seed`, for a reduction to fold into.
/// Usable as `COPY_SRC` so a reduction read on the CPU can stage it.
pub fn seeded_storage(device: &wgpu::Device, seed: &[u32]) -> wgpu::Buffer {
    let storage = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("reduction-storage"),
        size: std::mem::size_of_val(seed) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: true,
    });
    storage
        .slice(..)
        .get_mapped_range_mut()
        .copy_from_slice(bytemuck::cast_slice(seed));
    storage.unmap();
    storage
}

/// Record one dispatch of `pipeline` over a `width × height` grid of texels,
/// one thread each, in the 16x16 workgroups every reduction shader declares.
pub fn encode_over_grid(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    width: u32,
    height: u32,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("reduction"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, Some(bind_group), &[]);
    pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
}

/// An in-flight reduction awaiting its async buffer mapping.
pub struct PendingReduction {
    staging: wgpu::Buffer,
    rx: Option<std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

impl PendingReduction {
    /// Seed a storage buffer with `seed`, let the caller build its bind group
    /// over it (binding the caller's input textures and params alongside),
    /// dispatch `pipeline` over `width × height` in 16x16 workgroups, copy the
    /// result into a staging buffer, and submit. Poll the returned handle with
    /// [`poll`](Self::poll).
    ///
    /// `make_bind_group` receives the freshly-created storage buffer so the
    /// caller can bind it at whichever binding index its shader expects. The
    /// bind group retains its resources, so any params buffer the closure
    /// creates may be dropped once it returns.
    pub fn dispatch(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &wgpu::ComputePipeline,
        make_bind_group: impl FnOnce(&wgpu::Buffer) -> wgpu::BindGroup,
        seed: &[u32],
        width: u32,
        height: u32,
    ) -> Self {
        let storage = seeded_storage(device, seed);
        let size = storage.size();
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reduction-staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = make_bind_group(&storage);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("reduction"),
        });
        encode_over_grid(&mut encoder, pipeline, &bind_group, width, height);
        encoder.copy_buffer_to_buffer(&storage, 0, &staging, 0, size);
        queue.submit([encoder.finish()]);

        PendingReduction { staging, rx: None }
    }

    /// Poll for the reduced words.
    ///
    /// - `None` - still pending; call again next frame.
    /// - `Some(None)` - the buffer mapping failed (logged).
    /// - `Some(Some(words))` - the storage buffer's final contents.
    ///
    /// Begins the async mapping on the first call, then nudges native backends
    /// with a non-blocking `device.poll(Poll)`, never a blocking wait, so this
    /// is safe on WebGPU/WASM.
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<Option<Vec<u32>>> {
        let rx = self.rx.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            self.staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
            rx
        });

        let _ = device.poll(wgpu::PollType::Poll);

        match rx.try_recv().ok()? {
            Ok(()) => {
                let words =
                    bytemuck::cast_slice(&self.staging.slice(..).get_mapped_range()).to_vec();
                self.staging.unmap();
                Some(Some(words))
            }
            Err(e) => {
                log::error!("reduction buffer mapping failed: {e}");
                Some(None)
            }
        }
    }
}
