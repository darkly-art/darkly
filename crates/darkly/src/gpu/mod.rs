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
pub mod baked_source_cache;
pub mod bbox;
pub mod black_and_white;
pub mod blend;
pub mod blend_mode;
pub mod blend_modes;
pub mod canvas_lib;
pub mod compositor;
pub mod content_bounds;
pub mod context;
pub mod diff_rect;
pub mod effect;
pub mod effect_scaling;
pub mod effects;
pub mod floating_preview;
pub mod flood_fill;
pub mod hash;
pub mod histogram;
pub mod layer_readback;
pub mod lut_filter;
pub mod ortho_transform;
pub mod overlay;
pub mod paint_target;
pub mod param_effect;
pub mod params;
pub mod preview;
pub mod readback;
pub mod region_store;
pub mod rescale;
pub mod revisions;
pub mod screen_run;
pub mod selection;
pub mod straight_composite;
#[cfg(any(test, feature = "testing"))]
pub mod test_utils;
pub mod texture_registry;
pub mod textured_void;
pub mod transform;
pub mod vector_renderer;
pub mod view;
pub mod void;
pub mod voids;

/// Convert straight-alpha RGBA8 to premultiplied, in place.
///
/// Every texture the compositor samples with a hardware filter stores
/// premultiplied alpha, so that filtering a texel against a transparent
/// neighbour doesn't drag that neighbour's colour into the result. Image data
/// arriving from the browser's 2D canvas (`getImageData`) is straight, so it
/// is converted once at the boundary before upload.
///
/// Rounds to nearest rather than truncating, so an opaque texel is exactly
/// unchanged (`c * 255 / 255 == c`) instead of drifting a level darker.
pub fn premultiply_rgba8_in_place(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a == 255 {
            continue;
        }
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            continue;
        }
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod premultiply_tests {
    use super::premultiply_rgba8_in_place;

    #[test]
    fn premultiply_exact() {
        // Half alpha halves the colour (rounded to nearest).
        let mut half = [255u8, 128, 0, 128];
        premultiply_rgba8_in_place(&mut half);
        assert_eq!(half, [128, 64, 0, 128]);

        // Fully transparent texels lose their colour entirely, so filtering
        // can't resurrect it.
        let mut clear = [200u8, 100, 50, 0];
        premultiply_rgba8_in_place(&mut clear);
        assert_eq!(clear, [0, 0, 0, 0]);

        // Opaque is the identity — no rounding drift.
        let mut opaque = [200u8, 100, 50, 255];
        premultiply_rgba8_in_place(&mut opaque);
        assert_eq!(opaque, [200, 100, 50, 255]);
    }

    #[test]
    fn premultiply_walks_every_texel() {
        let mut two = [255u8, 255, 255, 128, 255, 255, 255, 0];
        premultiply_rgba8_in_place(&mut two);
        assert_eq!(two, [128, 128, 128, 128, 0, 0, 0, 0]);
    }
}
