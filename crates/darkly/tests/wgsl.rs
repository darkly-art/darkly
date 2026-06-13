//! Framework tests for `crate::brush::wgsl` — the brush-graph
//! → WGSL fragment shader compiler.
//!
//! Asserts:
//!
//! 1. **Identical topologies hash to the same id** — two structurally
//!    identical graphs (independent of node ID allocation) hash to the
//!    same `topology_hash` so the per-brush pipeline cache shares
//!    pipelines.
//! 2. **The Rough Ink builtin compiles end-to-end** — the framework
//!    handles a real graph with random + curve + shape + stamp +
//!    paint and produces non-empty WGSL.

use std::collections::HashMap;

use darkly::brush::eval::BrushNodeEvaluator;
use darkly::brush::wgsl::{compile_brush_to_wgsl, CompileError};
use darkly::brush::wire::BrushWireType;
use darkly::brush::BrushNodeRegistry;
use darkly::nodegraph::{compile, Graph, PortRef};

fn registry() -> &'static BrushNodeRegistry {
    darkly::brush::registry()
}

fn evals() -> HashMap<String, Box<dyn BrushNodeEvaluator>> {
    darkly::brush::registry().evaluators()
}

#[test]
fn empty_graph_errors_cleanly() {
    let graph = Graph::<BrushWireType>::new();
    let reg = registry();
    let plan = compile(&graph, reg.as_map()).unwrap();
    let err = compile_brush_to_wgsl(&graph, &plan, &evals())
        .expect_err("empty graph has no terminal — must error");
    assert!(matches!(err, CompileError::NoTerminal));
}

#[test]
fn rough_ink_brush_compiles_to_nonempty_wgsl() {
    // Lift the Rough Ink graph straight from `builtin_brushes::all()`
    // — it's the canonical demo brush this framework was built to
    // support, and a quick smoke test that every per-node
    // `compile_wgsl` works in the context of a real graph.
    let rough_ink = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Rough Ink")
        .expect("Rough Ink brush registered");
    let reg = registry();
    let plan = compile(&rough_ink.metadata.graph, reg.as_map()).unwrap();
    let compiled =
        compile_brush_to_wgsl(&rough_ink.metadata.graph, &plan, &evals()).expect("compiles");
    assert!(compiled.stroke_wgsl.contains("@fragment"));
    assert!(compiled.stroke_wgsl.contains("fn fs_main"));
    assert!(compiled.stroke_wgsl.contains("shape_r_theta")); // perlin shape
    assert!(compiled.stroke_wgsl.contains("DabRecord"));
    assert!(compiled.stroke_wgsl.contains("Uniforms"));
    // Preview variant must compile too, with the same upstream shape.
    assert!(compiled.cursor_preview_wgsl.contains("@fragment"));
    assert!(compiled.cursor_preview_wgsl.contains("fn fs_main"));
    assert!(compiled.cursor_preview_wgsl.contains("shape_r_theta"));
    assert!(compiled.dab_record_size >= 16); // intrinsic header + pen
    assert!(compiled.uniform_size > 0); // intrinsic + paint_color
    assert!(compiled.topology_hash != 0);
}

/// Regression test: `shape_r_theta` must *subtract* the rotation from θ
/// (not add it). The fragment shader's `theta` is
/// `atan2(local_uv.y, local_uv.x)` with screen y-down — the same frame
/// `pen.drawing_angle` (`atan2(dy, dx)`) lives in, where positive angles
/// are clockwise visually. For a polar formula `r(θ)`, adding α to the
/// argument rotates the geometry CCW in this frame; subtracting rotates
/// it CW. The user-facing semantic is "rotation = α (radians) points the
/// shape's θ=0 reference ray at screen angle α," which makes
/// `pen.drawing_angle → shape.rotation_input` an identity wire that
/// orients the shape along the stroke direction. That semantic requires
/// subtraction. If a future reader is tempted to "clean up" the operator
/// back to `+`, this test will catch it.
#[test]
fn shape_rotation_subtracts_from_theta_for_drawing_angle_compatibility() {
    let rough_ink = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Rough Ink")
        .expect("Rough Ink brush registered");
    let reg = registry();
    let plan = compile(&rough_ink.metadata.graph, reg.as_map()).unwrap();
    let compiled =
        compile_brush_to_wgsl(&rough_ink.metadata.graph, &plan, &evals()).expect("compiles");
    for (label, wgsl) in [
        ("stroke_wgsl", &compiled.stroke_wgsl),
        ("cursor_preview_wgsl", &compiled.cursor_preview_wgsl),
    ] {
        assert!(
            wgsl.contains("theta - p.rotation"),
            "{label} must subtract rotation from theta (drawing_angle compatibility); not found"
        );
        assert!(
            !wgsl.contains("theta + p.rotation"),
            "{label} must not add rotation to theta — that rotates the shape opposite to drawing_angle"
        );
    }
}

