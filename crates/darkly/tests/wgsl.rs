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
use darkly::brush::texture_source::{BakeChannels, ResolvedSource};
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
        compiled.graph_sources.is_empty(),
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

/// With a static field, the noise node **bakes**: its `color` output compiles
/// to a single `textureSample` of a cached RGBA tile — not three per-fragment
/// `fbm_tile` calls. This is the complexity-class win: the ~80-hash kernel runs
/// once per texel at bake time, not per fragment per overlapping dab. The 3D
/// noise lib path must not leak in, and the shader validates under naga.
#[test]
fn noise_static_color_bakes_to_single_sample() {
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

    // Baked: no per-fragment fBm call site (the lib's `fn fbm_tile`
    // definition is always concatenated, so match the call prefix).
    assert!(
        !compiled.stroke_wgsl.contains("fbm_tile(noise"),
        "static noise must bake, not call fbm_tile; found it in:\n{}",
        compiled.stroke_wgsl,
    );
    let samples = compiled
        .stroke_wgsl
        .matches("textureSampleLevel(graph_tex_")
        .count();
    assert_eq!(samples, 1, "baked color is a single RGBA tile sample");
    // The slot is a Baked RGBA source.
    assert_eq!(compiled.graph_sources.len(), 1);
    assert!(
        matches!(
            &compiled.graph_sources[0],
            ResolvedSource::Baked(spec) if spec.channels == BakeChannels::Rgba
        ),
        "color output must bake an RGBA tile, got {:?}",
        compiled.graph_sources,
    );
    // The 2D/3D lib split must not leak the 3D texture path in.
    assert!(
        !compiled.stroke_wgsl.contains("texture_3d")
            && !compiled.stroke_wgsl.contains("fbm_noise3d"),
        "brush shader must not include the 3D fbm bindings",
    );
    naga_validate(&compiled.stroke_wgsl, "noise stroke_wgsl");
    naga_validate(&compiled.cursor_preview_wgsl, "noise cursor_preview_wgsl");
}

/// A wired field input (here `octaves`, driven per-dab) can't be baked, so the
/// node falls back to the live per-fragment `fbm_tile` kernel and requests no
/// baked source. This is the other half of the static-vs-wired gate.
#[test]
fn noise_wired_field_falls_back_to_live_kernel() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let rand = graph.add_node("random", reg.get("random").unwrap().ports.clone());
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    let wires = [
        (pen.clone(), "position", term.clone(), "position"),
        (rand.clone(), "value", noise.clone(), "octaves"),
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
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("wired noise compiles");

    assert!(
        compiled.stroke_wgsl.contains("fbm_tile(noise"),
        "wired-field noise must emit the live fBm kernel",
    );
    assert!(
        compiled.graph_sources.is_empty(),
        "wired-field noise must not bake a tile, got {:?}",
        compiled.graph_sources,
    );
    naga_validate(&compiled.stroke_wgsl, "wired noise stroke_wgsl");
}

