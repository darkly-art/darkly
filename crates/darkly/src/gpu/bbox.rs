//! Shared async "bounding box of interesting pixels" GPU reduction.
//!
//! A compute shader visits every texel, tests a caller-defined predicate, and
//! folds the coordinates of any matching texel into a 16-byte atomic min/max
//! buffer. After dispatch the buffer holds `[min_x, min_y, max_x, max_y]`; if
//! `min_x > max_x` no texel matched. The result is read back asynchronously —
//! no full-texture readback required.
//!
//! This module owns the machinery that every bbox caller shares: the atomic
//! storage buffer + its `BOUNDS_INIT` seed, the staging buffer, the
//! `div_ceil(16)` dispatch tiling, the `map_async` → poll → decode round-trip,
//! and the "`min_x > max_x` ⇒ empty" convention. What stays per-caller is the
//! *predicate* (a WGSL shader + bind-group layout) and the *post-processing*
//! of the returned rect. [`DiffRectPass`](super::diff_rect::DiffRectPass) and
//! [`ContentBoundsPass`](super::content_bounds::ContentBoundsPass) are thin
//! wrappers over this primitive; the CPU-side twin lives in
//! [`changed_pixels_bbox`](crate::engine::rendering::changed_pixels_bbox).

/// Initial values for the atomic bounds buffer: min = MAX, max = 0.
/// After dispatch, `min_x > max_x` means no texel matched the predicate.
const BOUNDS_INIT: [u32; 4] = [u32::MAX, u32::MAX, 0, 0];

/// Size of the atomic bounds / staging buffers: 4 × u32.
const BOUNDS_BYTES: u64 = 16;

/// The shared bbox-reduction machinery. Stateless — it only bundles the
/// [`dispatch`](Self::dispatch) constructor for an in-flight [`PendingBbox`].
pub struct BboxReduction;

impl BboxReduction {
    /// Seed the atomic storage buffer, let the caller build its bind group
    /// over it (binding the caller's input textures + predicate params
    /// alongside), dispatch `pipeline` over `width × height`, copy the result
    /// into a staging buffer, and submit. Poll the returned handle with
    /// [`PendingBbox::poll`].
    ///
    /// `make_bind_group` receives the freshly-created storage buffer so the
    /// caller can bind it at whichever binding index its shader expects; the
    /// caller owns everything else in the bind group (input textures, the
    /// params uniform). The bind group retains its resources, so any params
    /// buffer the closure creates may be dropped once it returns.
    pub fn dispatch(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &wgpu::ComputePipeline,
        make_bind_group: impl FnOnce(&wgpu::Buffer) -> wgpu::BindGroup,
        width: u32,
        height: u32,
    ) -> PendingBbox {
        let storage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bbox-storage"),
            size: BOUNDS_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        {
            let mut mapping = storage.slice(..).get_mapped_range_mut();
            mapping.copy_from_slice(bytemuck::bytes_of(&BOUNDS_INIT));
        }
        storage.unmap();

        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bbox-staging"),
            size: BOUNDS_BYTES,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = make_bind_group(&storage);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bbox-reduction"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("bbox-reduction"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, Some(&bind_group), &[]);
            pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
        }
        encoder.copy_buffer_to_buffer(&storage, 0, &staging, 0, BOUNDS_BYTES);
        queue.submit([encoder.finish()]);

        PendingBbox { staging, rx: None }
    }
}

/// An in-flight bbox reduction awaiting its async buffer mapping.
pub struct PendingBbox {
    staging: wgpu::Buffer,
    rx: Option<std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

impl PendingBbox {
    /// Poll for the reduced bbox.
    ///
    /// - `None` — still pending; call again next frame.
    /// - `Some(None)` — no texel matched the predicate (empty result).
    /// - `Some(Some([x, y, w, h]))` — the tight bounding box, converted from
    ///   the shader's inclusive `[min, max]` to origin + size (`w = max - min
    ///   + 1`). Coordinates are texel-local; callers translate as needed.
    ///
    /// Begins the async mapping on the first call, then nudges native backends
    /// with a non-blocking `device.poll(Poll)` — never a blocking wait, so this
    /// is safe on WebGPU/WASM.
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<Option<[u32; 4]>> {
        if self.rx.is_none() {
            let slice = self.staging.slice(..);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            slice.map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
            self.rx = Some(rx);
        }

        let _ = device.poll(wgpu::PollType::Poll);

        match self.rx.as_ref().unwrap().try_recv().ok() {
            Some(Ok(())) => {
                let slice = self.staging.slice(..);
                let mapped = slice.get_mapped_range();
                let raw: [u32; 4] = *bytemuck::from_bytes(&mapped[..BOUNDS_BYTES as usize]);
                drop(mapped);
                self.staging.unmap();

                let [min_x, min_y, max_x, max_y] = raw;
                Some(if min_x <= max_x && min_y <= max_y {
                    // +1 because max is an inclusive pixel coordinate.
                    Some([min_x, min_y, max_x - min_x + 1, max_y - min_y + 1])
                } else {
                    None
                })
            }
            Some(Err(e)) => {
                log::error!("bbox reduction buffer mapping failed: {e}");
                Some(None)
            }
            None => None,
        }
    }
}
