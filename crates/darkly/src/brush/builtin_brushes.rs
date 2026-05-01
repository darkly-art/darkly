//! Built-in brushes shipped with the application.
//!
//! Each brush is a programmatically constructed node graph wrapped in a
//! `Brush`.  Image-based brushes embed their tip PNGs via
//! `include_bytes!`.  All brushes are inserted into the `BrushLibrary`
//! at engine startup.

use crate::brush::bundle::{Brush, BrushMetadata, BrushResourceMeta, ResourceKind};
use crate::brush::wire::BrushWireType;
use crate::brush::BrushNodeRegistry;
use crate::gpu::params::ParamValue;
use crate::nodegraph::{Graph, NodeId, PortRef};

/// Return all built-in brushes.
pub fn all() -> Vec<Brush> {
    vec![
        soft_round(),
        hard_round(),
        ink_pen(),
        airbrush(),
        scatter_brush(),
        calligraphy(),
        textured_ink(),
        pencil(),
        charcoal(),
        canvas_brush(),
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

    /// Set the stabilization strength and expose it in the toolbar.
    fn set_stabilize(&mut self, strength: f32) {
        self.expose_port(self.pen, "stabilize", strength);
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

    /// Add an image node with a resource name, wired to stamp.tip.
    fn add_image(&mut self, resource_name: &str) {
        let image = self.graph.add_node(
            "image",
            self.registry.get("image").unwrap().ports.clone(),
            vec![ParamValue::String(resource_name.to_string())],
        );
        self.wire(image, "texture", self.stamp, "tip");
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

    /// Set a port's default value and expose it as a user-adjustable control.
    ///
    /// The port's existing metadata (label, unit, icon, range) drives the UI.
    /// Use this for brush-specific knobs that reuse a port's built-in label.
    /// If you need a custom label for a specific brush, fall back to
    /// `add_user_input` + `wire`.
    fn expose_port(&mut self, node: NodeId, port: &str, value: f32) {
        self.graph.set_port_default(node, port, value).unwrap();
        self.graph.set_port_exposed(node, port, true).unwrap();
    }

    /// Add a curve node with control points defining the transfer function.
    fn add_curve(&mut self, points: Vec<[f32; 2]>) -> NodeId {
        self.graph.add_node(
            "curve",
            self.registry.get("curve").unwrap().ports.clone(),
            vec![ParamValue::Curve(points)],
        )
    }

    /// Add a multiply node (Scalar × Scalar → Scalar).
    fn add_multiply(&mut self) -> NodeId {
        self.graph.add_node(
            "multiply",
            self.registry.get("multiply").unwrap().ports.clone(),
            vec![],
        )
    }

    /// Add a random node. `mode`: 0 = per-dab, 1 = per-stroke.
    fn add_random(&mut self, mode: i32) -> NodeId {
        self.graph.add_node(
            "random",
            self.registry.get("random").unwrap().ports.clone(),
            vec![ParamValue::Int(mode)],
        )
    }

    /// Splice a `scatter` node onto the position wire feeding
    /// `color_output.position`, with size-proportional displacement —
    /// `stamp.dab_size` → `split_vec2.x` → `scatter.dab_size`. The
    /// amounts are exposed as toolbar knobs. Returns the scatter node id.
    fn wire_scatter(&mut self, amount_x: f32, amount_y: f32) -> NodeId {
        let scatter = self.graph.add_node(
            "scatter",
            self.registry.get("scatter").unwrap().ports.clone(),
            vec![],
        );
        let split = self.graph.add_node(
            "split_vec2",
            self.registry.get("split_vec2").unwrap().ports.clone(),
            vec![],
        );
        self.graph.disconnect(
            &PortRef {
                node: self.pen,
                port: "position".into(),
            },
            &PortRef {
                node: self.color_output,
                port: "position".into(),
            },
        );
        self.wire(self.pen, "position", scatter, "position");
        self.wire(scatter, "position", self.color_output, "position");
        self.wire(self.stamp, "dab_size", split, "vec");
        self.wire(split, "x", scatter, "dab_size");
        self.expose_port(scatter, "amount_x", amount_x);
        self.expose_port(scatter, "amount_y", amount_y);
        scatter
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

    /// Insert a texture_overlay node between stamp and color_output.
    ///
    /// Rewires: stamp.dab → texture_overlay.dab → color_output.dab,
    /// stamp.dab_size → texture_overlay.dab_size → color_output.dab_size.
    /// Wires pen_input.position → texture_overlay.position for tiling.
    /// Returns the texture_overlay node ID for further wiring (e.g. pattern input).
    fn add_texture_overlay(&mut self, blend_mode: i32) -> NodeId {
        let tex = self.graph.add_node(
            "texture_overlay",
            self.registry.get("texture_overlay").unwrap().ports.clone(),
            vec![ParamValue::Int(blend_mode)],
        );

        // Disconnect stamp → color_output for dab and dab_size.
        self.graph.disconnect(
            &PortRef {
                node: self.stamp,
                port: "dab".into(),
            },
            &PortRef {
                node: self.color_output,
                port: "dab".into(),
            },
        );
        self.graph.disconnect(
            &PortRef {
                node: self.stamp,
                port: "dab_size".into(),
            },
            &PortRef {
                node: self.color_output,
                port: "dab_size".into(),
            },
        );

        // Wire stamp → texture_overlay → color_output.
        self.wire(self.stamp, "dab", tex, "dab");
        self.wire(self.stamp, "dab_size", tex, "dab_size");
        self.wire(tex, "dab", self.color_output, "dab");
        self.wire(tex, "dab_size", self.color_output, "dab_size");

        // Wire position for pattern tiling.
        self.wire(self.pen, "position", tex, "position");

        tex
    }

    /// Add a pattern image node and wire it to a texture_overlay's pattern input.
    fn add_pattern(&mut self, resource_name: &str, tex_overlay: NodeId) {
        let image = self.graph.add_node(
            "image",
            self.registry.get("image").unwrap().ports.clone(),
            vec![ParamValue::String(resource_name.to_string())],
        );
        self.wire(image, "texture", tex_overlay, "pattern");
    }

    /// Build the brush with embedded PNG resources.
    fn build_with_resources(
        self,
        name: &str,
        category: &str,
        resources: Vec<(&str, ResourceKind, &[u8])>,
    ) -> Brush {
        let mut metadata = BrushMetadata::from_graph(name, self.graph);
        metadata.category = category.to_string();

        let mut resource_data = Vec::new();
        for (res_name, kind, data) in &resources {
            metadata.resources.push(BrushResourceMeta {
                name: res_name.to_string(),
                kind: kind.clone(),
                path: format!("resources/{}", res_name),
            });
            resource_data.push((res_name.to_string(), data.to_vec()));
        }

        Brush {
            metadata,
            resource_data,
            thumbnail_png: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Brush definitions
// ---------------------------------------------------------------------------

fn soft_round() -> Brush {
    let mut b = BrushBuilder::new();
    b.add_circle(0.7);
    b.wire(b.paint_color, "color", b.stamp, "color");
    b.build("Soft Round", "basic")
}

fn hard_round() -> Brush {
    let mut b = BrushBuilder::new();
    b.add_circle(0.05);
    b.wire(b.paint_color, "color", b.stamp, "color");
    b.build("Hard Round", "basic")
}

fn ink_pen() -> Brush {
    let mut b = BrushBuilder::new();
    b.add_circle(0.1);
    // pressure → curve (approx sqrt) → stamp.size
    let curve = b.add_curve(vec![
        [0.0, 0.0],
        [0.25, 0.5],
        [0.5, 0.71],
        [0.75, 0.87],
        [1.0, 1.0],
    ]);
    b.wire(b.pen, "pressure", curve, "input");
    b.wire(curve, "output", b.stamp, "size_input");
    b.wire(b.pen, "pressure", b.stamp, "flow");
    b.wire(b.paint_color, "color", b.stamp, "color");
    b.set_stabilize(0.6);
    b.build("Ink Pen", "inking")
}

fn airbrush() -> Brush {
    let mut b = BrushBuilder::new();
    b.add_circle(1.0);
    b.set_port(b.stamp, "size_input", 0.15);
    b.wire(b.pen, "pressure", b.stamp, "flow");
    b.wire(b.paint_color, "color", b.stamp, "color");
    b.build("Airbrush", "basic")
}

fn scatter_brush() -> Brush {
    let mut b = BrushBuilder::new();
    b.add_circle(0.3);
    b.wire(b.pen, "pressure", b.stamp, "size_input");
    b.wire(b.paint_color, "color", b.stamp, "color");
    b.wire_scatter(1.0, 1.0);
    b.build("Scatter Brush", "effects")
}

fn calligraphy() -> Brush {
    let mut b = BrushBuilder::new();
    b.add_image("calligraphy.png");
    b.wire(b.pen, "pressure", b.stamp, "size_input");
    b.wire(b.pen, "tilt_direction", b.stamp, "rotation");
    b.wire(b.paint_color, "color", b.stamp, "color");
    b.set_stabilize(0.6);
    // Tighter spacing than the 10% default — calligraphic strokes need
    // smooth edges as the angled tip rotates with tilt direction.
    b.set_port(b.pen, "spacing", 0.05);

    let tip_bytes: &[u8] = include_bytes!("../../resources/brush_tips/calligraphy.png");
    b.build_with_resources(
        "Calligraphy",
        "inking",
        vec![("calligraphy.png", ResourceKind::BrushTip, tip_bytes)],
    )
}

fn textured_ink() -> Brush {
    let mut b = BrushBuilder::new();
    b.add_image("ink_dry.png");
    b.wire(b.pen, "pressure", b.stamp, "size_input");
    b.wire(b.pen, "pressure", b.stamp, "flow");
    // random.value is in -1..1; stamp.rotation wants radians. Scale by π
    // so rotation jitters across a full turn (-π..π).
    let rand_rot = b.add_random(0);
    let scale_rot = b.add_multiply();
    b.set_port(scale_rot, "b", std::f32::consts::PI);
    b.wire(rand_rot, "value", scale_rot, "a");
    b.wire(scale_rot, "result", b.stamp, "rotation");
    b.wire(b.paint_color, "color", b.stamp, "color");

    let tip_bytes: &[u8] = include_bytes!("../../resources/brush_tips/ink_dry.png");
    b.build_with_resources(
        "Textured Ink",
        "effects",
        vec![("ink_dry.png", ResourceKind::BrushTip, tip_bytes)],
    )
}

fn pencil() -> Brush {
    let mut b = BrushBuilder::new();
    b.add_circle(0.15);
    b.wire(b.pen, "pressure", b.stamp, "size_input");
    b.wire(b.pen, "pressure", b.stamp, "flow");
    b.wire(b.paint_color, "color", b.stamp, "color");

    // Insert texture overlay with Multiply blend (pencil grain).
    let tex = b.add_texture_overlay(0); // 0 = Multiply
    b.add_pattern("paper_grain.png", tex);

    // Subtle pencil feel.
    b.set_port(tex, "scale", 0.5);
    b.set_port(tex, "strength", 0.8);

    let pattern_bytes: &[u8] = include_bytes!("../../resources/brush_tips/paper_grain.png");
    b.build_with_resources(
        "Pencil",
        "sketching",
        vec![("paper_grain.png", ResourceKind::Pattern, pattern_bytes)],
    )
}

fn charcoal() -> Brush {
    let mut b = BrushBuilder::new();
    b.add_circle(0.6);
    b.wire(b.pen, "pressure", b.stamp, "size_input");
    b.wire(b.pen, "pressure", b.stamp, "flow");
    b.wire(b.paint_color, "color", b.stamp, "color");

    // Texture overlay with Subtract blend (charcoal grain — cuts into dab).
    let tex = b.add_texture_overlay(1); // 1 = Subtract
    b.add_pattern("canvas_grain.png", tex);

    b.set_port(tex, "scale", 0.7);
    b.set_port(tex, "strength", 0.9);

    let pattern_bytes: &[u8] = include_bytes!("../../resources/brush_tips/canvas_grain.png");
    b.build_with_resources(
        "Charcoal",
        "sketching",
        vec![("canvas_grain.png", ResourceKind::Pattern, pattern_bytes)],
    )
}

fn canvas_brush() -> Brush {
    let mut b = BrushBuilder::new();
    b.add_circle(0.4);
    b.wire(b.pen, "pressure", b.stamp, "size_input");
    b.wire(b.paint_color, "color", b.stamp, "color");

    // Texture overlay with Multiply blend and user-adjustable strength.
    let tex = b.add_texture_overlay(0); // 0 = Multiply
    b.add_pattern("canvas_grain.png", tex);

    b.set_port(tex, "scale", 1.0);
    b.expose_port(tex, "strength", 0.6);

    let pattern_bytes: &[u8] = include_bytes!("../../resources/brush_tips/canvas_grain.png");
    b.build_with_resources(
        "Canvas Brush",
        "painting",
        vec![("canvas_grain.png", ResourceKind::Pattern, pattern_bytes)],
    )
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
        for mut brush in all() {
            // Brushes ship without positions; auto-layout before checking.
            if brush.metadata.graph.needs_layout() {
                brush.metadata.graph.auto_layout();
            }
            let positions: Vec<[i32; 2]> = brush
                .metadata
                .graph
                .nodes
                .values()
                .map(|n| [n.position[0] as i32, n.position[1] as i32])
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