/// Regression test: brush stamp rotation must counteract view rotation,
/// so the on-screen orientation is invariant under the user spinning
/// the canvas. The implementation places this correction at two
/// places in the compiled WGSL — every shape node and the canonical
/// stroke-follow wire share these two intercepts, no per-node code.
///
/// 1. The per-fragment skeleton subtracts `u.intrinsic.view_rotation`
///    from `theta`. `_shape.wgsl` does `theta - p.rotation`, so the
///    effective canvas-frame stamp rotation becomes
///    `p.rotation + view_rotation`. The present shader's canvas→screen
///    transform then subtracts `view_rotation` again, leaving the on-
///    screen orientation = the user-set `p.rotation`. Static rotation
///    knobs (e.g. Charcoal's constant 6.3) become screen-relative.
///
/// 2. `pen_input` subtracts `u.intrinsic.view_rotation` from
///    `drawing_angle`'s wire output. `drawing_angle` is
///    `atan2(canvas_dy, canvas_dx)` — a canvas-frame angle. The
///    subtraction lifts it to screen-frame so the canonical
///    `pen.drawing_angle → shape.rotation_input` wire keeps following
///    the on-screen stroke direction after the skeleton's
///    counteraction.
///
/// Wrong signs here (add instead of subtract) cause a self-consistent
/// failure: the cursor rotates at 2× the view's rate. This test
/// guards against both directions.
#[test]
fn stamp_rotation_counteracts_view_rotation() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node(
        "pen_input",
        reg.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let paint_color = graph.add_node(
        "paint_color",
        reg.get("paint_color").unwrap().ports.clone(),
        vec![],
    );
    let shape = graph.add_node(
        "shape",
        reg.get("shape").unwrap().ports.clone(),
        vec![darkly::gpu::params::ParamValue::Int(0)], // Sine
    );
    let stamp = graph.add_node(
        "stamp",
        reg.get("stamp").unwrap().ports.clone(),
        vec![darkly::gpu::params::ParamValue::Int(0)],
    );
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone(), vec![]);
    let wires = [
        (pen, "position", term, "position"),
        (pen, "drawing_angle", shape, "rotation_input"),
        (paint_color, "color", stamp, "color"),
        (shape, "mask", stamp, "tip"),
        (stamp, "dab", term, "rgba"),
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
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("compiles");

    for (label, wgsl) in [
        ("stroke_wgsl", &compiled.stroke_wgsl),
        ("cursor_preview_wgsl", &compiled.cursor_preview_wgsl),
    ] {
        // (1) Skeleton intercept.
        assert!(
            wgsl.contains("atan2(local_uv.y, local_uv.x) - u.intrinsic.view_rotation"),
            "{label} skeleton must subtract view_rotation from theta — without it, \
             stamps rotate with the canvas instead of counteracting view rotation"
        );
        // (2) pen.drawing_angle wire intercept. Field name embeds the
        // node id (e.g. `f0_drawing_angle`); match the suffix.
        assert!(
            wgsl.contains("_drawing_angle - u.intrinsic.view_rotation"),
            "{label} pen.drawing_angle must subtract view_rotation — without it, \
             the canonical drawing_angle → rotation_input wire would push the \
             stamp off the on-screen stroke direction by V"
        );
        // Wrong-sign guards. Adding instead of subtracting was exactly
        // the previous attempt's bug: the two adjustments compounded
        // and the cursor rotated at 2× the view's rate.
        assert!(
            !wgsl.contains("atan2(local_uv.y, local_uv.x) + u.intrinsic.view_rotation"),
            "{label} adds view_rotation to theta — sign error rotates stamp WITH \
             the view at 2× rate"
        );
        assert!(
            !wgsl.contains("_drawing_angle + u.intrinsic.view_rotation"),
            "{label} adds view_rotation to drawing_angle — sign error rotates \
             stamp WITH the view at 2× rate"
        );
    }
}

