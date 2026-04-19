//! Engine-level integration test for brush preview regen through the
//! overlay mask pipeline. Covers:
//!   - `regenerate_brush_preview` finds the preview producer via the
//!     `color_output.preview` wire in the default graph.
//!   - The stamp's `render_preview` writes into the overlay's preview mask.
//!   - `BrushPreviewInfo` is cached on the engine with non-zero extents.

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

fn test_engine(w: u32, h: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, w, h)
}

#[test]
fn default_graph_regenerates_brush_preview_into_overlay_mask() {
    let mut engine = test_engine(256, 256);

    // Default graph is wired `stamp.dab → color_output.preview`, so this
    // should trigger a stamp render_preview into the overlay preview mask.
    engine.regenerate_brush_preview();

    // Engine caches the canvas-space info for the brush tool.
    let info = engine.brush_preview_info()
        .expect("default graph has a preview producer");
    assert!(info.half_extent_canvas_px[0] > 0.0);
    assert!(info.half_extent_canvas_px[1] > 0.0);
    assert_eq!(info.rotation_rad, 0.0, "default graph has rotation=0");

    // The overlay should now own a populated preview mask texture.
    let (mask_w, mask_h) = engine.compositor_preview_mask_size();
    assert!(mask_w > 0 && mask_h > 0, "preview mask allocated");

    // Read back the mask and verify it's non-empty (default circle tip fills
    // most of the target). Center pixel's red channel should be > 0.
    let pixels = engine.test_readback_overlay_preview_mask();
    let cx = mask_w / 2;
    let cy = mask_h / 2;
    let center_idx = ((cy * mask_w + cx) * 4) as usize;
    let center_r = pixels[center_idx];
    assert!(center_r > 0,
        "preview mask center should be non-zero (got {}): default round \
         stamp must write its alpha into the mask target", center_r);

    // A pixel near the very edge of the target should be transparent,
    // confirming the stamp renders as a bounded disc, not a filled rect.
    let edge_idx = (0 * 4) as usize;
    let edge_a = pixels[edge_idx + 3];
    assert!(edge_a <= 16,
        "preview mask corner should be near-zero alpha, got {edge_a}");
}
