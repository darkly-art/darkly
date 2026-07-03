/// Clear a texture view to fully-transparent black via an empty render
/// pass. WebGPU has no standalone "clear texture" command — clears are
/// expressed as the load op of a render pass. This wraps the empty-pass
/// boilerplate so callers that just need a clear can do it in one call.
pub fn clear_view_transparent(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    label: &str,
) {
    let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
}

/// Create a 2D texture and its default view in one call. Wraps the
/// `TextureDescriptor` + `create_view` pair that recurs at every transient
/// (staging / scratch / crop) texture allocation.
pub fn create_texture_with_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    label: &str,
    usage: wgpu::TextureUsages,
) -> (wgpu::Texture, wgpu::TextureView) {
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
        usage,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Blit a `width`×`height` sub-region between two textures at the given
/// source and destination origins. Wraps the `TexelCopyTextureInfo` +
/// `Extent3d` boilerplate of an `encoder.copy_texture_to_texture` region copy.
pub fn blit_region(
    encoder: &mut wgpu::CommandEncoder,
    src: &wgpu::Texture,
    src_origin: (u32, u32),
    dst: &wgpu::Texture,
    dst_origin: (u32, u32),
    width: u32,
    height: u32,
) {
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: src,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: src_origin.0,
                y: src_origin.1,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: dst,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: dst_origin.0,
                y: dst_origin.1,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

pub mod apply_mask;
pub mod atlas;
pub mod blend;
pub mod blend_mode;
pub mod blend_modes;
pub mod canvas_lib;
pub mod compositor;
pub mod content_bounds;
pub mod context;
pub mod diff_rect;
pub mod effect;
pub mod filter;
pub mod filters;
pub mod floating_preview;
pub mod flood_fill;
pub mod histogram;
pub mod lut_filter;
pub mod ortho_transform;
pub mod overlay;
pub mod paint_target;
pub mod params;
pub mod preview;
pub mod readback;
pub mod region_store;
pub mod rescale;
pub mod selection;
pub mod straight_composite;
#[cfg(any(test, feature = "testing"))]
pub mod test_utils;
pub mod texture_registry;
pub mod transform;
pub mod vector_renderer;
pub mod veil;
pub mod veil_chain;
pub mod veil_preview;
pub mod veils;
pub mod video_stream_void;
pub mod view;
pub mod void;
pub mod void_preview;
pub mod voids;
