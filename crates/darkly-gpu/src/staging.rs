use darkly_core::tile::{TileData, TILE_SIZE};

/// Handles CPU→GPU tile uploads using queue.write_texture.
pub struct StagingRing {
    _next: usize,
}

impl StagingRing {
    pub fn new() -> Self {
        StagingRing { _next: 0 }
    }

    /// Upload a single tile's pixel data to a target texture at tile coordinates.
    pub fn upload_tile(
        &mut self,
        queue: &wgpu::Queue,
        tile_data: &TileData,
        target: &wgpu::Texture,
        tile_x: i32,
        tile_y: i32,
    ) {
        let dst_x = tile_x as u32 * TILE_SIZE as u32;
        let dst_y = tile_y as u32 * TILE_SIZE as u32;

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
            &tile_data.pixels,
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
