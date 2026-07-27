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
use darkly::brush::input_value::InputValue;
use darkly::brush::wgsl::{compile_brush_to_wgsl, CompileError};
use darkly::brush::wire::BrushWireType;
use darkly::brush::BrushNodeRegistry;
use darkly::nodegraph::{compile, Graph, NodeId, PortRef};

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
/// `pen.drawing_angle → circle.rotation_input` an identity wire that
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
/// places in the compiled WGSL — every circle node and the canonical
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
///    `pen.drawing_angle → circle.rotation_input` wire keeps following
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
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let paint_color = graph.add_node("paint_color", reg.get("paint_color").unwrap().ports.clone());
    let shape = graph.add_node("circle", reg.get("circle").unwrap().ports.clone());
    let stamp = graph.add_node("stamp", reg.get("stamp").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    let wires = [
        (pen.clone(), "position", term.clone(), "position"),
        (
            pen.clone(),
            "drawing_angle",
            shape.clone(),
            "rotation_input",
        ),
        (paint_color.clone(), "color", stamp.clone(), "color"),
        (shape.clone(), "mask", stamp.clone(), "tip"),
        (stamp.clone(), "dab", term.clone(), "rgba"),
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
    // pen + circle(perlin) + stamp + paint with a wire on
    // `amplitude` so it counts as wired. shape's extent must report
    // `1 + amplitude.natural_range.max = 1.5`, and the framework's
    // compose pass must surface it on the CompiledBrush.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let paint_color = graph.add_node("paint_color", reg.get("paint_color").unwrap().ports.clone());
    let rand_amp = graph.add_node("random", reg.get("random").unwrap().ports.clone());
    let shape = graph.add_node("circle", reg.get("circle").unwrap().ports.clone());
    graph
        .set_port_value(&shape, "algorithm", InputValue::Int(1))
        .unwrap(); // Perlin
    let stamp = graph.add_node("stamp", reg.get("stamp").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    let wires = [
        (rand_amp.clone(), "value", shape.clone(), "amplitude"),
        (shape.clone(), "mask", stamp.clone(), "tip"),
        (paint_color.clone(), "color", stamp.clone(), "color"),
        (stamp.clone(), "dab", term.clone(), "rgba"),
        (pen.clone(), "position", term.clone(), "position"),
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
fn extent_grows_with_shape_aspect_anisotropy() {
    // The `aspect` knob squashes the tip into an ellipse; a thinner nib
    // (smaller aspect) has a longer perpendicular axis, so the dab bbox must
    // grow by the worst-case anisotropy factor `1 / aspect_min`. Build
    // pen + circle(sine, amplitude unwired ⇒ base radius 1) + stamp + paint
    // with a wire on `aspect` so its natural-range minimum (0.1) counts:
    // factor must reach 1/0.1 = 10. Without folding `aspect` into the extent,
    // the tall nib would be clipped to the round bbox on save-point rewind.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let paint_color = graph.add_node("paint_color", reg.get("paint_color").unwrap().ports.clone());
    let rand_aspect = graph.add_node("random", reg.get("random").unwrap().ports.clone());
    let shape = graph.add_node("circle", reg.get("circle").unwrap().ports.clone());
    let stamp = graph.add_node("stamp", reg.get("stamp").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    let wires = [
        (rand_aspect.clone(), "value", shape.clone(), "aspect"),
        (shape.clone(), "mask", stamp.clone(), "tip"),
        (paint_color.clone(), "color", stamp.clone(), "color"),
        (stamp.clone(), "dab", term.clone(), "rgba"),
        (pen.clone(), "position", term.clone(), "position"),
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
    // base = 1 (sine, amplitude 0); aniso_max = 1/aspect_min = 1/0.1 = 10.
    assert!(
        (compiled.brush_extent_factor - 10.0).abs() < 1e-3,
        "wired aspect (min 0.1) must inflate the bbox ×10, got {}",
        compiled.brush_extent_factor,
    );
    // The compiled silhouette must carry the aspect argument into ShapeParams.
    assert!(
        compiled.stroke_wgsl.contains("ShapeParams"),
        "shape brush must emit a ShapeParams constructor",
    );
}

#[test]
fn extent_neutral_when_aspect_unwired() {
    // Regression: the default `aspect` (1.0) must leave the bbox unchanged so
    // every existing round brush keeps its footprint. pen + circle(sine,
    // amplitude wired ⇒ 1.5) + stamp + paint with `aspect` left unwired: the
    // factor must stay at the pre-anisotropy 1.5, not grow.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let paint_color = graph.add_node("paint_color", reg.get("paint_color").unwrap().ports.clone());
    let rand_amp = graph.add_node("random", reg.get("random").unwrap().ports.clone());
    let shape = graph.add_node("circle", reg.get("circle").unwrap().ports.clone());
    let stamp = graph.add_node("stamp", reg.get("stamp").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    let wires = [
        (rand_amp.clone(), "value", shape.clone(), "amplitude"),
        (shape.clone(), "mask", stamp.clone(), "tip"),
        (paint_color.clone(), "color", stamp.clone(), "color"),
        (stamp.clone(), "dab", term.clone(), "rgba"),
        (pen.clone(), "position", term.clone(), "position"),
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
    assert!(
        (compiled.brush_extent_factor - 1.5).abs() < 1e-4,
        "unwired aspect (default 1.0) must leave factor at 1.5, got {}",
        compiled.brush_extent_factor,
    );
}

#[test]
fn extent_default_identity_when_no_shape() {
    // pen → paint with no upstream circle node — every node
    // returns the trait-default `Identity`, so the brush extent
    // collapses to (factor=1.0, extra_px=0.0). bbox_radius then
    // equals the dab's effective_radius, matching the existing
    // `paint` terminal's footprint exactly.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
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
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
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

/// The Clone builtin compiles to WGSL with `samples_source` set, the
/// stroke shader declares the `@group(3)` source binding and calls the
/// clone-sample helper, and the preview variant compiles too (it binds a
/// fallback so it must still declare the source). Naga validation of the
/// assembled shader happens when the pipeline builds — see `tests/clone.rs`.
#[test]
fn clone_brush_compiles_with_samples_source() {
    let clone = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Clone")
        .expect("Clone brush registered");
    let reg = registry();
    let plan = compile(&clone.metadata.graph, reg.as_map()).unwrap();
    let compiled =
        compile_brush_to_wgsl(&clone.metadata.graph, &plan, &evals()).expect("clone compiles");

    assert!(compiled.samples_source, "clone must set samples_source");
    assert!(
        compiled.graph_texture_names.is_empty(),
        "clone source is not a named registry texture"
    );
    // Stroke shader declares the @group(3) source texture and samples it.
    assert!(compiled
        .stroke_wgsl
        .contains("@group(3) @binding(0) var graph_smp"));
    assert!(compiled.stroke_wgsl.contains("graph_tex_0"));
    assert!(compiled.stroke_wgsl.contains("fn clone_sample"));
    // Regression: the source sample sits behind a `uv`-dependent in-bounds
    // branch (non-uniform control flow), so it must use derivative-free
    // `textureSampleLevel`. `textureSample` computes implicit derivatives
    // and is illegal in non-uniform control flow — native naga is lenient,
    // but the browser's WGSL validator rejects it and the brush fails at
    // pipeline-build time in-app.
    assert!(
        compiled
            .stroke_wgsl
            .contains("textureSampleLevel(graph_tex_0"),
        "clone must sample with textureSampleLevel (derivative-free), not textureSample"
    );
    assert!(!compiled.stroke_wgsl.contains("textureSample(graph_tex_0"));
    // The stroke body *invokes* the sampler (`let clone_c_N = clone_sample_N(…)`).
    assert!(
        compiled.stroke_wgsl.contains("= clone_sample"),
        "stroke body must invoke the clone sampler"
    );

    // --- Dab-preview regression (Fix A) ---
    // Under Approach A the preview shader still *declares* the @group(3)
    // source binding (the shared `clone_sample` decl references it, bound to
    // the fallback tile), so the declaration is present in both shaders.
    assert!(compiled
        .cursor_preview_wgsl
        .contains("@group(3) @binding(0) var graph_smp"));
    assert!(compiled.cursor_preview_wgsl.contains("fn clone_sample"));
    // But the preview *body* must not sample the source — it emits a neutral
    // constant instead. The tell is the call site, not the declaration.
    assert!(
        !compiled.cursor_preview_wgsl.contains("= clone_sample"),
        "preview body must not invoke the clone sampler (Fix A: neutral fill, no source at hover)"
    );
    assert!(
        compiled
            .cursor_preview_wgsl
            .contains("vec4<f32>(0.6, 0.6, 0.6, 1.0)"),
        "preview body must emit the neutral clone fill"
    );
    // Both variants are complete, valid shaders.
    naga_validate(&compiled.stroke_wgsl, "clone stroke_wgsl");
    naga_validate(&compiled.cursor_preview_wgsl, "clone cursor_preview_wgsl");
}

/// The noise node compiles to smooth, per-channel fBm: three independent
/// seeds drive `fbm_rot` (interpolated value noise), NOT the old blocky
/// `node_noise_value` cell hash, and no 3D texture binding leaks in from the
/// lib split. The fully assembled shader validates under naga.
#[test]
fn noise_node_emits_per_channel_fbm() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
    for (name, v) in [
        ("scale", InputValue::Scalar(32.0)),
        ("seed", InputValue::Int(7)),
        ("octaves", InputValue::Scalar(4.0)),
        ("warp", InputValue::Scalar(0.6)),
        ("roughness", InputValue::Scalar(0.5)),
    ] {
        graph.set_port_value(&noise, name, v).unwrap();
    }
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    let wires = [
        (pen.clone(), "position", term.clone(), "position"),
        (noise.clone(), "color", term.clone(), "rgba"),
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
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("noise compiles");

    // Smooth, interpolated fBm — not the old blocky cell hash.
    assert!(
        compiled.stroke_wgsl.contains("fbm_rot("),
        "noise must call the interpolated fBm helper; not found in:\n{}",
        compiled.stroke_wgsl,
    );
    assert!(
        !compiled.stroke_wgsl.contains("node_noise_value"),
        "noise must not emit the old cell-noise call",
    );
    // Three channels off consecutive seeds (7, 8, 9) — independent R/G/B.
    for seed_lit in ["7u", "8u", "9u"] {
        assert!(
            compiled.stroke_wgsl.contains(seed_lit),
            "noise must drive channel seed {seed_lit}; not found in:\n{}",
            compiled.stroke_wgsl,
        );
    }
    // The 2D/3D lib split must not leak the 3D texture path into the brush
    // shader (which has no such binding) — a re-merge would fail here and also
    // fail naga with a `@group(0)` collision.
    assert!(
        !compiled.stroke_wgsl.contains("texture_3d")
            && !compiled.stroke_wgsl.contains("fbm_noise3d"),
        "brush shader must not include the 3D fbm bindings",
    );
    // Fully assembled shader is valid under the same front-end wgpu uses.
    naga_validate(&compiled.stroke_wgsl, "noise stroke_wgsl");
    naga_validate(&compiled.cursor_preview_wgsl, "noise cursor_preview_wgsl");
}

/// Feature test: the `polygon` node compiles and emits its signed-distance
/// helper plus a `smoothstep` feather in both shader variants, and both
/// validate under naga.
#[test]
fn polygon_node_compiles_and_emits_sdf() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let paint_color = graph.add_node("paint_color", reg.get("paint_color").unwrap().ports.clone());
    let poly = graph.add_node("polygon", reg.get("polygon").unwrap().ports.clone());
    graph
        .set_port_value(&poly, "points", InputValue::Int(5))
        .unwrap();
    let stamp = graph.add_node("stamp", reg.get("stamp").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    wire(
        &mut graph,
        &[
            (pen.clone(), "position", term.clone(), "position"),
            (paint_color.clone(), "color", stamp.clone(), "color"),
            (poly.clone(), "mask", stamp.clone(), "tip"),
            (stamp.clone(), "dab", term.clone(), "rgba"),
        ],
    );
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("polygon compiles");
    for (label, w) in [
        ("stroke_wgsl", &compiled.stroke_wgsl),
        ("cursor_preview_wgsl", &compiled.cursor_preview_wgsl),
    ] {
        assert!(
            w.contains("polygon_sdf"),
            "{label} must emit the node-id-suffixed SDF helper",
        );
        assert!(
            w.contains("smoothstep("),
            "{label} must feather the SDF with smoothstep",
        );
    }
    naga_validate(&compiled.stroke_wgsl, "polygon stroke_wgsl");
    naga_validate(&compiled.cursor_preview_wgsl, "polygon cursor_preview_wgsl");
}

/// The rounded regular-polygon SDF (a Rust mirror of the node's emitted
/// `polygon_sdf` helper, plus the `sd - ρ` rounding operator it composes).
/// Confirms the exact contract: negative inside, the vertices on the unit
/// circumcircle, the edge midpoint on the boundary at the apothem, and that
/// rounding morphs the tip toward the unit disc while never exceeding the
/// circumradius `1.0` (the constant extent bound).
#[test]
fn polygon_sdf_geometry() {
    // Circumradius-`r` regular n-gon SDF — iq's `sdRegularPolygon`, transcribed
    // to match `polygon.rs`'s emitted WGSL byte-for-byte.
    fn poly_sdf(p: [f32; 2], n: f32, r: f32) -> f32 {
        let an = std::f32::consts::PI / n;
        let acs = [an.cos(), an.sin()];
        let a0 = p[0].atan2(p[1]);
        let two_an = 2.0 * an;
        let bn = (a0 - two_an * (a0 / two_an).floor()) - an;
        let len = (p[0] * p[0] + p[1] * p[1]).sqrt();
        let mut q = [len * bn.cos(), len * bn.sin().abs()];
        q[0] -= r * acs[0];
        q[1] -= r * acs[1];
        q[1] += (-q[1]).clamp(0.0, r * acs[1]);
        let ql = (q[0] * q[0] + q[1] * q[1]).sqrt();
        ql * q[0].signum()
    }
    // The node's composition: circumradius `1 - ρ`, then round by `ρ`.
    fn rounded(p: [f32; 2], n: f32, rounding: f32) -> f32 {
        poly_sdf(p, n, 1.0 - rounding) - rounding
    }

    for &n in &[3.0_f32, 4.0, 5.0, 6.0, 12.0] {
        let an = std::f32::consts::PI / n;
        let apothem = an.cos();
        // A vertex points straight up (+y); the edge midpoint bisects the
        // sector at angle `an` from +y.
        let vertex_dir = [0.0_f32, 1.0];
        let edge_dir = [an.sin(), an.cos()];

        // Sharp polygon (ρ = 0): negative inside, vertex on the unit circle,
        // edge midpoint on the boundary at the apothem.
        assert!(
            rounded([0.0, 0.0], n, 0.0) < 0.0,
            "n={n}: centre must be inside",
        );
        assert!(
            rounded(vertex_dir, n, 0.0).abs() < 1e-3,
            "n={n}: a vertex sits on the unit circumcircle (sd≈0)",
        );
        let edge_mid = [edge_dir[0] * apothem, edge_dir[1] * apothem];
        assert!(
            rounded(edge_mid, n, 0.0).abs() < 1e-3,
            "n={n}: the edge midpoint sits on the boundary at the apothem",
        );
        // A radius-1 point in the edge direction is outside the sharp polygon.
        assert!(
            rounded(edge_dir, n, 0.0) > 1e-3,
            "n={n}: radius-1 in the edge direction is outside the sharp polygon",
        );

        // Rounding morphs toward the unit disc: at ρ = 1 the field is exactly
        // `|p| - 1` (isotropic), so the edge direction that was outside at ρ = 0
        // now sits on the boundary, and every direction has radius 1.
        assert!(
            rounded(edge_dir, n, 1.0).abs() < 1e-3,
            "n={n}: full rounding pushes the boundary out to the unit disc",
        );
        for i in 0..16 {
            let a = (i as f32) * std::f32::consts::TAU / 16.0;
            let p = [0.5 * a.cos(), 0.5 * a.sin()];
            assert!(
                (rounded(p, n, 1.0) - (-0.5)).abs() < 1e-3,
                "n={n}: at ρ=1 the field is the unit disc (sd = |p| - 1)",
            );
        }

        // The circumradius bound `1.0` holds for every rounding: no point
        // beyond radius 1 is ever inside.
        for &rounding in &[0.0_f32, 0.5, 1.0] {
            for i in 0..64 {
                let a = (i as f32) * std::f32::consts::TAU / 64.0;
                let p = [1.001 * a.cos(), 1.001 * a.sin()];
                assert!(
                    rounded(p, n, rounding) > 0.0,
                    "n={n} ρ={rounding}: nothing past the circumradius may be inside",
                );
            }
        }
    }
}

/// The `polygon` node opts into per-node previews by flagging its `mask`
/// output as a spatial image.
#[test]
fn polygon_node_previewable() {
    let reg = registry();
    assert!(
        reg.get("polygon").unwrap().preview_output().is_some(),
        "polygon.mask is a spatial coverage field and must be previewable",
    );
}

/// The polygon node folds `aspect` into its footprint the same way the circle
/// family does: a wired `aspect` (natural-range min 0.1) must inflate the dab
/// bbox ×10 so a thin nib isn't clipped on save-point rewind.
#[test]
fn polygon_extent_grows_with_aspect() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let paint_color = graph.add_node("paint_color", reg.get("paint_color").unwrap().ports.clone());
    let rand_aspect = graph.add_node("random", reg.get("random").unwrap().ports.clone());
    let poly = graph.add_node("polygon", reg.get("polygon").unwrap().ports.clone());
    let stamp = graph.add_node("stamp", reg.get("stamp").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    wire(
        &mut graph,
        &[
            (rand_aspect.clone(), "value", poly.clone(), "aspect"),
            (poly.clone(), "mask", stamp.clone(), "tip"),
            (paint_color.clone(), "color", stamp.clone(), "color"),
            (stamp.clone(), "dab", term.clone(), "rgba"),
            (pen.clone(), "position", term.clone(), "position"),
        ],
    );
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).unwrap();
    assert!(
        (compiled.brush_extent_factor - 10.0).abs() < 1e-3,
        "wired aspect (min 0.1) must inflate the bbox ×10, got {}",
        compiled.brush_extent_factor,
    );
}

/// Coordinate-frame guard: the polygon tip must be screen-relative like every
/// theta-based tip. Because it works from raw `local_uv`, it has to fold
/// `view_rotation` into its own rotation — otherwise the dab spins as the user
/// rotates the canvas view. Assert both variants build the rotation angle with
/// `view_rotation` folded in.
#[test]
fn polygon_is_view_rotation_invariant() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let paint_color = graph.add_node("paint_color", reg.get("paint_color").unwrap().ports.clone());
    let poly = graph.add_node("polygon", reg.get("polygon").unwrap().ports.clone());
    let stamp = graph.add_node("stamp", reg.get("stamp").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    wire(
        &mut graph,
        &[
            (pen.clone(), "position", term.clone(), "position"),
            (paint_color.clone(), "color", stamp.clone(), "color"),
            (poly.clone(), "mask", stamp.clone(), "tip"),
            (stamp.clone(), "dab", term.clone(), "rgba"),
        ],
    );
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("polygon compiles");
    for (label, w) in [
        ("stroke_wgsl", &compiled.stroke_wgsl),
        ("cursor_preview_wgsl", &compiled.cursor_preview_wgsl),
    ] {
        assert!(
            w.contains("_phi: f32"),
            "{label} polygon must build a rotation angle",
        );
        assert!(
            w.contains("+ u.intrinsic.view_rotation);"),
            "{label} polygon rotation must fold in view_rotation — without it the \
             tip spins with the canvas view (coordinate-frame regression)",
        );
    }
}

/// Parse + validate a fully assembled brush shader under naga (the same
/// front-end wgpu uses in-app), panicking with the diagnostic on failure.
fn naga_validate(src: &str, label: &str) {
    let module = naga::front::wgsl::parse_str(src)
        .unwrap_or_else(|e| panic!("{label} failed to parse under naga:\n{e}"));
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    validator
        .validate(&module)
        .unwrap_or_else(|e| panic!("{label} failed naga validation:\n{e:?}"));
}

/// Double-allocation regression (Fix A, critique #5): a static-texture brush
/// (`Charcoal`, whose `image` node allocates a `@group(3)` paper texture)
/// must declare **exactly one** graph texture in its preview shader. The
/// preview recompile runs against throwaway allocation cells, so a
/// non-terminal node re-emitting its body for the preview pass can't
/// double-count a second `@group(3)` texture into the layout.
#[test]
fn static_texture_brush_preview_declares_single_graph_texture() {
    let charcoal = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Charcoal")
        .expect("Charcoal brush registered");
    // Go through `compile_graph` — it applies the Switch rewrite the
    // persisted graph needs before WGSL compilation, matching the in-app
    // path — and read the compiled brush off the runner.
    let runner = darkly::brush::compile_graph(&charcoal.metadata.graph).expect("charcoal compiles");
    let compiled = runner
        .compiled_brush()
        .expect("charcoal has a compiled terminal");

    assert_eq!(
        compiled.graph_texture_names.len(),
        1,
        "charcoal declares one graph texture (paper)"
    );
    let preview_texture_decls = compiled
        .cursor_preview_wgsl
        .matches("var graph_tex_")
        .count();
    assert_eq!(
        preview_texture_decls, 1,
        "preview must declare exactly one @group(3) texture, not a double-allocated second"
    );
    // The preview shader is valid (the paper grain samples in both modes).
    naga_validate(
        &compiled.cursor_preview_wgsl,
        "charcoal cursor_preview_wgsl",
    );
    naga_validate(&compiled.stroke_wgsl, "charcoal stroke_wgsl");
}

// ── Sampling-frame selector (noise/image `space`) ───────────────────────
//
// The `space` param folds a coordinate frame into the emitted sample
// coordinate at compile time: Canvas keeps the historical `target_pos /
// scale` (grain pinned to the canvas); Dab samples the stamp's oriented
// unit frame so the grain rides the rotating stamp. These tests assert the
// emitter picks the right arm and that both shader variants stay valid.

/// Apply the noise node's inputs in registration order, with
/// `space`/`scale_with_brush`. `space`: 0 = Canvas, 1 = Dab.
fn apply_noise_inputs(
    graph: &mut Graph<BrushWireType>,
    noise: &NodeId,
    space: i32,
    scale_with_brush: bool,
) {
    for (name, v) in [
        ("scale", InputValue::Scalar(32.0)),
        ("seed", InputValue::Int(7)),
        ("octaves", InputValue::Scalar(4.0)),
        ("warp", InputValue::Scalar(0.6)),
        ("roughness", InputValue::Scalar(0.5)),
        ("space", InputValue::Int(space)),
        ("scale_with_brush", InputValue::Bool(scale_with_brush)),
    ] {
        graph.set_port_value(noise, name, v).unwrap();
    }
}

/// Wire a slice of `(from_node, from_port, to_node, to_port)` into a graph.
fn wire(
    graph: &mut Graph<BrushWireType>,
    wires: &[(
        darkly::nodegraph::NodeId,
        &str,
        darkly::nodegraph::NodeId,
        &str,
    )],
) {
    for (fnode, fport, tnode, tport) in wires {
        graph
            .connect(
                PortRef {
                    node: fnode.clone(),
                    port: (*fport).into(),
                },
                PortRef {
                    node: tnode.clone(),
                    port: (*tport).into(),
                },
            )
            .unwrap();
    }
}

#[test]
fn noise_canvas_space_is_byte_identical() {
    // Regression guard: a Canvas-space noise brush must emit the exact
    // `target_pos / scale` coordinate shipped brushes already rely on, and
    // never the Dab oriented-frame locals. Guarantees zero behavior change
    // for existing brushes when the frame selector defaults to Canvas.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
    apply_noise_inputs(&mut graph, &noise, 0, true);
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    wire(
        &mut graph,
        &[
            (pen.clone(), "position", term.clone(), "position"),
            (noise.clone(), "color", term.clone(), "rgba"),
        ],
    );
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("compiles");
    // `scale` is now a Scalar *input* read via `cctx.input("scale").as_f32()`,
    // and `sample_frame` interpolates the scale expression parenthesized — so
    // an unwired 32.0 default emits `target_pos / (32.000000)`. The numeric
    // literal (`32.000000`) is unchanged; the surrounding parens are the only
    // permitted textual delta, required so a *wired* scale expression composes.
    // This pins the param→input migration for a value that was a `ParamDef`
    // before this change.
    assert!(
        compiled.stroke_wgsl.contains("target_pos / (32.000000)"),
        "Canvas mode must emit the parenthesized canvas-pixel coordinate; not found",
    );
    assert!(
        !compiled.stroke_wgsl.contains("dab_local"),
        "Canvas mode must not emit the Dab oriented-frame basis",
    );
    naga_validate(&compiled.stroke_wgsl, "noise canvas stroke");
    naga_validate(&compiled.cursor_preview_wgsl, "noise canvas preview");
}

/// The settable-source `brush_settings.size` must reach a *compiled* brush:
/// wiring it into a node that reads its input in WGSL (`noise.scale`) has to
/// emit the packed per-dab size field in the shader, not the node's default
/// literal. This is the guard for the review's F1 — a source seeded only in the
/// CPU slot table would compile clean here and silently deliver nothing.
#[test]
fn settings_size_source_reaches_compiled_brush() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let settings = graph.add_node(
        "brush_settings",
        reg.get("brush_settings").unwrap().ports.clone(),
    );
    // A distinctive base size — packed per-dab at runtime, so it appears in the
    // shader as a `d.<field>` reference, never as a baked literal.
    graph
        .set_port_default(&settings, "size", 0.25)
        .expect("brush_settings has a size input");
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
    apply_noise_inputs(&mut graph, &noise, 0, true);
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    wire(
        &mut graph,
        &[
            (settings.clone(), "size", noise.clone(), "scale"),
            (pen.clone(), "position", term.clone(), "position"),
            (noise.clone(), "color", term.clone(), "rgba"),
        ],
    );
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("compiles");

    // `noise.scale` now divides by the wired size dab field, not the 32.0
    // default literal — proof the source flows through the compiled path.
    let size_field = format!("d.n{}_size", settings.0);
    assert!(
        compiled.stroke_wgsl.contains(&size_field),
        "compiled shader must reference the packed size field `{size_field}`",
    );
    assert!(
        !compiled.stroke_wgsl.contains("target_pos / (32.000000)"),
        "the wired size must override noise.scale's default literal",
    );
    naga_validate(&compiled.stroke_wgsl, "settings.size → noise.scale stroke");
    naga_validate(
        &compiled.cursor_preview_wgsl,
        "settings.size → noise.scale preview",
    );
}

/// A wirable scalar input (`noise.scale`) driven by a per-dab sensor
/// (`pen.pressure`) must emit the upstream expression in the divide — not a
/// literal — and both shader variants must still pass naga validation. Guards
/// the subsumed scalar-to-port conversion at its highest-risk seam.
#[test]
fn noise_scale_wired_emits_upstream_expr_and_validates() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
    apply_noise_inputs(&mut graph, &noise, 0, true);
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    wire(
        &mut graph,
        &[
            (pen.clone(), "position", term.clone(), "position"),
            (pen.clone(), "pressure", noise.clone(), "scale"),
            (noise.clone(), "color", term.clone(), "rgba"),
        ],
    );
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("compiles");
    // The literal default must be gone — the divide now reads the wired dab
    // field (`d.n0_pressure` or similar), parenthesized.
    assert!(
        !compiled.stroke_wgsl.contains("target_pos / (32.000000)"),
        "wired scale must not fall back to the literal default",
    );
    assert!(
        compiled.stroke_wgsl.contains("_pressure"),
        "wired scale must interpolate the upstream pressure expression",
    );
    naga_validate(&compiled.stroke_wgsl, "noise wired-scale stroke");
    naga_validate(&compiled.cursor_preview_wgsl, "noise wired-scale preview");
}

/// A wired `octaves` input must emit the `clamp(i32(round(..)), 1, 8)` guard
/// (i32, not u32 — `fbm_rot`'s octave arg is i32) and validate on both
/// variants — the naga-riskiest arm of the subsumed conversion.
#[test]
fn noise_octaves_wired_emits_i32_clamp_and_validates() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let rand = graph.add_node("random", reg.get("random").unwrap().ports.clone());
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
    apply_noise_inputs(&mut graph, &noise, 0, true);
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    wire(
        &mut graph,
        &[
            (pen.clone(), "position", term.clone(), "position"),
            (rand.clone(), "value", noise.clone(), "octaves"),
            (noise.clone(), "color", term.clone(), "rgba"),
        ],
    );
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("compiles");
    assert!(
        compiled.stroke_wgsl.contains("clamp(i32(round("),
        "wired octaves must emit an i32 round+clamp guard",
    );
    naga_validate(&compiled.stroke_wgsl, "noise wired-octaves stroke");
    naga_validate(&compiled.cursor_preview_wgsl, "noise wired-octaves preview");
}

#[test]
fn noise_dab_space_emits_oriented_frame_and_variation() {
    // Dab mode must rotate the unit-disc offset by the `rotation` input and
    // fold the `variation` offset. With both inputs unwired they fall to
    // literal defaults (0), so the basis and offset still appear.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
    apply_noise_inputs(&mut graph, &noise, 1, true);
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    wire(
        &mut graph,
        &[
            (pen.clone(), "position", term.clone(), "position"),
            (noise.clone(), "color", term.clone(), "rgba"),
        ],
    );
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("compiles");
    let w = &compiled.stroke_wgsl;
    assert!(
        w.contains("dab_local"),
        "Dab mode must emit the oriented basis local"
    );
    assert!(
        w.contains("cos(") && w.contains("sin("),
        "Dab basis must rotate by the rotation input"
    );
    assert!(
        w.contains("* 64.0"),
        "Dab mode must fold the per-dab variation offset"
    );
    assert!(
        !w.contains("target_pos / 32.000000"),
        "Dab mode must not fall back to the canvas coordinate",
    );
    naga_validate(w, "noise dab stroke");
    naga_validate(&compiled.cursor_preview_wgsl, "noise dab preview");
}

#[test]
fn noise_scale_with_brush_picks_arm_at_compile_time() {
    let reg = registry();
    for (swb, expect_norm) in [(true, true), (false, false)] {
        let mut graph = Graph::<BrushWireType>::new();
        let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
        let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
        apply_noise_inputs(&mut graph, &noise, 1, swb);
        let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
        wire(
            &mut graph,
            &[
                (pen.clone(), "position", term.clone(), "position"),
                (noise.clone(), "color", term.clone(), "rgba"),
            ],
        );
        let plan = compile(&graph, reg.as_map()).unwrap();
        let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("compiles");
        let w = &compiled.stroke_wgsl;
        if expect_norm {
            assert!(
                w.contains("dab_local / (32.000000)"),
                "scale_with_brush=true divides the unit-disc offset"
            );
            // The skeleton always defines `local_uv = local * d.inv_radius_target_px`,
            // so guard against the *reconstruction* specifically, not the symbol.
            assert!(
                !w.contains("1.0 / d.inv_radius_target_px"),
                "scale_with_brush=true must not reconstruct pixels"
            );
        } else {
            assert!(
                w.contains("1.0 / d.inv_radius_target_px"),
                "scale_with_brush=false must reconstruct dab-pixels from inv_radius",
            );
        }
    }
}

#[test]
fn noise_rotation_input_wires_per_dab() {
    // Wiring pen.drawing_angle → noise.rotation must substitute the per-dab
    // drawing-angle expression into the oriented basis (not a literal),
    // proving orientation flows through the ordinary input-port path.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
    apply_noise_inputs(&mut graph, &noise, 1, true);
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    wire(
        &mut graph,
        &[
            (pen.clone(), "position", term.clone(), "position"),
            (pen.clone(), "drawing_angle", noise.clone(), "rotation"),
            (noise.clone(), "color", term.clone(), "rgba"),
        ],
    );
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("compiles");
    assert!(
        compiled
            .stroke_wgsl
            .contains("_drawing_angle - u.intrinsic.view_rotation"),
        "noise.rotation must carry the wired per-dab drawing_angle expression",
    );
    naga_validate(&compiled.stroke_wgsl, "noise wired-rotation stroke");
    naga_validate(
        &compiled.cursor_preview_wgsl,
        "noise wired-rotation preview",
    );
}

#[test]
fn image_dab_tip_needs_no_shape_node() {
    // The finding-#2 case: an image tip in its default Dab space, oriented by
    // pen.drawing_angle, with NO circle node present. Both shader variants must
    // compile and sample in the oriented frame.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let image = graph.add_node("image", reg.get("image").unwrap().ports.clone());
    for (name, v) in [
        ("texture_name", InputValue::String("paper".into())),
        ("scale", InputValue::Scalar(512.0)),
        ("space", InputValue::Int(1)),
        ("scale_with_brush", InputValue::Bool(true)),
    ] {
        graph.set_port_value(&image, name, v).unwrap();
    }
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    wire(
        &mut graph,
        &[
            (pen.clone(), "position", term.clone(), "position"),
            (pen.clone(), "drawing_angle", image.clone(), "rotation"),
            (image.clone(), "color", term.clone(), "rgba"),
        ],
    );
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("compiles");
    for (label, w) in [
        ("stroke_wgsl", &compiled.stroke_wgsl),
        ("cursor_preview_wgsl", &compiled.cursor_preview_wgsl),
    ] {
        assert!(
            w.contains("@fragment") && w.contains("fn fs_main"),
            "{label} must be a complete fragment shader"
        );
        assert!(
            w.contains("dab_local"),
            "{label} image tip must sample the oriented Dab frame"
        );
        assert!(
            w.contains("_drawing_angle - u.intrinsic.view_rotation"),
            "{label} image.rotation must be the wired drawing_angle"
        );
    }
    naga_validate(&compiled.stroke_wgsl, "image dab-tip stroke");
    naga_validate(&compiled.cursor_preview_wgsl, "image dab-tip preview");
}
