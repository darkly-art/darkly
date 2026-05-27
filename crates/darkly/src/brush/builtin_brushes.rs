//! Built-in brushes shipped with the application.
//!
//! Each brush is a programmatically constructed node graph wrapped in a
//! `Brush`. All brushes are inserted into the `BrushLibrary` at engine
//! startup.

use crate::brush::bundle::{Brush, BrushMetadata};
use crate::brush::wire::BrushWireType;
use crate::brush::BrushNodeRegistry;
use crate::gpu::params::ParamValue;
use crate::nodegraph::{Graph, NodeId, PortRef};

/// Return all built-in brushes.
pub fn all() -> Vec<Brush> {
    vec![
        round(),
        airbrush(),
        ink_pen(),
        smooth_watercolor(),
        rough_watercolor(),
        smudge_brush(),
        liquify_push(),
        rough_ink(),
    ]
}

// ---------------------------------------------------------------------------
// Brush definitions
// ---------------------------------------------------------------------------

/// Build a Basic brush around the compiled `paint_compiled` terminal.
///
/// All three Basic brushes (Round, Airbrush, Ink Pen) share the same
/// `pen_input + paint_color + circle + stamp + paint_compiled` skeleton
/// — the same shape as Rough Ink — and only differ in their per-brush
/// signal wires (pressure → flow vs opacity, optional pressure curve)
/// and the circle softness default. The closure runs after the bare
/// graph is built and is responsible for wiring the brush-specific
/// signal flow and setting any per-port defaults.
///
/// The `circle` runs in sine algorithm with amplitude 0, producing a
/// plain disc — its only role here is to be the softness-aware shape
/// mask. Per-brush softness lives on `circle.softness` (the compiled
/// terminal has no softness port).
fn paint_brush(
    name: &str,
    configure: impl FnOnce(
        &mut Graph<BrushWireType>,
        NodeId, // pen
        NodeId, // paint_color
        NodeId, // circle
        NodeId, // stamp
        NodeId, // terminal
    ),
) -> Brush {
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
    let circle = graph.add_node(
        "circle",
        registry.get("circle").unwrap().ports.clone(),
        vec![ParamValue::Int(0)], // 0 = Sine Harmonic; amplitude 0 → plain disc
    );
    let stamp = graph.add_node(
        "stamp",
        registry.get("stamp").unwrap().ports.clone(),
        vec![ParamValue::Int(0)], // 0 = Alpha Mask
    );
    // `paint_compiled` owns the dab dimensions; stamp's `size` port is
    // inert in the compiled execution model. Hide it so users don't see
    // two "Size" sliders in the brush properties panel.
    graph.set_port_exposed(stamp, "size", false).unwrap();
    let terminal = graph.add_node(
        "paint_compiled",
        registry.get("paint_compiled").unwrap().ports.clone(),
        vec![],
    );

    // Shared shape: pen position drives terminal position; paint_color
    // flows through stamp.color (the compiled terminal has no `color`
    // port — color is folded into `rgba` by the stamp); circle is the
    // tip mask for stamp; stamp's premultiplied RGBA is the terminal's
    // input.
    let shared_wires = [
        (pen, "position", terminal, "position"),
        (paint_color, "color", stamp, "color"),
        (circle, "texture", stamp, "tip"),
        (stamp, "dab", terminal, "rgba"),
    ];
    for (from_node, from_port, to_node, to_port) in shared_wires {
        graph
            .connect(
                PortRef {
                    node: from_node,
                    port: from_port.into(),
                },
                PortRef {
                    node: to_node,
                    port: to_port.into(),
                },
            )
            .unwrap();
    }

    configure(&mut graph, pen, paint_color, circle, stamp, terminal);

    let mut metadata = BrushMetadata::from_graph(name, graph);
    metadata.category = "basic".to_string();
    Brush::without_resources(metadata)
}

/// Wire `pen.pressure → curve → terminal.size_input` with the given
/// initial curve points. Helper for Basic brushes that want a
/// user-tunable pressure-to-size transfer function.
fn wire_pressure_size_curve(
    graph: &mut Graph<BrushWireType>,
    pen: NodeId,
    terminal: NodeId,
    points: Vec<[f32; 2]>,
) {
    let registry = BrushNodeRegistry::new();
    let curve = graph.add_node(
        "curve",
        registry.get("curve").unwrap().ports.clone(),
        vec![ParamValue::Curve(points)],
    );
    for (from_node, from_port, to_node, to_port) in [
        (pen, "pressure", curve, "input"),
        (curve, "output", terminal, "size_input"),
    ] {
        graph
            .connect(
                PortRef {
                    node: from_node,
                    port: from_port.into(),
                },
                PortRef {
                    node: to_node,
                    port: to_port.into(),
                },
            )
            .unwrap();
    }
}

