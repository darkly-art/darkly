//! Round-trip test for rich-layer copy/paste.
//!
//! Confirms `copy_layer_rich` → JSON → `paste_layer_rich` preserves the
//! source layer's pixels, blend mode, opacity, name, and mask presence —
//! the foundation of cross-tab paste in the multi-tab editor.
//!
//! Run with: `cargo test -p darkly --test layer_clipboard -- --test-threads=1`

use darkly::engine::types::{LayerInfo, StrokeOp};
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

fn paint_dot(engine: &mut DarklyEngine, layer_id: LayerId, x: f32, y: f32) {
    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: 1.0,
        cg: 0.0,
        cb: 0.0,
        ca: 1.0,
    });
    engine.end_stroke();
    engine.render(0.0);
}

/// Block until the rich-copy readback completes and return its JSON. Uses
/// the engine's test-only `test_flush_readbacks` helper, which calls
/// `device.poll(Wait)` to drive WebGPU mapping callbacks synchronously.
fn drain_rich_copy(engine: &mut DarklyEngine) -> String {
    engine.test_flush_readbacks();
    engine
        .poll_copy_rich_result()
        .expect("rich copy never produced a result")
}

/// Find a top-level raster layer by id in the engine's serializable tree.
/// Returns `(name, opacity, blend_mode_type_id, modifier_count)`.
fn raster_props(engine: &DarklyEngine, id: LayerId) -> (String, f32, String, usize) {
    let id_f = id.to_ffi() as f64;
    let tree = engine.layer_tree();
    for info in tree {
        if let LayerInfo::Raster {
            id: lid,
            name,
            opacity,
            blend_mode,
            modifiers,
            ..
        } = info
        {
            if (lid - id_f).abs() < 0.5 {
                return (name, opacity, blend_mode.to_string(), modifiers.len());
            }
        }
    }
    panic!("layer {id_f} not found in tree");
}

#[test]
fn rich_copy_paste_preserves_blend_mode_opacity_and_pixels() {
    let (w, h) = (64u32, 64u32);
    let mut source = test_engine(w, h);
    let layer = source.add_raster_layer(None);

    // Paint something so the readback has non-trivial bytes.
    paint_dot(&mut source, layer, 32.0, 32.0);

    // Apply non-default metadata that only the rich path can preserve.
    source.set_opacity(layer, 0.42);
    source.set_blend_mode(layer, "multiply");
    source.set_layer_name(layer, "Source layer");

    source.copy_layer_rich(layer);
    let json = drain_rich_copy(&mut source);

    // Sanity-check the JSON envelope.
    assert!(json.contains("\"blend_mode\":\"multiply\""));
    assert!(json.contains("\"name\":\"Source layer\""));

    // Paste into a fresh engine — same engine→engine round-trip the
    // multi-tab cross-tab paste does, just without the system clipboard
    // in the middle.
    let mut sink = test_engine(w, h);
    let pasted_id = sink
        .paste_layer_rich(&json, None)
        .expect("paste_layer_rich should succeed for a v1 payload");

    let (name, opacity, blend_mode, _modifier_count) = raster_props(&sink, pasted_id);
    assert_eq!(name, "Source layer");
    assert!(
        (opacity - 0.42).abs() < 1e-5,
        "opacity should round-trip; got {opacity}"
    );
    assert_eq!(blend_mode, "multiply", "blend mode should round-trip");

    // Pixels: the pasted layer must contain at least one non-zero alpha
    // pixel (the dab we painted).
    let pasted_pixels = sink.test_readback_layer(pasted_id);
    assert!(
        !pasted_pixels.is_empty(),
        "pasted layer should have non-empty pixel readback"
    );
    let nonzero = pasted_pixels.chunks_exact(4).any(|p| p[3] > 0);
    assert!(
        nonzero,
        "pasted layer should contain at least one non-zero alpha pixel"
    );
}

#[test]
fn rich_paste_records_mask_presence_v1() {
    let (w, h) = (32u32, 32u32);
    let mut source = test_engine(w, h);
    let layer = source.add_raster_layer(None);
    paint_dot(&mut source, layer, 16.0, 16.0);
    source.add_mask(layer);

    source.copy_layer_rich(layer);
    let json = drain_rich_copy(&mut source);
    assert!(
        json.contains("\"mask\":{"),
        "mask metadata should be in JSON"
    );

    let mut sink = test_engine(w, h);
    let pasted_id = sink.paste_layer_rich(&json, None).expect("paste succeeds");

    let (_, _, _, modifier_count) = raster_props(&sink, pasted_id);
    assert!(
        modifier_count > 0,
        "pasted layer should have a modifier (the mask)"
    );
}

#[test]
fn rich_paste_rejects_malformed_json() {
    let mut sink = test_engine(32, 32);
    assert_eq!(sink.paste_layer_rich("not json", None), None);
    assert_eq!(sink.paste_layer_rich("{}", None), None);
}