/// A consumer that wires only the scalar `value` output bakes a single
/// **grayscale** (R8) tile and samples it once — no chromatic RGBA tile, no
/// per-fragment fBm. This is the monochrome grain path a brush like `sponge`
/// takes.
#[test]
fn noise_static_value_bakes_grayscale() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
    graph
        .set_port_value(&noise, "seed", InputValue::Int(7))
        .unwrap();
    let paint_color = graph.add_node("paint_color", reg.get("paint_color").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    // `noise.value` drives a scalar terminal input (`flow`); `color` is left
    // unconsumed so only the monochrome path should be emitted.
    let wires = [
        (pen.clone(), "position", term.clone(), "position"),
        (paint_color.clone(), "color", term.clone(), "rgba"),
        (noise.clone(), "value", term.clone(), "flow"),
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
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("noise value compiles");

    // Baked: one grayscale tile, one sample, no live fBm kernel.
    assert!(!compiled.stroke_wgsl.contains("fbm_tile(noise"));
    let samples = compiled
        .stroke_wgsl
        .matches("textureSampleLevel(graph_tex_")
        .count();
    assert_eq!(samples, 1, "value-only noise samples one baked tile");
    assert_eq!(compiled.graph_sources.len(), 1);
    assert!(
        matches!(
            &compiled.graph_sources[0],
            ResolvedSource::Baked(spec) if spec.channels == BakeChannels::Grayscale
        ),
        "value output must bake a grayscale tile, got {:?}",
        compiled.graph_sources,
    );
    naga_validate(&compiled.stroke_wgsl, "noise value stroke_wgsl");
    naga_validate(
        &compiled.cursor_preview_wgsl,
        "noise value cursor_preview_wgsl",
    );
}

/// When a graph consumes **both** `value` and `color`, two tiles bake (one
/// grayscale, one RGBA) and both sample off the **one** shared coordinate
/// binding — no duplicated coordinate setup.
#[test]
fn noise_both_outputs_share_one_coord() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
    graph
        .set_port_value(&noise, "seed", InputValue::Int(7))
        .unwrap();
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    let wires = [
        (pen.clone(), "position", term.clone(), "position"),
        (noise.clone(), "color", term.clone(), "rgba"),
        (noise.clone(), "value", term.clone(), "flow"),
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
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals()).expect("noise both compiles");

    // Two baked tiles (grayscale + RGBA), each sampled once, off one coord.
    assert!(!compiled.stroke_wgsl.contains("fbm_tile(noise"));
    let samples = compiled
        .stroke_wgsl
        .matches("textureSampleLevel(graph_tex_")
        .count();
    assert_eq!(samples, 2, "value + color sample two baked tiles");
    let coords = compiled.stroke_wgsl.matches("_p = ").count();
    assert_eq!(
        coords, 1,
        "the sampled coordinate must be bound once and shared, found {coords} in:\n{}",
        compiled.stroke_wgsl,
    );
    assert_eq!(compiled.graph_sources.len(), 2, "one grayscale + one RGBA");
    let has_gray = compiled.graph_sources.iter().any(
        |s| matches!(s, ResolvedSource::Baked(spec) if spec.channels == BakeChannels::Grayscale),
    );
    let has_rgba = compiled
        .graph_sources
        .iter()
        .any(|s| matches!(s, ResolvedSource::Baked(spec) if spec.channels == BakeChannels::Rgba));
    assert!(has_gray && has_rgba, "both a grayscale and an RGBA tile");
    naga_validate(&compiled.stroke_wgsl, "noise both stroke_wgsl");
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
        assert!(
            w.contains("_tinv: mat2x2"),
            "{label} must build the inverse squeeze transform that places the polygon vertices",
        );
        assert!(
            w.contains("_beta: f32 ="),
            "{label} must fold in the independent squeeze angle",
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
    // Unsqueezed (a=1, no rotation) the exact SDF is the regular n-gon: a vertex
    // on +y at the circumradius, the edge midpoint at the apothem, negative
    // inside, and rounding morphs it toward the unit disc while staying within
    // circumradius 1 (the constant extent bound).
    let sd = |p: [f32; 2], n: f32, round: f32| poly_sd_exact(p, n, round, 1.0, 0.0, 0.0);

    for &n in &[3.0_f32, 4.0, 5.0, 6.0, 12.0] {
        let an = std::f32::consts::PI / n;
        let apothem = an.cos();
        // A vertex points straight up (+y); the edge midpoint bisects the
        // sector at angle `an` from +y.
        let vertex_dir = [0.0_f32, 1.0];
        let edge_dir = [an.sin(), an.cos()];

        // Sharp polygon (ρ = 0): negative inside, vertex on the unit circle,
        // edge midpoint on the boundary at the apothem.
        assert!(sd([0.0, 0.0], n, 0.0) < 0.0, "n={n}: centre must be inside");
        assert!(
            sd(vertex_dir, n, 0.0).abs() < 2e-3,
            "n={n}: a vertex sits on the unit circumcircle (sd≈0)",
        );
        let edge_mid = [edge_dir[0] * apothem, edge_dir[1] * apothem];
        assert!(
            sd(edge_mid, n, 0.0).abs() < 2e-3,
            "n={n}: the edge midpoint sits on the boundary at the apothem",
        );
        // A radius-1 point in the edge direction is outside the sharp polygon.
        assert!(
            sd(edge_dir, n, 0.0) > 1e-3,
            "n={n}: radius-1 in the edge direction is outside the sharp polygon",
        );

        // Rounding morphs toward the unit disc: at ρ = 1 the base polygon
        // collapses to a point and the field is exactly `|p| - 1`.
        for i in 0..16 {
            let a = (i as f32) * std::f32::consts::TAU / 16.0;
            let p = [0.5 * a.cos(), 0.5 * a.sin()];
            assert!(
                (sd(p, n, 1.0) - (-0.5)).abs() < 2e-3,
                "n={n}: at ρ=1 the field is the unit disc (sd = |p| - 1)",
            );
        }

        // The circumradius bound `1.0` holds for every rounding: no point
        // beyond radius 1 is ever inside.
        for &round in &[0.0_f32, 0.5, 1.0] {
            for i in 0..64 {
                let a = (i as f32) * std::f32::consts::TAU / 64.0;
                let p = [1.04 * a.cos(), 1.04 * a.sin()];
                assert!(
                    sd(p, n, round) > 0.0,
                    "n={n} ρ={round}: nothing past the circumradius may be inside",
                );
            }
        }
    }
}

/// Regression: the polygon softness feather must stay *inside* the boundary
/// (the circumradius), like the circle family's `shape_coverage`. An earlier
/// symmetric feather (`1 - smoothstep(-band, band, sd)`) bloomed *outward* past
/// the circumradius, where the skeleton's disc clip (`local_dist >=
/// bbox_target_px`) hard-cut it — filling the gaps between the vertices out to
/// the circumcircle and flattening a soft polygon into a plain circle. The
/// inward feather has zero coverage the moment `sd >= 0`, so nothing ever
/// reaches the clip.
#[test]
fn polygon_softness_feathers_inward() {
    // Mirror of the node's emitted coverage: `smoothstep(0.0, band, -sd)`.
    fn coverage(sd: f32, softness: f32) -> f32 {
        let band = softness.clamp(0.0, 1.0).max(0.004);
        let t = ((-sd) / band).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
    for &softness in &[0.0_f32, 0.5, 1.0] {
        // Outside the boundary (sd > 0) coverage is exactly 0 for any softness —
        // the feather never blooms past the circumradius into the disc clip.
        assert_eq!(
            coverage(0.01, softness),
            0.0,
            "softness={softness}: no coverage outside the boundary",
        );
        // The soft edge is entirely inside: ~0 at the boundary, full deep inside.
        assert!(
            coverage(-1e-4, softness) < 0.5,
            "softness={softness}: coverage fades to ~0 at the boundary",
        );
        assert!(
            coverage(-1.0, softness) > 0.99,
            "softness={softness}: full coverage well inside the boundary",
        );
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

/// The polygon node folds `squeeze` into its footprint: a wired `squeeze`
/// (natural-range max 1.0 ⇒ semi-axis 0.1) must inflate the dab bbox ×10 so a
/// thin nib isn't clipped on save-point rewind.
#[test]
fn polygon_extent_grows_with_squeeze() {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    let paint_color = graph.add_node("paint_color", reg.get("paint_color").unwrap().ports.clone());
    let rand_squeeze = graph.add_node("random", reg.get("random").unwrap().ports.clone());
    let poly = graph.add_node("polygon", reg.get("polygon").unwrap().ports.clone());
    let stamp = graph.add_node("stamp", reg.get("stamp").unwrap().ports.clone());
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    wire(
        &mut graph,
        &[
            (rand_squeeze.clone(), "value", poly.clone(), "squeeze"),
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
        "wired squeeze (max 1.0) must inflate the bbox ×10, got {}",
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

// ---------------------------------------------------------------------------
// Rust mirror of the polygon node's emitted coverage: the exact screen-space
// signed distance to the squeezed polygon, `poly_sd_exact`.
// ---------------------------------------------------------------------------

/// Map a base circumradius-`r` vertex from the polygon's own frame into screen
/// space: `screen = R(β−φ)·diag(a,1/a)·R(−β)·p` — the inverse of the node's
/// forward squeeze transform.
fn poly_to_screen(p: [f32; 2], a: f32, phi: f32, beta: f32) -> [f32; 2] {
    let (cb, sb) = (beta.cos(), beta.sin());
    let r1 = [p[0] * cb + p[1] * sb, -p[0] * sb + p[1] * cb]; // R(−β)·p
    let d = [r1[0] * a, r1[1] / a]; // diag(a, 1/a)
    let ang = beta - phi;
    let (c, s) = (ang.cos(), ang.sin());
    [d[0] * c - d[1] * s, d[0] * s + d[1] * c] // R(β−φ)
}

/// Mirror of the node's emitted exact polygon SDF: generate the (squeezed)
/// polygon's screen-space vertices, take the min distance to its edges with a
/// winding sign, then round by `round`. Negative inside; a true Euclidean field.
fn poly_sd_exact(uv: [f32; 2], n: f32, round: f32, a: f32, phi: f32, beta: f32) -> f32 {
    let aa = a.clamp(0.01, 1.0);
    let cr = 1.0 - round;
    let ni = n.max(3.0) as i32;
    let vert = |k: i32| {
        let ak = std::f32::consts::TAU * (k as f32) / n;
        poly_to_screen([cr * ak.sin(), cr * ak.cos()], aa, phi, beta)
    };
    let mut vj = vert(ni - 1);
    let mut d2 = f32::INFINITY;
    let mut s = 1.0_f32;
    for i in 0..ni {
        let vi = vert(i);
        let e = [vj[0] - vi[0], vj[1] - vi[1]];
        let w = [uv[0] - vi[0], uv[1] - vi[1]];
        let t =
            ((w[0] * e[0] + w[1] * e[1]) / (e[0] * e[0] + e[1] * e[1]).max(1e-12)).clamp(0.0, 1.0);
        let b = [w[0] - e[0] * t, w[1] - e[1] * t];
        d2 = d2.min(b[0] * b[0] + b[1] * b[1]);
        let c0 = uv[1] >= vi[1];
        let c1 = uv[1] < vj[1];
        let c2 = e[0] * w[1] > e[1] * w[0];
        if (c0 && c1 && c2) || (!c0 && !c1 && !c2) {
            s = -s;
        }
        vj = vi;
    }
    s * d2.sqrt() - round
}

/// Feature: the exact Euclidean distance makes the softness band a uniform
/// screen-space width on every edge under squeeze. Band width perpendicular to
/// an edge is `band / |∇_screen sd|`, so isotropy ⇔ `|∇_screen sd| ≈ 1`. We march
/// to the boundary along many directions and measure the screen-space gradient
/// there; an exact SDF holds it ≈1 everywhere (dipping only at the measure-zero
/// corner points).
#[test]
fn polygon_softness_band_isotropic_under_squeeze() {
    let n = 6.0_f32;
    let a = 0.35_f32; // squeezed semi-axis
    let phi = 0.3_f32; // arbitrary non-zero rotation
    let beta = 0.0_f32;

    fn grad_mag(field: &dyn Fn([f32; 2]) -> f32, uv: [f32; 2]) -> f32 {
        let e = 2e-3;
        let dx = field([uv[0] + e, uv[1]]) - field([uv[0] - e, uv[1]]);
        let dy = field([uv[0], uv[1] + e]) - field([uv[0], uv[1] - e]);
        (dx * dx + dy * dy).sqrt() / (2.0 * e)
    }
    let field = |uv: [f32; 2]| poly_sd_exact(uv, n, 0.0, a, phi, beta);

    let mut grads = Vec::new();
    for i in 0..180 {
        let ang = (i as f32) * std::f32::consts::TAU / 180.0;
        let dir = [ang.cos(), ang.sin()];
        let (mut lo, mut hi) = (0.0_f32, 4.0_f32);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if field([dir[0] * mid, dir[1] * mid]) < 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let r = 0.5 * (lo + hi);
        grads.push(grad_mag(&field, [dir[0] * r, dir[1] * r]));
    }

    grads.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let median = grads[grads.len() / 2];
    assert!(
        (median - 1.0).abs() < 0.02,
        "exact SDF should give a unit-gradient (uniform) band; median = {median}",
    );
    let tight =
        grads.iter().filter(|&&x| (x - 1.0).abs() < 0.1).count() as f32 / grads.len() as f32;
    assert!(
        tight > 0.9,
        "the band should be uniform on essentially every edge; tight fraction = {tight}",
    );
}

/// Feature: `squeeze_angle` aims the squash independently of `rotation`. With
/// rotation held fixed, the squeezed (narrow) axis of the tip should rotate with
/// the squeeze angle — so which of the +x / +y boundary radii is shorter flips
/// as the angle goes 0 → 90°.
#[test]
fn polygon_squeeze_angle_steers_independently() {
    let n = 6.0_f32;
    let a = 0.4_f32; // squeezed
    let phi = 0.0_f32; // rotation fixed

    fn radius(field: &dyn Fn([f32; 2]) -> f32, dir: [f32; 2]) -> f32 {
        let (mut lo, mut hi) = (0.0_f32, 5.0_f32);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if field([dir[0] * mid, dir[1] * mid]) < 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }
    let along = |beta: f32, dir: [f32; 2]| {
        radius(&|uv: [f32; 2]| poly_sd_exact(uv, n, 0.0, a, phi, beta), dir)
    };

    // β = 0: squeeze along the shape's x-axis → tip narrow in x, wide in y.
    assert!(
        along(0.0, [1.0, 0.0]) < 0.8 * along(0.0, [0.0, 1.0]),
        "at squeeze_angle=0 the tip should be narrow along x",
    );
    // β = 90°: the squeeze axis rotates to y (rotation unchanged) → now narrow in y.
    let b = std::f32::consts::FRAC_PI_2;
    assert!(
        along(b, [0.0, 1.0]) < 0.8 * along(b, [1.0, 0.0]),
        "squeeze_angle should steer the squash independently of rotation \
         (narrow axis rotates from x to y)",
    );
}

/// Feature (the reported bug): squeezing a square and rounding it must produce a
/// proper rounded rectangle — a **convex** outline — not a shape whose corners
/// balloon outward. We march the boundary all the way around and assert the
/// turning is monotone (every consecutive cross-product keeps one sign), which a
/// ballooned, non-convex corner would violate.
#[test]
fn polygon_rounded_rectangle_stays_convex() {
    let n = 4.0_f32;
    let round = 0.4_f32;
    let a = 0.35_f32; // heavily squeezed square → rectangle
    let phi = 0.0_f32;
    let beta = 0.0_f32;
    let field = |uv: [f32; 2]| poly_sd_exact(uv, n, round, a, phi, beta);

    // Boundary point along a ray from the (interior) origin.
    let boundary = |dir: [f32; 2]| {
        let (mut lo, mut hi) = (0.0_f32, 6.0_f32);
        for _ in 0..48 {
            let mid = 0.5 * (lo + hi);
            if field([dir[0] * mid, dir[1] * mid]) < 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let r = 0.5 * (lo + hi);
        [dir[0] * r, dir[1] * r]
    };
    let m = 240;
    let pts: Vec<[f32; 2]> = (0..m)
        .map(|i| {
            let ang = (i as f32) * std::f32::consts::TAU / (m as f32);
            boundary([ang.cos(), ang.sin()])
        })
        .collect();

    // Signed area (shoelace) is positive for a CCW simple polygon; the boundary
    // points are generated in CCW angular order, so use it to fix the expected
    // turn sign, then assert every corner turns the same way (convex).
    let mut area = 0.0_f32;
    for i in 0..m {
        let p = pts[i];
        let q = pts[(i + 1) % m];
        area += p[0] * q[1] - q[0] * p[1];
    }
    let want = area.signum();
    for i in 0..m {
        let a0 = pts[i];
        let a1 = pts[(i + 1) % m];
        let a2 = pts[(i + 2) % m];
        let e0 = [a1[0] - a0[0], a1[1] - a0[1]];
        let e1 = [a2[0] - a1[0], a2[1] - a1[1]];
        let cross = e0[0] * e1[1] - e0[1] * e1[0];
        assert!(
            cross * want >= -1e-4,
            "rounded rectangle must stay convex (no ballooning corner); \
             reversed turn at point {i}: cross={cross}",
        );
    }
}

/// Feature invariant: for every squeeze / angle / rounding, nothing beyond the
/// tip's screen-space footprint bound (`1/a`, matching `extent()`) is inside —
/// the rounding never grows the tip past its budgeted extent.
#[test]
fn polygon_within_extent_bound() {
    for &n in &[3.0_f32, 4.0, 5.0, 6.0] {
        for &a in &[0.2_f32, 0.5, 1.0] {
            for &round in &[0.0_f32, 0.5, 1.0] {
                for &phi in &[0.0_f32, 0.7] {
                    for &beta in &[0.0_f32, 0.9] {
                        let bound = 1.0 / a;
                        for i in 0..96 {
                            let ang = (i as f32) * std::f32::consts::TAU / 96.0;
                            // Just outside the footprint bound (+2%).
                            let r = bound * 1.02;
                            let uv = [r * ang.cos(), r * ang.sin()];
                            assert!(
                                poly_sd_exact(uv, n, round, a, phi, beta) > 0.0,
                                "n={n} a={a} round={round} phi={phi} beta={beta}: \
                                 nothing past the extent bound may be inside",
                            );
                        }
                    }
                }
            }
        }
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
        compiled.graph_sources.len(),
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
/// (i32, not u32 — `fbm_tile`'s octave arg is i32) and validate on both
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
    // fold the per-dab `variation` offset (a 2D hash of `variation` bounded to
    // the field period). With both inputs unwired they fall to literal defaults
    // (0), so the basis and offset still appear.
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
        w.contains("fbm_offset2(u32(max(") && w.contains("* 4096.0), 128.000000)"),
        "Dab mode must fold the per-dab variation offset via the 2D hash \
         bounded to the noise field period (FIELD_SPAN = 128)"
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
