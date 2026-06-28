//! Engine-level GPU integration test for the text / vector layer.
//!
//! Adds a text layer, realizes it through the compositor (Vello render), reads
//! back the layer texture and asserts the glyphs produced coverage, then
//! confirms an undo removes the layer and a redo restores it with text intact.
//!
//! Run with: `cargo test -p darkly --test text_layer --features testing -- --test-threads=1`

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::TextProps;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Count non-transparent pixels in an RGBA buffer.
fn covered_pixels(pixels: &[u8]) -> usize {
    pixels.chunks_exact(4).filter(|p| p[3] > 0).count()
}

#[test]
fn text_layer_realizes_glyph_coverage_then_undo_redo() {
    let (w, h) = (256, 128);
    let mut engine = test_engine(w, h);

    let mut text = TextProps::new("Hello".to_string());
    text.size = 64.0;
    let id = engine.add_text_layer(text, 16.0, 16.0, [255, 255, 255, 255], None);
    assert!(engine.has_layer(id), "text layer is in the tree after add");

    // Force a composite so the vector layer realizes via Vello.
    let _ = engine.test_readback_canvas();

    let pixels = engine.test_readback_layer(id);
    let covered = covered_pixels(&pixels);
    assert!(
        covered > 0,
        "rendered text must produce non-empty coverage (got {covered})"
    );

    // Undo removes the layer from the tree.
    engine.undo();
    assert!(!engine.has_layer(id), "undo removes the text layer");

    // Redo restores the layer and it re-realizes (text object survived the
    // undo/redo round-trip — content stability is covered by the layer-kind
    // serialize test).
    engine.redo();
    assert!(engine.has_layer(id), "redo restores the text layer");
    let _ = engine.test_readback_canvas();
    let after = covered_pixels(&engine.test_readback_layer(id));
    assert!(after > 0, "text re-realizes after redo (got {after})");
}

#[test]
fn editing_text_content_re_realizes() {
    let (w, h) = (256, 128);
    let mut engine = test_engine(w, h);

    let id = engine.add_text_layer(
        TextProps::new("i".to_string()),
        16.0,
        16.0,
        [255, 255, 255, 255],
        None,
    );
    let _ = engine.test_readback_canvas();
    let thin = covered_pixels(&engine.test_readback_layer(id));

    // A much wider string must cover strictly more pixels after re-realization.
    engine.set_text_content(id, "WWWWWWWW".to_string());
    let _ = engine.test_readback_canvas();
    let wide = covered_pixels(&engine.test_readback_layer(id));

    assert!(
        wide > thin,
        "editing content re-rasterizes: wide ({wide}) should exceed thin ({thin})"
    );
}
