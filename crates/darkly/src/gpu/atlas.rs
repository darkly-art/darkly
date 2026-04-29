/// GPU-side texture storage for a single raster layer.
/// One Rgba8Unorm texture per layer, sized to the layer's pixel bounds —
/// which default to canvas dimensions but may be larger when content
/// extends past the canvas (e.g. paste of an oversized image).
pub struct LayerTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// Texture dimensions in pixels. Mirrors the size used at allocation
    /// so callers don't need to reach into the wgpu::Texture descriptor.
    pub width: u32,
    pub height: u32,
    /// Canvas-space offset of the texture's (0,0) pixel. Default (0,0) for
    /// canvas-aligned layers; non-zero for layers whose bounds extend past
    /// or are placed inside canvas (e.g. paste of an oversized image).
    pub offset_x: i32,
    pub offset_y: i32,
}

impl LayerTexture {
    /// RGBA layer texture — default fill is 0 (transparent), which is the GPU's init value.
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self::with_format(
            device,
            None,
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
            "layer-texture",
        )
    }

    /// RGBA layer texture sized + positioned to match the given bounds.
    /// Equivalent to `new` followed by setting `offset_x`/`offset_y`.
    pub fn with_bounds(device: &wgpu::Device, bounds: crate::layer::LayerBounds) -> Self {
        let mut t = Self::new(device, bounds.width, bounds.height);
        t.offset_x = bounds.offset_x;
        t.offset_y = bounds.offset_y;
        t
    }

    /// R8Unorm mask texture — default fill is 255 (white = reveal all).
    pub fn new_mask(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) -> Self {
        Self::with_format(
            device,
            Some(queue),
            width,
            height,
            wgpu::TextureFormat::R8Unorm,
            "mask-texture",
        )
    }

    fn with_format(
        device: &wgpu::Device,
        queue: Option<&wgpu::Queue>,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        label: &str,
    ) -> Self {
        let bpp = format.block_copy_size(None).unwrap_or(1);
        let fill_byte = match format {
            wgpu::TextureFormat::R8Unorm => 255u8, // white = reveal all
            _ => 0u8,                              // transparent
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
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
                let row_bytes = width * bpp;
                let data = vec![fill_byte; (row_bytes * height) as usize];
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
                        rows_per_image: Some(height),
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        LayerTexture {
            texture,
            view,
            width,
            height,
            offset_x: 0,
            offset_y: 0,
        }
    }
}
