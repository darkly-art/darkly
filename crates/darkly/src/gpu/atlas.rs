/// GPU-side texture storage for a single raster layer.
/// One Rgba8Unorm texture per layer, sized to exact canvas dimensions.
pub struct LayerTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl LayerTexture {
    /// RGBA layer texture — default fill is 0 (transparent), which is the GPU's init value.
    pub fn new(device: &wgpu::Device, canvas_width: u32, canvas_height: u32) -> Self {
        Self::with_format(device, None, canvas_width, canvas_height, wgpu::TextureFormat::Rgba8Unorm, "layer-texture")
    }

    /// R8Unorm mask texture — default fill is 255 (white = reveal all).
    pub fn new_mask(device: &wgpu::Device, queue: &wgpu::Queue, canvas_width: u32, canvas_height: u32) -> Self {
        Self::with_format(device, Some(queue), canvas_width, canvas_height, wgpu::TextureFormat::R8Unorm, "mask-texture")
    }

    fn with_format(
        device: &wgpu::Device,
        queue: Option<&wgpu::Queue>,
        canvas_width: u32,
        canvas_height: u32,
        format: wgpu::TextureFormat,
        label: &str,
    ) -> Self {
        let bpp = format.block_copy_size(None).unwrap_or(1) as u32;
        let fill_byte = match format {
            wgpu::TextureFormat::R8Unorm => 255u8, // white = reveal all
            _ => 0u8,                              // transparent
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: canvas_width,
                height: canvas_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        // Fill with non-zero default if needed (GPU textures init to 0).
        if fill_byte != 0 {
            if let Some(queue) = queue {
                let row_bytes = canvas_width * bpp;
                let data = vec![fill_byte; (row_bytes * canvas_height) as usize];
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes),
                        rows_per_image: Some(canvas_height),
                    },
                    wgpu::Extent3d { width: canvas_width, height: canvas_height, depth_or_array_layers: 1 },
                );
            }
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        LayerTexture {
            texture,
            view,
        }
    }
}