#[test]
fn topology_hash_is_stable_for_identical_graphs() {
    let rough_a = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Rough Ink")
        .unwrap();
    let rough_b = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Rough Ink")
        .unwrap();
    let reg = registry();
    let plan_a = compile(&rough_a.metadata.graph, reg.as_map()).unwrap();
    let plan_b = compile(&rough_b.metadata.graph, reg.as_map()).unwrap();
    let a = compile_brush_to_wgsl(&rough_a.metadata.graph, &plan_a, &evals()).unwrap();
    let b = compile_brush_to_wgsl(&rough_b.metadata.graph, &plan_b, &evals()).unwrap();
    assert_eq!(a.topology_hash, b.topology_hash);
    assert_eq!(a.dab_record_size, b.dab_record_size);
    assert_eq!(a.uniform_size, b.uniform_size);
}

#[test]
fn extent_protocol_composes_along_chain() {
    // Build the same skeleton the test harness builds for Perlin:
    // pen + shape(perlin) + stamp + paint with a wire on
    // `amplitude` so it counts as wired. shape's extent must report
    // `1 + amplitude.natural_range.max = 1.5`, and the framework's
    // compose pass must surface it on the CompiledBrush.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node(
        "pen_input",
        reg.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let paint_color = graph.add_node(
        "paint_color",
        reg.get("paint_color").unwrap().ports.clone(),
        vec![],
    );
    let rand_amp = graph.add_node(
        "random",
        reg.get("random").unwrap().ports.clone(),
        vec![darkly::gpu::params::ParamValue::Int(0)],
    );
    let shape = graph.add_node(
        "shape",
        reg.get("shape").unwrap().ports.clone(),
        vec![darkly::gpu::params::ParamValue::Int(1)], // Perlin
    );
    let stamp = graph.add_node(
        "stamp",
        reg.get("stamp").unwrap().ports.clone(),
        vec![darkly::gpu::params::ParamValue::Int(0)],
    );
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone(), vec![]);
    let wires = [
        (rand_amp, "value", shape, "amplitude"),
        (shape, "mask", stamp, "tip"),
        (paint_color, "color", stamp, "color"),
        (stamp, "dab", term, "rgba"),
        (pen, "position", term, "position"),
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
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).unwrap();
    // amplitude port has natural_range = (0.0, 0.5); the wire bumps
    // factor to 1.5.
    assert!(
        (compiled.brush_extent_factor - 1.5).abs() < 1e-4,
        "expected extent factor ≈ 1.5, got {}",
        compiled.brush_extent_factor,
    );
    assert!(
        compiled.brush_extent_extra_px.abs() < 1e-6,
        "no displacement nodes — extra_px must be zero, got {}",
        compiled.brush_extent_extra_px,
    );
}

#[test]
fn extent_default_identity_when_no_shape() {
    // pen → paint with no upstream shape node — every node
    // returns the trait-default `Identity`, so the brush extent
    // collapses to (factor=1.0, extra_px=0.0). bbox_radius then
    // equals the dab's effective_radius, matching the existing
    // `paint` terminal's footprint exactly.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node(
        "pen_input",
        reg.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone(), vec![]);
    graph
        .connect(
            PortRef {
                node: pen,
                port: "position".into(),
            },
            PortRef {
                node: term,
                port: "position".into(),
            },
        )
        .unwrap();
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).unwrap();
    assert!(
        (compiled.brush_extent_factor - 1.0).abs() < 1e-6,
        "no shape upstream — factor must be 1.0, got {}",
        compiled.brush_extent_factor,
    );
    assert!(
        compiled.brush_extent_extra_px.abs() < 1e-6,
        "no shape upstream — extra_px must be 0.0, got {}",
        compiled.brush_extent_extra_px,
    );
}

#[test]
fn paint_only_graph_falls_through_to_disc() {
    // pen_input → paint with no upstream graph: terminal's
    // `rgba` input is unwired, so the fallback "opaque white modulated
    // by local_dist" path runs. Smoke test that this compiles too.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node(
        "pen_input",
        reg.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone(), vec![]);
    graph
        .connect(
            PortRef {
                node: pen,
                port: "position".into(),
            },
            PortRef {
                node: term,
                port: "position".into(),
            },
        )
        .unwrap();
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals())
        .expect("paint with no rgba wire still compiles");
    assert!(compiled.stroke_wgsl.contains("local_dist"));
    assert!(compiled
        .stroke_wgsl
        .contains("vec4<f32>(1.0, 1.0, 1.0, 1.0)"));
}
