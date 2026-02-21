use darkly_core::tile::{TILE_BYTES, TILE_SIZE, TileData};

/// Ring buffer of staging buffers for CPU→GPU tile uploads.
/// Avoids allocating new buffers each frame (P1).
pub struct StagingRing {
    buffers: Vec<wgpu::Buffer>,
    next: usize,
}

impl StagingRing {
    pub fn new(device: &wgpu::Device, count: usize) -> Self {
        let buffers = (0..count)
            .map(|i| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("staging-{i}")),
                    size: TILE_BYTES as u64,
                    usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::MAP_WRITE,
                    mapped_at_creation: false,
                })
            })
            .collect();

        StagingRing { buffers, next: 0 }
    }

    /// Upload a single tile to the target texture at the given tile coordinates.
    /// Uses queue.write_texture for simplicity — no staging buffer mapping needed
    /// since WebGPU's write_texture handles the CPU→GPU copy efficiently.
    pub fn upload_tile(
        &mut self,
        queue: &wgpu::Queue,
        tile_data: &TileData,
        target: &wgpu::Texture,
        tile_x: u32,
        tile_y: u32,
    ) {
        let dst_x = tile_x * TILE_SIZE as u32;
        let dst_y = tile_y * TILE_SIZE as u32;

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: target,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: dst_x,
                    y: dst_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::bytes_of(&tile_data.0),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(TILE_SIZE as u32 * 4),
                rows_per_image: Some(TILE_SIZE as u32),
            },
            wgpu::Extent3d {
                width: TILE_SIZE as u32,
                height: TILE_SIZE as u32,
                depth_or_array_layers: 1,
            },
        );
    }
}
