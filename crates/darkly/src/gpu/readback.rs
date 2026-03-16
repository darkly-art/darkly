//! On-demand async GPU→CPU pixel readback.
//!
//! Used for save/export, clipboard copy, flood fill seed reads, and color picking.
//!
//! On WebGPU (WASM), `map_async` resolves via a JS Promise — the browser event
//! loop must run before the callback fires.  `blocking_read` therefore spins
//! `device.poll(Wait)` which never completes in single-threaded WASM, freezing
//! the tab at 100% CPU.
//!
//! The correct pattern: call [`begin_mapping`] after `queue.submit`, then poll
//! with [`poll`] each frame until the data arrives.  `blocking_read` is kept
//! only for headless / native test code.

/// Alignment required by wgpu for bytes_per_row in buffer↔texture copies.
const COPY_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// A pending GPU→CPU readback.
///
/// Created by [`request_readback`], which encodes the copy command.
/// After submitting the encoder, call [`begin_mapping`] to start the async map,
/// then [`poll`] each frame until data is available.
pub struct ReadbackRequest {
    buffer: wgpu::Buffer,
    height: u32,
    padded_row_bytes: u32,
    unpadded_row_bytes: u32,
    /// Receiver for the map_async callback.  `None` until `begin_mapping()`.
    rx: Option<std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

/// Initiate a readback of a texture region.
///
/// Encodes a `copy_texture_to_buffer` command into `encoder`.
/// After this, the caller must:
/// 1. `queue.submit([encoder.finish()])`
/// 2. `request.begin_mapping()`
/// 3. Poll with `request.poll(device)` each frame.
pub fn request_readback(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    rect: [u32; 4],
) -> ReadbackRequest {
    let [x, y, w, h] = rect;
    let bpp = format.block_copy_size(None).unwrap_or(1) as u32;
    let unpadded_row_bytes = w * bpp;
    let padded_row_bytes = (unpadded_row_bytes + COPY_ROW_ALIGNMENT - 1)
        / COPY_ROW_ALIGNMENT
        * COPY_ROW_ALIGNMENT;
    let buffer_size = padded_row_bytes as u64 * h as u64;

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row_bytes),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );

    ReadbackRequest {
        buffer,
        height: h,
        padded_row_bytes,
        unpadded_row_bytes,
        rx: None,
    }
}

impl ReadbackRequest {
    /// Start the async buffer mapping.
    ///
    /// **Must** be called after `queue.submit()` — the copy command must be
    /// submitted before the map can complete.  Call this exactly once.
    pub fn begin_mapping(&mut self) {
        let slice = self.buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.rx = Some(rx);
    }

    /// Non-blocking poll.  Returns `Some(pixels)` when the readback is ready.
    ///
    /// Calls `device.poll(Poll)` to give the backend a chance to process
    /// callbacks (needed on native; on WebGPU the browser resolves the
    /// Promise between frames).
    pub fn poll(&self, device: &wgpu::Device) -> Option<Vec<u8>> {
        let rx = self.rx.as_ref()?;

        // Nudge the backend so it can fire ready callbacks (native).
        let _ = device.poll(wgpu::PollType::Poll);

        match rx.try_recv() {
            Ok(Ok(())) => {
                let slice = self.buffer.slice(..);
                let data = self.extract_pixels(&slice);
                self.buffer.unmap();
                Some(data)
            }
            Ok(Err(e)) => {
                log::error!("readback buffer mapping failed: {e}");
                None
            }
            Err(_) => None, // not ready yet
        }
    }

    /// Blocking read — **native / test only**.
    ///
    /// On WebGPU/WASM this will spin at 100% CPU forever because `map_async`
    /// resolves via a JS Promise that requires the event loop.  Use the
    /// async `begin_mapping` + `poll` path for production code.
    pub fn blocking_read(&self, device: &wgpu::Device) -> Vec<u8> {
        let slice = self.buffer.slice(..);

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv()
            .expect("map callback never fired")
            .expect("buffer mapping failed");

        let data = self.extract_pixels(&slice);
        self.buffer.unmap();
        data
    }

    /// Strip row padding and return tightly-packed pixel data.
    fn extract_pixels(&self, slice: &wgpu::BufferSlice) -> Vec<u8> {
        let mapped = slice.get_mapped_range();
        let unpadded = self.unpadded_row_bytes as usize;
        let padded = self.padded_row_bytes as usize;

        if unpadded == padded {
            return mapped.to_vec();
        }

        let mut out = Vec::with_capacity(unpadded * self.height as usize);
        for row in 0..self.height as usize {
            let start = row * padded;
            out.extend_from_slice(&mapped[start..start + unpadded]);
        }
        out
    }
}