/// Round — soft procedural disc, pressure-driven size + flow, identity
/// pressure curve so the brush feels predictable out of the box.
fn round() -> Brush {
    paint_brush(
        "Round",
        |graph, pen, _paint_color, circle, stamp, terminal| {
            // Identity curve so pressure maps 1:1 to size by default — the user
            // can still scrub the curve node's spline in the brush editor for a
            // bespoke response.
            wire_pressure_size_curve(graph, pen, terminal, vec![[0.0, 0.0], [1.0, 1.0]]);
            // Pressure → flow via stamp (folds into per-dab color alpha;
            // `paint_compiled.flow` stays at its 1.0 default).
            graph
                .connect(
                    PortRef {
                        node: pen,
                        port: "pressure".into(),
                    },
                    PortRef {
                        node: stamp,
                        port: "flow".into(),
                    },
                )
                .unwrap();
            // Mid-softness — a sensible default between the harder Ink Pen and
            // the fully-feathered Airbrush.
            graph.set_port_default(circle, "softness", 0.5).unwrap();
            graph.set_port_exposed(circle, "softness", true).unwrap();
        },
    )
}

/// Airbrush — fully-soft disc, pressure-driven *opacity* (not flow).
/// Build-up-with-pressure semantic: every dab deposits at full flow into
/// the scratch, but the per-event opacity cap (driven by current pressure)
/// attenuates the commit, so light strokes layer up gradually as the user
/// passes back over the same area. Pressure does NOT affect dab radius —
/// `size_input` is left at its 100% port default (no pen wire) so the
/// airbrush footprint is a fixed soft disc the user controls only via the
/// Size slider.
fn airbrush() -> Brush {
    paint_brush(
        "Airbrush",
        |graph, pen, _paint_color, circle, _stamp, terminal| {
            graph
                .connect(
                    PortRef {
                        node: pen,
                        port: "pressure".into(),
                    },
                    PortRef {
                        node: terminal,
                        port: "flow".into(),
                    },
                )
                .unwrap();
            graph.set_port_default(circle, "softness", 1.0).unwrap();
            graph.set_port_exposed(circle, "softness", true).unwrap();
        },
    )
}

/// Ink Pen — harder edge than Round, pressure-driven size through a
/// front-loaded curve (high size at low pressure) and pressure-driven
/// flow. Stabilizer exposed for clean line work.
fn ink_pen() -> Brush {
    paint_brush(
        "Ink Pen",
        |graph, pen, _paint_color, circle, stamp, terminal| {
            // Curve front-loads the size response — small pressure already
            // produces a recognisable mark, matching the feel of a fine-tipped
            // ink pen.
            // One bend handle above the diagonal — the natural cubic spline
            // draws a smooth √x-ish arc through it. Matches the "soft tip
            // feel" curve tablet drivers and inking presets converge on:
            // light pressure already produces a recognisable mark.
            wire_pressure_size_curve(
                graph,
                pen,
                terminal,
                vec![[0.0, 0.0], [0.4, 0.7], [1.0, 1.0]],
            );
            graph
                .connect(
                    PortRef {
                        node: pen,
                        port: "pressure".into(),
                    },
                    PortRef {
                        node: stamp,
                        port: "flow".into(),
                    },
                )
                .unwrap();
            // Harder edge than Round — matches the prior `paint`-terminal
            // softness default of 0.1.
            graph.set_port_default(circle, "softness", 0.1).unwrap();
            graph.set_port_exposed(circle, "softness", true).unwrap();
            // Stabilization exposed to the toolbar (matches prior ink-pen
            // behavior) — line work benefits more than soft-edged brushes.
            graph.set_port_default(pen, "stabilize", 0.6).unwrap();
            graph.set_port_exposed(pen, "stabilize", true).unwrap();
        },
    )
}

