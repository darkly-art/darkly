//! On-demand async GPU→CPU pixel readback.
//!
//! Used for save/export, clipboard copy, flood fill seed reads, and color picking.

/// Alignment required by wgpu for bytes_per_row in buffer↔texture copies.
const COPY_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// A pending GPU→CPU readback.
///
/// Created by [`request_readback`], completed by [`poll`] or [`blocking_read`].
pub struct ReadbackRequest {
    buffer: wgpu::Buffer,
    height: u32,
    padded_row_bytes: u32,
    unpadded_row_bytes: u32,
}

/// Initiate a readback of a texture region.
///
/// Encodes a `copy_texture_to_buffer` command. The returned [`ReadbackRequest`]
/// can be polled or blocking-read after the encoder is submitted.
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
    }
}

impl ReadbackRequest {
    /// Non-blocking poll. Returns `Some(pixels)` if the readback is ready.
    pub fn poll(&self, device: &wgpu::Device) -> Option<Vec<u8>> {
        let slice = self.buffer.slice(..);

        // Start the map if not already started.
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        let _ = device.poll(wgpu::PollType::Poll);

        match rx.try_recv() {
            Ok(Ok(())) => {
                let data = self.extract_pixels(&slice);
                self.buffer.unmap();
                Some(data)
            }
            _ => None,
        }
    }

    /// Blocking read. Waits until the GPU is done and returns the pixel data.
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
            // No padding — fast path.
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
