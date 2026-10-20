//! Zero a sub-rect of any copyable texture by copying rows out of a
//! zero-filled buffer.
//!
//! A render-pass clear hits the whole attachment, and a compute clear
//! cannot reach a non-storage format such as `r8unorm`; a buffer copy is
//! format agnostic, needs no pipeline, and touches only the region. This is
//! the mechanism wgpu-core itself uses to clear textures it cannot render
//! to (`clear_texture_via_buffer_copies` in `wgpu-core/src/command/clear.rs`,
//! wgpu-core 29.0.3): a fixed zero buffer, rows per copy bounded by its
//! size, the row pitch aligned to `COPY_BYTES_PER_ROW_ALIGNMENT`.
//!
//! Credit: adapted from wgpu-core's `clear_texture_via_buffer_copies`,
//! by the gfx-rs authors, https://github.com/gfx-rs/wgpu/blob/trunk/wgpu-core/src/command/clear.rs

use crate::coord::LayerRect;

/// Size of the shared zero buffer: one megabyte holds at least 128 rows of
/// an 8 KB row pitch (a 2048 px wide `rg32float` rect).
pub const ZERO_BUFFER_SIZE: u64 = 1 << 20;

/// The shared zero source. Buffers are zero on creation (WebGPU spec), so
/// nothing is ever written to it.
pub fn create_zero_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("zero-fill"),
        size: ZERO_BUFFER_SIZE,
        usage: wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// Zero `rect` (texture-local) of `texture`, which must carry `COPY_DST`.
/// Chunked by rows so every copy's `bytes_per_row * rows` fits the zero
/// buffer. An empty rect is a no-op.
pub fn zero_fill_rect(
    encoder: &mut wgpu::CommandEncoder,
    zero: &wgpu::Buffer,
    texture: &wgpu::Texture,
    rect: LayerRect,
) {
    if rect.is_empty() {
        return;
    }
    let format = texture.format();
    let bytes_per_texel = format
        .block_copy_size(None)
        .unwrap_or_else(|| panic!("zero fill of a format without a copy size: {format:?}"));
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let bytes_per_row = (rect.width * bytes_per_texel).div_ceil(align) * align;
    let rows_per_copy = (ZERO_BUFFER_SIZE / u64::from(bytes_per_row)) as u32;
    assert!(
        rows_per_copy > 0,
        "zero fill row pitch {bytes_per_row} exceeds the zero buffer ({ZERO_BUFFER_SIZE} bytes)"
    );
    let mut y = 0;
    while y < rect.height {
        let rows = rows_per_copy.min(rect.height - y);
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: zero,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: rect.x0(),
                    y: rect.y0() + y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: rect.width,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        y += rows;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_utils::{readback_texture, test_device};

    /// Fill a `w x h` texture of `format` with a non-zero pattern, zero
    /// `rect`, and check every byte: zero inside, the pattern outside.
    fn fill_and_zero(format: wgpu::TextureFormat, w: u32, h: u32, rect: LayerRect) {
        let (device, queue) = test_device();
        let bpp = format.block_copy_size(None).unwrap();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("zero-fill-test"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let pattern: Vec<u8> = (0..(w * h * bpp)).map(|i| (i % 251) as u8 + 1).collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pattern,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * bpp),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let zero = create_zero_buffer(&device);
        let mut encoder = device.create_command_encoder(&Default::default());
        zero_fill_rect(&mut encoder, &zero, &texture, rect);
        queue.submit([encoder.finish()]);
        let out = readback_texture(&device, &queue, &texture, format, w, h);
        assert_eq!(out.len(), pattern.len());
        for y in 0..h {
            for x in 0..w {
                let inside = x >= rect.x0() && x < rect.x1() && y >= rect.y0() && y < rect.y1();
                for b in 0..bpp {
                    let i = ((y * w + x) * bpp + b) as usize;
                    let expected = if inside { 0 } else { pattern[i] };
                    assert_eq!(
                        out[i], expected,
                        "{format:?} byte {b} of ({x}, {y}), inside the rect: {inside}"
                    );
                }
            }
        }
    }

    /// Every format the stroke scratch and its channels use, over a rect
    /// whose row pitch is not a multiple of the copy alignment and whose
    /// height exceeds what one zero-buffer chunk holds (a 2000 px wide
    /// `rg32float` row is 16 KB, so the buffer holds 64 rows of it).
    #[test]
    fn zeroes_only_the_rect_in_every_scratch_format() {
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::R8Unorm,
            wgpu::TextureFormat::R32Uint,
            wgpu::TextureFormat::Rg32Float,
        ] {
            fill_and_zero(format, 2000, 100, LayerRect::from_xywh(37, 11, 1931, 85));
        }
    }

    #[test]
    fn empty_rect_is_a_no_op() {
        fill_and_zero(
            wgpu::TextureFormat::Rgba8Unorm,
            16,
            16,
            LayerRect::from_xywh(4, 4, 0, 8),
        );
    }
}