/// Build a Wet Media (watercolor) brush around the compiled
/// `watercolor_compiled` terminal.
///
/// All watercolor variants share the same shape — `pen_input +
/// paint_color + circle → watercolor_compiled` — and only differ in
/// the circle's algorithm + default shape params, plus the per-dab
/// modulation (random rotation for sine, random seed for perlin).
/// The closure receives the `circle` node so it can set its
/// algorithm/defaults and wire per-dab `random` sources to its
/// `phase_input` or `seed` ports.
fn watercolor_brush(
    name: &str,
    configure: impl FnOnce(
        &mut Graph<BrushWireType>,
        NodeId, // pen
        NodeId, // paint_color
        NodeId, // circle
        NodeId, // terminal
    ),
) -> Brush {
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::<BrushWireType>::new();

    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    // Stabilization: stroke smoothing helps watercolor read as a single
    // continuous wash rather than a jittery line. 50% is enough to take
    // the edge off without the brush feeling laggy.
    graph.set_port_default(pen, "stabilize", 0.5).unwrap();
    graph.set_port_exposed(pen, "stabilize", true).unwrap();

    let paint_color = graph.add_node(
        "paint_color",
        registry.get("paint_color").unwrap().ports.clone(),
        vec![],
    );
    let circle = graph.add_node(
        "circle",
        registry.get("circle").unwrap().ports.clone(),
        vec![ParamValue::Int(0)], // algorithm — closure overwrites
    );
    // Soft edge by default — watercolor reads as a wash, not a
    // hard-edged stamp. Variants can override via `configure`.
    graph.set_port_default(circle, "softness", 0.2).unwrap();
    graph.set_port_exposed(circle, "softness", true).unwrap();
    let terminal = graph.add_node(
        "watercolor_compiled",
        registry.get("watercolor_compiled").unwrap().ports.clone(),
        vec![],
    );

    // Pressure → flow folds into the per-dab color alpha (max-deposit
    // ceiling). Color and shape feed the terminal directly — the
    // compiled fragment shader inlines the shape evaluator and the
    // color is a stroke-constant uniform reference.
    let wires = [
        (pen, "position", terminal, "position"),
        (pen, "pressure", terminal, "flow"),
        (paint_color, "color", terminal, "color"),
        (circle, "texture", terminal, "mask"),
    ];
    for (from_node, from_port, to_node, to_port) in wires {
        graph
            .connect(
                PortRef {
                    node: from_node,
                    port: from_port.into(),
                },
                PortRef {
                    node: to_node,
                    port: to_port.into(),
                },
            )
            .unwrap();
    }

    configure(&mut graph, pen, paint_color, circle, terminal);

    let mut metadata = BrushMetadata::from_graph(name, graph);
    metadata.category = "wet_media".to_string();
    Brush::without_resources(metadata)
}

/// Smooth watercolor: sine-harmonic dab with gentle bumps for an organic
/// hand-painted edge.
fn smooth_watercolor() -> Brush {
    watercolor_brush(
        "Smooth Watercolor",
        |graph, _pen, _paint_color, circle, _terminal| {
            graph
                .set_param(circle, 0, ParamValue::Int(0)) // 0 = Sine Harmonic
                .unwrap();
            graph.set_port_default(circle, "amplitude", 0.05).unwrap();
            graph.set_port_default(circle, "frequency", 5.0).unwrap();
            graph.set_port_default(circle, "phase", 0.0).unwrap();
            // Smooth watercolor leans on a softer edge than the shared
            // default to keep the harmonic bumps reading as a wash.
            graph.set_port_default(circle, "softness", 0.5).unwrap();

            // Per-dab random rotation so the harmonic bumps land at a
            // fresh angle every stamp — without it, every dab is
            // identical and the bumps line up along the stroke.
            // (Rough watercolor doesn't need this because its per-dab
            // seed gives a fresh noise pattern, not just a fresh
            // rotation of the same pattern.)
            let registry = BrushNodeRegistry::new();
            let rand_rot = graph.add_node(
                "random",
                registry.get("random").unwrap().ports.clone(),
                vec![ParamValue::Int(0)], // 0 = per-dab
            );
            graph
                .connect(
                    PortRef {
                        node: rand_rot,
                        port: "value".into(),
                    },
                    PortRef {
                        node: circle,
                        port: "phase_input".into(),
                    },
                )
                .unwrap();
        },
    )
}

