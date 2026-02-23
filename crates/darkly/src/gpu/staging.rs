use crate::tile::{TILE_SIZE, TileData};

/// Handles CPU→GPU tile uploads via queue.write_texture.
pub struct StagingRing;

impl StagingRing {
    pub fn new() -> Self {
        StagingRing
    }

    /// Upload a single tile to the target texture at the given tile coordinates.
    /// Uses queue.write_texture — WebGPU handles the CPU→GPU copy efficiently.
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
            &tile_data.0,
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
