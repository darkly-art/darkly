//! Built-in brushes shipped with the application.
//!
//! Each brush is a programmatically constructed node graph wrapped in a
//! `Brush`.  Image-based brushes embed their tip PNGs via
//! `include_bytes!`.  All brushes are inserted into the `BrushLibrary`
//! at engine startup.

use crate::brush::bundle::{Brush, BrushMetadata};
use crate::brush::wire::BrushWireType;
use crate::brush::BrushNodeRegistry;
use crate::gpu::params::ParamValue;
use crate::nodegraph::{Graph, NodeId, PortRef};

/// Return all built-in brushes.
pub fn all() -> Vec<Brush> {
    vec![
        ink_pen(),
        airbrush(),
        smooth_watercolor(),
        rough_watercolor(),
        smudge_brush(),
        liquify_push(),
    ]
}

// ---------------------------------------------------------------------------
// BrushBuilder — eliminates boilerplate across brushes
// ---------------------------------------------------------------------------

struct BrushBuilder {
    graph: Graph<BrushWireType>,
    registry: BrushNodeRegistry,
    pen: NodeId,
    paint_color: NodeId,
    stamp: NodeId,
    #[allow(dead_code)] // Used in new() for wiring, not read afterwards.
    color_output: NodeId,
}

impl BrushBuilder {
    /// Create a new builder with the standard nodes and output wiring.
    ///
    /// Pre-wires: stamp.dab → color_output.dab, stamp.dab_size →
    /// color_output.dab_size, pen_input.position → color_output.position,
    /// and stamp.preview → color_output.brush_preview (the hover preview
    /// path — terminal's `render_preview` hook blits this into the
    /// overlay). Brushes that want jitter call `wire_scatter` to splice
    /// a `scatter` node onto the position wire.
    fn new() -> Self {
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
        let stamp = graph.add_node(
            "stamp",
            registry.get("stamp").unwrap().ports.clone(),
            vec![],
        );
        let color_output = graph.add_node(
            "color_output",
            registry.get("color_output").unwrap().ports.clone(),
            vec![],
        );

        // Standard output wiring (every brush needs this).
        let wires = [
            (stamp, "dab", color_output, "dab"),
            (stamp, "dab_size", color_output, "dab_size"),
            (pen, "position", color_output, "position"),
            // Hover preview: the terminal's `render_preview` hook blits the
            // stamp's transform-baked, deposition-stripped preview texture.
            (stamp, "preview", color_output, "brush_preview"),
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

        BrushBuilder {
            graph,
            registry,
            pen,
            paint_color,
            stamp,
            color_output,
        }
    }

    /// Add a circle node with the given softness, wired to stamp.tip.
    fn add_circle(&mut self, softness: f32) {
        let circle = self.graph.add_node(
            "circle",
            self.registry.get("circle").unwrap().ports.clone(),
            vec![],
        );
        self.graph
            .set_port_default(circle, "softness", softness)
            .unwrap();
        self.wire(circle, "texture", self.stamp, "tip");
    }

    /// Set a port's instance-level default value.
    ///
    /// Replaces the old `add_constant` + `wire` pattern for feeding a fixed
    /// number into a port: the port simply carries the value, no extra node
    /// or wire needed.  The port's node-def metadata (label, unit, icon,
    /// range) is reused; only the default changes.
    fn set_port(&mut self, node: NodeId, port: &str, value: f32) {
        self.graph.set_port_default(node, port, value).unwrap();
    }

    /// Generic wire helper.
    fn wire(&mut self, from: NodeId, from_port: &str, to: NodeId, to_port: &str) {
        self.graph
            .connect(
                PortRef {
                    node: from,
                    port: from_port.into(),
                },
                PortRef {
                    node: to,
                    port: to_port.into(),
                },
            )
            .unwrap();
    }

    /// Build the brush (no resources).
    fn build(self, name: &str, category: &str) -> Brush {
        let mut metadata = BrushMetadata::from_graph(name, self.graph);
        metadata.category = category.to_string();
        Brush::without_resources(metadata)
    }
}

// ---------------------------------------------------------------------------
// Brush definitions
// ---------------------------------------------------------------------------

/// Ink Pen — POC compute-shader terminal.
///
/// Bypasses `BrushBuilder::new()`'s hardcoded `stamp + color_output` pair
/// and wires straight into `ink_pen_compute`, a single GPU node that
/// folds circle + stamp + compositing into one compute dispatch per
/// rendering phase. Everything else (`pen_input`, `paint_color`, the
/// pressure curve, the stabilizer) is identical to the prior ink-pen
/// definition — only the GPU terminal has changed.
///
/// See `crates/darkly/src/brush/nodes/ink_pen_compute.rs` for the
/// terminal and `darkly-stabilization-perf-investigation.md` for the
/// motivation behind the swap.
fn ink_pen() -> Brush {
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
    let curve = graph.add_node(
        "curve",
        registry.get("curve").unwrap().ports.clone(),
        vec![ParamValue::Curve(vec![
            [0.0, 0.0],
            [0.25, 0.5],
            [0.5, 0.71],
            [0.75, 0.87],
            [1.0, 1.0],
        ])],
    );
    let terminal = graph.add_node(
        "ink_pen_compute",
        registry.get("ink_pen_compute").unwrap().ports.clone(),
        vec![],
    );

    let wires = [
        (pen, "pressure", curve, "input"),
        (curve, "output", terminal, "size_input"),
        (pen, "pressure", terminal, "flow"),
        (pen, "position", terminal, "position"),
        (paint_color, "color", terminal, "color"),
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

    // Stabilization exposed to the toolbar (matches prior ink-pen behavior).
    graph.set_port_default(pen, "stabilize", 0.6).unwrap();
    graph.set_port_exposed(pen, "stabilize", true).unwrap();

    let mut metadata = BrushMetadata::from_graph("Ink Pen", graph);
    metadata.category = "inking".to_string();
    Brush::without_resources(metadata)
}

fn airbrush() -> Brush {
    let mut b = BrushBuilder::new();
    b.add_circle(1.0);
    b.set_port(b.stamp, "size_input", 0.15);
    b.wire(b.pen, "pressure", b.stamp, "flow");
    b.wire(b.paint_color, "color", b.stamp, "color");
    b.build("Airbrush", "basic")
}

/// Build a watercolor brush around a procedural `circle` tip.
///
/// All watercolor variants share the same wiring — pen + paint_color into a
/// stamp into the `watercolor` terminal, with a per-dab random rotation so
/// the dab's outline lands at a fresh angle every stamp. The variants only
/// differ in how the circle is configured, so the caller passes a closure
/// that sets the algorithm enum and any port defaults on the circle node.
///
/// Built directly rather than via `BrushBuilder` because the standard
/// builder pre-wires `color_output` as the terminal — watercolor swaps
/// that for its own `watercolor` terminal node.
fn watercolor_brush(
    name: &str,
    configure: impl FnOnce(&mut Graph<BrushWireType>, NodeId, NodeId),
) -> Brush {
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::<BrushWireType>::new();

    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    // Stabilization: stroke smoothing helps watercolor read as a single
    // continuous wash rather than a jittery line. 50% is enough to take the
    // edge off without the brush feeling laggy.
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
        vec![ParamValue::Int(0)], // overwritten by the closure if needed
    );
    let stamp = graph.add_node(
        "stamp",
        registry.get("stamp").unwrap().ports.clone(),
        vec![],
    );
    let watercolor = graph.add_node(
        "watercolor",
        registry.get("watercolor").unwrap().ports.clone(),
        vec![],
    );

    let wires = [
        // Stamp builds the dab shape and bakes paint color into RGB. The
        // watercolor terminal reads `dab.a` for the alpha mask and uses the
        // separately-wired `color` for the paint color in the mix.
        (circle, "texture", stamp, "tip"),
        (paint_color, "color", stamp, "color"),
        (paint_color, "color", watercolor, "color"),
        // Pressure → flow so light strokes deposit less paint, the way a
        // real brush carries less pigment with less pressure.
        (pen, "pressure", stamp, "flow"),
        (stamp, "dab", watercolor, "dab"),
        (stamp, "dab_size", watercolor, "dab_size"),
        (pen, "position", watercolor, "position"),
        (stamp, "preview", watercolor, "brush_preview"),
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

    // Variant-specific configuration runs last so the closure can wire
    // additional nodes to any of the shared nodes (circle for shape
    // params, stamp for rotation/jitter, etc.).
    configure(&mut graph, circle, stamp);

    let mut metadata = BrushMetadata::from_graph(name, graph);
    metadata.category = "painting".to_string();
    Brush::without_resources(metadata)
}

/// Smooth watercolor: sine-harmonic dab with gentle bumps for an organic
/// hand-painted edge.
fn smooth_watercolor() -> Brush {
    watercolor_brush("Smooth Watercolor", |graph, circle, stamp| {
        graph
            .set_param(circle, 0, ParamValue::Int(0)) // 0 = Sine Harmonic
            .unwrap();
        graph.set_port_default(circle, "amplitude", 0.05).unwrap();
        graph.set_port_default(circle, "frequency", 5.0).unwrap();
        graph.set_port_default(circle, "phase", 0.0).unwrap();
        // Per-dab random rotation so the harmonic bumps land at a fresh
        // angle every stamp — without it, every dab is identical and the
        // bumps line up along the stroke. (Rough watercolor doesn't need
        // this because its per-dab seed gives a fresh noise pattern, not
        // just a fresh rotation of the same pattern.)
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
                    node: stamp,
                    port: "rotation_input".into(),
                },
            )
            .unwrap();
    })
}

