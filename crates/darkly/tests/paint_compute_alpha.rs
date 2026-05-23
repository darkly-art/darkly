//! Regression test for the alpha-storage bug in `paint_compute.wgsl`.
//!
//! Pre-fix, the shader stored a Porter-Duff *premultiplied* source-over
//! result (`src + dst * (1 - src.a)`) into the **straight-alpha** scratch
//! buffer. A half-coverage white dab — `src = (0.5, 0.5, 0.5, 0.5)` —
//! landed as `(0.5, 0.5, 0.5, 0.5)` straight-alpha and read as
//! `(128, 128, 128, 128)` rgba8: *grey at half opacity* instead of
//! *white at half opacity* (`(255, 255, 255, 128)`).
//!
//! Same root cause as `compositing-lessons-learned.md` §4 (hardware alpha
//! blending writing premultiplied into a straight-alpha target), only
//! manifested in a compute shader instead of a hardware blend state.
//!
//! Phase 0 of `brush-compute-port-v2.md` — the fix prepends
//! `source_over.wgsl` to the shader and calls `source_over` /
//! `destination_out` instead of inlining the math.

use darkly::brush::wire::BrushWireType;
use darkly::brush::BrushNodeRegistry;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::nodegraph::{Graph, PortRef};

fn test_engine(w: u32, h: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, w, h)
}

/// Stabilizer-free Ink Pen graph: a single `stroke_to` lands a bounded
/// number of dabs at the requested position.
///
/// Why not use `engine.brush_load("Ink Pen")`? The library Ink Pen sets
/// `pen.stabilize = 0.6`, which stacks many dabs at the same canvas pixel.
/// Under the buggy shader those dabs accumulate geometrically toward
/// `(1,1,1,1)` — at ~10+ overlapping half-flow dabs the centre pixel
/// would saturate to `(255,255,255,255)` and the regression check
/// "RGB ≈ 255" would pass even against the broken code. Removing
/// stabilization caps the dab count so the buggy `(a, a, a, a)` math
/// stays visible.
fn paint_compute_no_stabilize() -> Graph<BrushWireType> {
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::new();

    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let paint_color = graph.add_node(
        "paint_color",
        registry.get("paint_color").unwrap().ports.clone(),
        vec![],
    );
    let terminal = graph.add_node(
        "paint_compute",
        registry.get("paint_compute").unwrap().ports.clone(),
        vec![],
    );

    let wires = [
        (pen, "position", terminal, "position"),
        (pen, "pressure", terminal, "size_input"),
        (pen, "pressure", terminal, "flow"),
        (paint_color, "color", terminal, "color"),
    ];
    for (fnode, fport, tnode, tport) in wires {
        graph
            .connect(
                PortRef {
                    node: fnode,
                    port: fport.into(),
                },
                PortRef {
                    node: tnode,
                    port: tport.into(),
                },
            )
            .unwrap();
    }

    graph
}

#[test]
fn paint_compute_half_flow_paints_white_not_grey() {
    let (w, h) = (128u32, 128u32);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    let graph = paint_compute_no_stabilize();
    let json = serde_json::to_string(&graph).expect("graph serializes");
    engine
        .set_brush_graph(&json)
        .expect("graph compiles as a brush");

    let cx = (w / 2) as f32;
    let cy = (h / 2) as f32;

    // pressure 0.5 with our graph means flow 0.5 (pressure → terminal.flow
    // directly). The terminal premultiplies the paint colour by flow:
    //   color = (1,1,1,1)·0.5 = (0.5,0.5,0.5,0.5) (premul)
    // At the dab centre, disc coverage = 1, so src.a = 0.5 in the shader.
    //   Bug:   blended = src + dst·(1 - src.a) = (0.5,0.5,0.5,0.5)
    //          → straight-alpha scratch reads grey-and-translucent.
    //   Fix:   source_over(src.rgb, 0.5, dst=(0,0,0,0))
    //          → out_a = 0.5, out_rgb = src.rgb / out_a = (1,1,1)
    //          → straight-alpha scratch reads white-and-translucent.
    //
    // Two `stroke_to` calls at the same position are required: the first
    // event takes the `render_from_stabilized_tail` first-event branch,
    // which queues a dab record on the gpu context but returns before
    // `flush_compute` fires. The second event synthesises a tip-divergence
    // (`tip_vi >= 1`), which routes through `render_from_stabilized_range_to`
    // — that path *does* flush at the end. After the re-render, a single
    // dab is actually dispatched to the scratch buffer (the second event's
    // segment has arc_len ≈ 0, so the loop skips placing additional dabs).
    let stroke_at = |time_ms: f64| StrokeOp::BrushStroke {
        x: cx,
        y: cy,
        pressure: 0.5,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms,
        cr: 1.0,
        cg: 1.0,
        cb: 1.0,
        ca: 1.0,
    };
    engine.begin_stroke(layer_id);
    engine.stroke_to(stroke_at(0.0));
    engine.stroke_to(stroke_at(16.0));
    engine.end_stroke();
    engine.render(0.0);

    let pixels = engine.test_readback_layer(layer_id);
    let idx = ((cy as u32 * w + cx as u32) * 4) as usize;
    let (r, g, b, a) = (
        pixels[idx] as i32,
        pixels[idx + 1] as i32,
        pixels[idx + 2] as i32,
        pixels[idx + 3] as i32,
    );

    assert!(
        a > 0,
        "centre pixel should have some alpha after painting, got rgba={:?}",
        (r, g, b, a),
    );

    // The crux of the regression check. After the fix, the centre reads
    // as pure white at partial alpha (R=G=B=255). Under the bug, R=G=B=A
    // — grey at partial alpha (R≈128 for a single half-flow dab).
    //
    // Tolerance 3 covers rgba8 quantisation through the unpack4x8unorm /
    // pack4x8unorm round-trip plus any minor accumulation if more than
    // one dab landed.
    let tol = 3;
    assert!(
        (r - 255).abs() <= tol && (g - 255).abs() <= tol && (b - 255).abs() <= tol,
        "centre RGB must read pure white (≈255) after the premul→straight-alpha fix; got rgba={:?}",
        (r, g, b, a),
    );
}