/// Rough watercolor: Perlin-noise dab with a more chaotic, granulated edge.
fn rough_watercolor() -> Brush {
    watercolor_brush(
        "Rough Watercolor",
        |graph, _pen, _paint_color, circle, _terminal| {
            graph
                .set_param(circle, 0, ParamValue::Int(1)) // 1 = Perlin Noise
                .unwrap();
            graph.set_port_default(circle, "softness", 0.05).unwrap();
            graph.set_port_default(circle, "amplitude", 0.4).unwrap();
            graph.set_port_default(circle, "frequency", 12.0).unwrap();
            graph.set_port_default(circle, "persistence", 0.55).unwrap();
            graph.set_port_default(circle, "octaves", 4.0).unwrap();
            // Per-dab random seed so every dab gets a fresh Perlin
            // pattern. Full per-dab noise reshuffle subsumes what a
            // rotation-randomizer would add.
            let registry = BrushNodeRegistry::new();
            let rand_seed = graph.add_node(
                "random",
                registry.get("random").unwrap().ports.clone(),
                vec![ParamValue::Int(0)], // 0 = per-dab
            );
            graph
                .connect(
                    PortRef {
                        node: rand_seed,
                        port: "value".into(),
                    },
                    PortRef {
                        node: circle,
                        port: "seed".into(),
                    },
                )
                .unwrap();
        },
    )
}

/// Smudge brush. Drags canvas pixels along the stroke — at each dab,
/// the `smudge_compiled` terminal samples the scratch at `position −
/// motion` and mixes by `rate × mask × selection × opacity`. Built
/// directly (not via `BrushBuilder`) because the standard builder
/// pre-wires `color_output`; smudge has its own terminal node with
/// its own lifecycle.
///
/// Compiled-graph shape: `pen → circle → smudge_compiled` plus
/// `pen.motion / .position` wired directly into the terminal. The
/// upstream `circle.texture` compiles inline into the terminal's
/// fragment shader as the per-fragment brush coverage.
fn smudge_brush() -> Brush {
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::<BrushWireType>::new();

    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let circle = graph.add_node(
        "circle",
        registry.get("circle").unwrap().ports.clone(),
        vec![ParamValue::Int(0)], // 0 = Sine Harmonic; amplitude 0 → plain disc
    );
    let smudge = graph.add_node(
        "smudge_compiled",
        registry.get("smudge_compiled").unwrap().ports.clone(),
        vec![],
    );

    // Stabilization on by default — smudge strokes read better when the
    // path is smooth. 40% is enough to take the edge off without lag.
    graph.set_port_default(pen, "stabilize", 0.4).unwrap();
    graph.set_port_exposed(pen, "stabilize", true).unwrap();

    // Tighten spacing well below the paint default. The smear is per-dab,
    // so the visible drag is dab-density-bound; 1% gives a near-continuous
    // trail. The single-pass WGSL-compiled brush pipeline keeps this within
    // frame budget — the port floor is also 1%.
    graph.set_port_default(pen, "spacing", 0.01).unwrap();

    // Sharper-than-typical tip. With a softened mask, the read at
    // `canvas_pos − motion` lands in the falloff ring and smears canvas
    // pixels into the "outside" of the brush footprint on each dab,
    // producing halo trails. Krita's stock smudge presets use sharper
    // edges for the same reason. Exposed so the user can dial it back
    // toward soft if they want the halo trail as an effect.
    graph.set_port_default(circle, "softness", 0.4).unwrap();
    graph.set_port_exposed(circle, "softness", true).unwrap();

    let wires = [
        (circle, "texture", smudge, "mask"),
        (pen, "position", smudge, "position"),
        (pen, "motion", smudge, "motion"),
        // Pressure shapes the dab — heavier press = larger smear footprint.
        (pen, "pressure", smudge, "size_input"),
    ];
    for (from_node, from_port, to_node, to_port) in wires {
        graph
            .connect(
                PortRef {
                    node: from_node,
                    port: from_port.into(),
                },
                PortRef {
                    node: to_node,
                    port: to_port.into(),
                },
            )
            .unwrap();
    }

    let mut metadata = BrushMetadata::from_graph("Smudge", graph);
    metadata.category = "painting".to_string();
    Brush::without_resources(metadata)
}