/// Rough watercolor: Perlin-noise dab with a more chaotic, granulated edge.
fn rough_watercolor() -> Brush {
    watercolor_brush("Rough Watercolor", |graph, circle, _stamp| {
        graph
            .set_param(circle, 0, ParamValue::Int(1)) // 1 = Perlin Noise
            .unwrap();
        graph.set_port_default(circle, "softness", 0.05).unwrap();
        graph.set_port_default(circle, "amplitude", 0.4).unwrap();
        graph.set_port_default(circle, "frequency", 12.0).unwrap();
        graph.set_port_default(circle, "persistence", 0.55).unwrap();
        graph.set_port_default(circle, "octaves", 4.0).unwrap();
        // Per-dab random seed so every dab gets a fresh Perlin pattern —
        // without it, every dab has the same bumpy outline and the
        // repetition reads as a regular texture rather than the chaotic
        // granulation this brush is meant for. The full per-dab noise
        // reshuffle subsumes what a rotation-randomizer would add, so
        // unlike smooth watercolor this variant doesn't wire one.
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
    })
}

/// Smudge brush. Drags canvas pixels along the stroke — at each dab, the
/// `smudge` terminal samples the scratch at `position − motion` and stamps
/// it back through the brush mask. Built directly (not via `BrushBuilder`)
/// because the standard builder pre-wires `color_output`; smudge has its
/// own terminal node with its own lifecycle.
fn smudge_brush() -> Brush {
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::<BrushWireType>::new();

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
        vec![],
    );
    let smudge = graph.add_node(
        "smudge",
        registry.get("smudge").unwrap().ports.clone(),
        vec![],
    );

    // Stabilization on by default — smudge strokes read better when the
    // path is smooth. 40% is enough to take the edge off without lag.
    graph.set_port_default(pen, "stabilize", 0.4).unwrap();
    graph.set_port_exposed(pen, "stabilize", true).unwrap();

    // Tighten spacing well below the paint default. The smear is per-dab,
    // so the visible drag is dab-density-bound; the liquify-style 4% gives
    // a continuous trail. The port floor is also 4%.
    graph.set_port_default(pen, "spacing", 0.04).unwrap();

    // Sharper-than-typical tip. With a softened mask, the read at
    // `canvas_pos − motion` lands in the falloff ring and smears canvas
    // pixels into the "outside" of the brush footprint on each dab,
    // producing halo trails. Krita's stock smudge presets use sharper
    // edges for the same reason. Exposed so the user can dial it back
    // toward soft if they want the halo trail as an effect.
    graph.set_port_default(circle, "softness", 0.4).unwrap();
    graph.set_port_exposed(circle, "softness", true).unwrap();

    let wires = [
        (circle, "texture", stamp, "tip"),
        (paint_color, "color", stamp, "color"),
        // Pressure shapes the dab — heavier press = larger, fuller smear.
        (pen, "pressure", stamp, "flow"),
        (pen, "pressure", stamp, "size_input"),
        (stamp, "dab", smudge, "dab"),
        (stamp, "dab_size", smudge, "dab_size"),
        (pen, "position", smudge, "position"),
        (pen, "motion", smudge, "motion"),
        (stamp, "preview", smudge, "brush_preview"),
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
/// falloff. Unlike paint brushes, the graph has no stamp / paint_color /
/// color_output — the liquify node is itself the terminal, with its own
/// `begin_stroke` / `commit` / `render_preview` lifecycle.
fn liquify_push() -> Brush {
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::<BrushWireType>::new();

    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let liquify = graph.add_node(
        "liquify",
        registry.get("liquify").unwrap().ports.clone(),
        vec![],
    );

    // pen_input.position → liquify.position
    graph
        .connect(
            PortRef {
                node: pen,
                port: "position".into(),
            },
            PortRef {
                node: liquify,
                port: "position".into(),
            },
        )
        .unwrap();
    // pen_input.drawing_angle → liquify.direction (radians; shader turns
    // it into a unit direction vector). Magnitude comes from strength.
    graph
        .connect(
            PortRef {
                node: pen,
                port: "drawing_angle".into(),
            },
            PortRef {
                node: liquify,
                port: "direction".into(),
            },
        )
        .unwrap();
    // pen_input.distance → liquify.distance (gates the first dab so a
    // stationary click doesn't smear in the default direction).
    graph
        .connect(
            PortRef {
                node: pen,
                port: "distance".into(),
            },
            PortRef {
                node: liquify,
                port: "distance".into(),
            },
        )
        .unwrap();

    // size / strength / softness are already `.exposed()` on the liquify
    // node-def, so the toolbar picks them up without extra brush work.

    // Tighten dab spacing well below the paint default (10%). Liquify's
    // per-dab displacement is ~25% of radius (DRAG_FACTOR in liquify.rs),
    // so spacing must be much smaller for warps to accumulate smoothly.
    // 4% is the port floor — anything lower kills stabilizer performance.
    graph.set_port_default(pen, "spacing", 0.04).unwrap();

    // Compensate the per-dab strength for the ~2.5× denser dabs — total
    // accumulated displacement along the stroke stays roughly what it was
    // at the old 10% spacing / 0.5 strength combination. Tune empirically.
    graph.set_port_default(liquify, "strength", 0.2).unwrap();

    let mut metadata = BrushMetadata::from_graph("Liquify", graph);
    metadata.category = "effects".to_string();
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
