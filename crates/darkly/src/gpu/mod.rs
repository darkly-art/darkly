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

pub mod atlas;
pub mod blend;
pub mod blend_mode;
pub mod blend_modes;
pub mod compositor;
pub mod content_bounds;
pub mod context;
pub mod diff_rect;
pub mod effect;
pub mod flood_fill;
pub mod overlay;
pub mod paint_target;
pub mod params;
pub mod readback;
pub mod region_store;
pub mod selection;
pub mod straight_composite;
pub mod test_utils;
pub mod transform;
pub mod veil;
pub mod veil_chain;
pub mod veils;
pub mod view;
pub mod void;
pub mod voids;