/// Liquify warp brush. Pushes pixels along pen motion with a radial
/// falloff. Unlike paint brushes, the graph has no stamp / paint_color
/// / color_output — `liquify_compiled` is itself the terminal, with
/// its own `begin_stroke` / `commit` / per-dab pass lifecycle.
fn liquify_push() -> Brush {
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::<BrushWireType>::new();

    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let liquify = graph.add_node(
        "liquify_compiled",
        registry.get("liquify_compiled").unwrap().ports.clone(),
        vec![],
    );

    let wires = [
        (pen, "position", liquify, "position"),
        // pen.drawing_angle → liquify.direction (radians; shader turns
        // it into a unit direction vector). Magnitude comes from motion.
        (pen, "drawing_angle", liquify, "direction"),
        // pen.distance → liquify.distance (gates the first dab so a
        // stationary click doesn't smear in the default direction).
        (pen, "distance", liquify, "distance"),
        // pen.motion → liquify.motion. The terminal uses |motion| as
        // the per-dab displacement scale, so 100% strength makes the
        // pixel travel a full cursor step (lock).
        (pen, "motion", liquify, "motion"),
    ];
    for (from_node, from_port, to_node, to_port) in wires {
        graph
            .connect(
                PortRef {
                    node: from_node,
                    port: from_port.into(),
                },
                PortRef {
                    node: to_node,
                    port: to_port.into(),
                },
            )
            .unwrap();
    }

    // size / strength / softness are already `.exposed()` on the
    // liquify_compiled node-def, so the toolbar picks them up without
    // extra brush work.

    // Pin dab spacing in *absolute canvas pixels*, not as a fraction
    // of brush diameter. Setting ratio to 0 disables the diameter
    // term; `spacing_min_px = LIQUIFY_SPACING_PX` then floors the
    // engine's `SpacingConfig::distance(...)` at that pixel value
    // regardless of brush size. Combined with the terminal's
    // `displacement = strength × |motion|`, this gives:
    //   * size-invariant per-dab push (the size slider controls only
    //     the warped extent),
    //   * `strength = 1` → lock (per-dab push = per-dab cursor motion),
    //   * `strength = 0.5` → 50% drag.
    graph.set_port_default(pen, "spacing", 0.0).unwrap();
    graph
        .set_port_default(
            pen,
            "spacing_min_px",
            crate::brush::nodes::liquify_compiled::LIQUIFY_SPACING_PX,
        )
        .unwrap();

    let mut metadata = BrushMetadata::from_graph("Liquify", graph);
    metadata.category = "effects".to_string();
    Brush::without_resources(metadata)
}

