use crate::tile::TILE_SIZE;

/// GPU-side texture storage for a single raster layer.
/// One Rgba8Unorm texture per layer, sized to canvas dimensions padded to tile boundary.
pub struct LayerTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width_in_tiles: u32,
    pub height_in_tiles: u32,
}

impl LayerTexture {
    pub fn new(device: &wgpu::Device, canvas_width: u32, canvas_height: u32) -> Self {
        Self::with_format(device, canvas_width, canvas_height, wgpu::TextureFormat::Rgba8Unorm, "layer-texture")
    }

    /// Create an R8Unorm mask texture (single-channel, 4x less memory than Rgba8Unorm).
    pub fn new_mask(device: &wgpu::Device, canvas_width: u32, canvas_height: u32) -> Self {
        Self::with_format(device, canvas_width, canvas_height, wgpu::TextureFormat::R8Unorm, "mask-texture")
    }

    fn with_format(
        device: &wgpu::Device,
        canvas_width: u32,
        canvas_height: u32,
        format: wgpu::TextureFormat,
        label: &str,
    ) -> Self {
        let width_in_tiles = (canvas_width + TILE_SIZE as u32 - 1) / TILE_SIZE as u32;
        let height_in_tiles = (canvas_height + TILE_SIZE as u32 - 1) / TILE_SIZE as u32;
        let tex_width = width_in_tiles * TILE_SIZE as u32;
        let tex_height = height_in_tiles * TILE_SIZE as u32;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: tex_width,
                height: tex_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        LayerTexture {
            texture,
            view,
            width_in_tiles,
            height_in_tiles,
        }
    }
}
