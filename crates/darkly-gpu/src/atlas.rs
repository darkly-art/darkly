use darkly_core::tile::TILE_SIZE;

/// The internal texture format used for layer textures and accumulators.
pub const LAYER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Create a layer-sized texture padded to tile boundaries.
pub fn create_layer_texture(
    device: &wgpu::Device,
    canvas_width: u32,
    canvas_height: u32,
    label: &str,
) -> wgpu::Texture {
    let ts = TILE_SIZE as u32;
    let width = ((canvas_width + ts - 1) / ts) * ts;
    let height = ((canvas_height + ts - 1) / ts) * ts;

    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: LAYER_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}