/// Rough Ink — the first 100%-compiled brush.
///
/// Wires `pen_input + paint_color + 3×random + circle(perlin) + stamp`
/// into the `paint_compiled` terminal. Each dab gets a unique
/// perlin-modulated silhouette driven by three independent per-dab
/// random seeds (amplitude, phase, seed). Pressure controls dab size
/// through an ink-pen front-loaded curve. The entire graph compiles
/// to one WGSL fragment shader at brush load — no per-dab GPU
/// dispatch, no inter-node textures.
///
/// This brush is the proving ground for the WGSL compilation
/// framework — see `crates/darkly/src/brush/wgsl_compile.rs`.
fn rough_ink() -> Brush {
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::<BrushWireType>::new();

    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    graph.set_port_default(pen, "stabilize", 0.6).unwrap();
    graph.set_port_exposed(pen, "stabilize", true).unwrap();

    let paint_color = graph.add_node(
        "paint_color",
        registry.get("paint_color").unwrap().ports.clone(),
        vec![],
    );
    let curve = graph.add_node(
        "curve",
        registry.get("curve").unwrap().ports.clone(),
        // One bend handle — see ink_pen for the rationale.
        vec![ParamValue::Curve(vec![[0.0, 0.0], [0.4, 0.7], [1.0, 1.0]])],
    );
    let rand_amp = graph.add_node(
        "random",
        registry.get("random").unwrap().ports.clone(),
        vec![ParamValue::Int(0)], // per-dab
    );
    let rand_phase = graph.add_node(
        "random",
        registry.get("random").unwrap().ports.clone(),
        vec![ParamValue::Int(0)],
    );
    let rand_seed = graph.add_node(
        "random",
        registry.get("random").unwrap().ports.clone(),
        vec![ParamValue::Int(0)],
    );
    let circle = graph.add_node(
        "circle",
        registry.get("circle").unwrap().ports.clone(),
        vec![ParamValue::Int(1)], // 1 = Perlin Noise
    );
    // Defaults for the perlin shape — these are stroke-constant
    // unless wired (the random nodes below override amplitude /
    // phase_input / seed per-dab).
    graph.set_port_default(circle, "frequency", 8.0).unwrap();
    graph.set_port_default(circle, "persistence", 0.5).unwrap();
    graph.set_port_default(circle, "octaves", 4.0).unwrap();
    graph.set_port_default(circle, "softness", 0.1).unwrap();

    let stamp = graph.add_node(
        "stamp",
        registry.get("stamp").unwrap().ports.clone(),
        vec![ParamValue::Int(0)], // 0 = Alpha Mask
    );
    // Stamp's `size` port is exposed by default (because per-dab
    // dispatch needs it), but `paint_compiled` ignores stamp's
    // dimension knobs — the terminal owns dab dimensions in the
    // compiled execution model. Hide stamp.size from the brush
    // properties panel so the user doesn't see two "Size" sliders
    // and scrub the inert one.
    graph.set_port_exposed(stamp, "size", false).unwrap();
    let terminal = graph.add_node(
        "paint_compiled",
        registry.get("paint_compiled").unwrap().ports.clone(),
        vec![],
    );

    let wires = [
        // Pressure → size (via ink-pen curve) on the TERMINAL,
        // because the terminal owns dab dimensions in the compiled
        // model.
        (pen, "pressure", curve, "input"),
        (curve, "output", terminal, "size_input"),
        // Pressure → flow on the stamp (modulates per-dab alpha).
        (pen, "pressure", stamp, "flow"),
        // 3 random nodes drive the perlin shape per dab.
        (rand_amp, "value", circle, "amplitude"),
        (rand_phase, "value", circle, "phase_input"),
        (rand_seed, "value", circle, "seed"),
        // Circle (shape) → stamp (tip mask).
        (circle, "texture", stamp, "tip"),
        // Paint color → stamp.
        (paint_color, "color", stamp, "color"),
        // Stamp.dab → terminal.rgba (premultiplied RGBA).
        (stamp, "dab", terminal, "rgba"),
        // Terminal needs position too.
        (pen, "position", terminal, "position"),
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

    // Amplitude default range is [0, 0.5] — the random nodes output
    // [0, 1] which gets remapped to [0, 0.5] by the wire-boundary
    // remap (circle.amplitude has natural_range = (0, 0.5)).

    let mut metadata = BrushMetadata::from_graph("Rough Ink", graph);
    metadata.category = "basic".to_string();
    Brush::without_resources(metadata)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_brushes_compile() {
        for brush in all() {
            let result = crate::brush::compile_graph(&brush.metadata.graph);
            assert!(
                result.is_ok(),
                "brush '{}' failed to compile: {:?}",
                brush.metadata.name,
                result.err(),
            );
        }
    }

    #[test]
    fn builtin_brushes_round_trip() {
        for brush in all() {
            let name = brush.metadata.name.clone();
            let bytes = brush.to_bytes().unwrap();
            let loaded = Brush::from_bytes(&bytes).unwrap();
            assert_eq!(loaded.metadata.name, name);
        }
    }

    #[test]
    fn builtin_brushes_no_overlapping_nodes() {
        for brush in all() {
            let layout = brush.metadata.graph.auto_layout();
            let positions: Vec<[i32; 2]> = layout
                .values()
                .map(|p| [p[0] as i32, p[1] as i32])
                .collect();
            for (i, a) in positions.iter().enumerate() {
                for b in &positions[i + 1..] {
                    assert_ne!(
                        a, b,
                        "brush '{}' has overlapping nodes at {:?}",
                        brush.metadata.name, a,
                    );
                }
            }
        }
    }

    #[test]
    fn builtin_brushes_unique_names() {
        let brushes = all();
        let mut names: Vec<_> = brushes.iter().map(|b| b.metadata.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), brushes.len(), "duplicate brush names");
    }

    /// Liquify needs much tighter spacing than the paint default — its
    /// per-dab displacement is ~25% of radius, so spacing must be well
    /// below that for warps to compose smoothly. Don't let this drift back
    /// to the default 10%.
    #[test]
    fn liquify_brush_has_tight_spacing() {
        let brush = liquify_push();
        let pen = brush
            .metadata
            .graph
            .nodes
            .values()
            .find(|n| n.type_id == "pen_input")
            .expect("liquify brush has a pen_input node");
        let spacing = pen
            .ports
            .iter()
            .find(|p| p.name == "spacing")
            .expect("pen_input has a spacing port");
        assert!(
            spacing.default <= 0.05,
            "liquify spacing default is {}, expected <= 5% for smooth warps",
            spacing.default
        );
    }
}
