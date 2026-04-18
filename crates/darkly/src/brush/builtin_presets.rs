//! Built-in brush presets shipped with the application.
//!
//! Each preset is a programmatically constructed node graph wrapped in a
//! `PresetBundle`.  Image-based presets embed their tip PNGs via
//! `include_bytes!`.  All presets are inserted into the `PresetLibrary`
//! at engine startup.

use crate::brush::preset::{BrushPreset, PresetBundle, PresetResourceMeta, ResourceKind};
use crate::brush::wire::BrushWireType;
use crate::brush::BrushNodeRegistry;
use crate::gpu::params::ParamValue;
use crate::nodegraph::{Graph, NodeId, PortRef};

/// Return all built-in presets.
pub fn all() -> Vec<PresetBundle> {
    vec![
        soft_round(),
        hard_round(),
        ink_pen(),
        airbrush(),
        scatter_brush(),
        calligraphy(),
        textured_ink(),
        size_slider(),
        pencil(),
        charcoal(),
        canvas_brush(),
        watercolor(),
    ]
}

// ---------------------------------------------------------------------------
// PresetBuilder — eliminates boilerplate across presets
// ---------------------------------------------------------------------------

struct PresetBuilder {
    graph: Graph<BrushWireType>,
    registry: BrushNodeRegistry,
    pen: NodeId,
    paint_color: NodeId,
    stamp: NodeId,
    #[allow(dead_code)] // Used in new() for wiring, not read afterwards.
    color_output: NodeId,
}

impl PresetBuilder {
    /// Create a new builder with the standard dab source `stamp` and
    /// output wiring.  See [`PresetBuilder::new_with_dab_source`] to
    /// substitute a different dab source (e.g. `smudge_stamp` for
    /// watercolor).
    fn new() -> Self {
        Self::new_with_dab_source("stamp")
    }

    /// Create a new builder whose dab source is the given node type.
    ///
    /// The dab source must expose the same output contract as `stamp`
    /// (`dab`, `dab_size`, `scatter_offset` ports), because those are
    /// auto-wired to `color_output`.  `self.stamp` points at the dab
    /// source — every preset helper that reads/writes `self.stamp`
    /// works unchanged with the substitute.
    ///
    /// Also wires `pen_input.position` into the dab source's `position`
    /// input when the source declares one, so nodes like `smudge_stamp`
    /// can sample the canvas at the dab location.
    fn new_with_dab_source(dab_source_type: &str) -> Self {
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
        let dab_source_reg = registry.get(dab_source_type)
            .unwrap_or_else(|| panic!("unknown dab source: {}", dab_source_type));
        let dab_source_has_position = dab_source_reg
            .ports
            .iter()
            .any(|p| p.name == "position" && p.dir == crate::nodegraph::PortDir::Input);
        let stamp = graph.add_node(
            dab_source_type,
            dab_source_reg.ports.clone(),
            vec![],
        );
        let color_output = graph.add_node(
            "color_output",
            registry.get("color_output").unwrap().ports.clone(),
            vec![],
        );

        // Standard output wiring (every preset needs this).
        let mut wires: Vec<(NodeId, &str, NodeId, &str)> = vec![
            (stamp, "dab", color_output, "dab"),
            (stamp, "dab_size", color_output, "dab_size"),
            (stamp, "scatter_offset", color_output, "scatter_offset"),
            (pen, "position", color_output, "position"),
        ];
        if dab_source_has_position {
            wires.push((pen, "position", stamp, "position"));
        }
        for (from_node, from_port, to_node, to_port) in wires {
            graph.connect(
                PortRef { node: from_node, port: from_port.into() },
                PortRef { node: to_node, port: to_port.into() },
            ).unwrap();
        }

        PresetBuilder { graph, registry, pen, paint_color, stamp, color_output }
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
        self.graph.set_port_default(circle, "softness", softness).unwrap();
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
    /// Use this for preset-specific knobs that reuse a port's built-in label.
    /// If you need a custom label for a specific preset, fall back to
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

    /// Add a user_input node with full metadata.
    ///
    /// `units`: 0 = percent, 1 = px, 2 = degrees, 3 = raw.
    fn add_user_input(
        &mut self,
        label: &str,
        value: f32,
        min: f32,
        max: f32,
        units: i32,
        icon: &str,
        description: &str,
    ) -> NodeId {
        self.graph.add_node(
            "user_input",
            self.registry.get("user_input").unwrap().ports.clone(),
            vec![
                ParamValue::String(label.to_string()),
                ParamValue::Float(value),
                ParamValue::Float(min),
                ParamValue::Float(max),
                ParamValue::Int(units),
                ParamValue::String(icon.to_string()),
                ParamValue::String(description.to_string()),
            ],
        )
    }

    /// Wire pen_input.pressure → stamp.size.
    fn wire_pressure_to_size(&mut self) {
        self.wire(self.pen, "pressure", self.stamp, "size");
    }

    /// Wire pen_input.pressure → stamp.opacity.
    fn wire_pressure_to_opacity(&mut self) {
        self.wire(self.pen, "pressure", self.stamp, "opacity");
    }

    /// Wire paint_color.color → stamp.color.
    fn wire_color(&mut self) {
        self.wire(self.paint_color, "color", self.stamp, "color");
    }

    /// Add a random node. `mode`: 0 = per-dab, 1 = per-stroke.
    fn add_random(&mut self, mode: i32) -> NodeId {
        self.graph.add_node(
            "random",
            self.registry.get("random").unwrap().ports.clone(),
            vec![ParamValue::Int(mode)],
        )
    }

    /// Generic wire helper.
    fn wire(&mut self, from: NodeId, from_port: &str, to: NodeId, to_port: &str) {
        self.graph.connect(
            PortRef { node: from, port: from_port.into() },
            PortRef { node: to, port: to_port.into() },
        ).unwrap();
    }

    /// Build the preset (no resources).
    fn build(self, name: &str, category: &str) -> PresetBundle {
        let mut preset = BrushPreset::from_graph(name, self.graph);
        preset.category = category.to_string();
        PresetBundle::without_resources(preset)
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
            &PortRef { node: self.stamp, port: "dab".into() },
            &PortRef { node: self.color_output, port: "dab".into() },
        );
        self.graph.disconnect(
            &PortRef { node: self.stamp, port: "dab_size".into() },
            &PortRef { node: self.color_output, port: "dab_size".into() },
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

    /// Build the preset with embedded PNG resources.
    fn build_with_resources(
        self,
        name: &str,
        category: &str,
        resources: Vec<(&str, ResourceKind, &[u8])>,
    ) -> PresetBundle {
        let mut preset = BrushPreset::from_graph(name, self.graph);
        preset.category = category.to_string();

        let mut resource_data = Vec::new();
        for (res_name, kind, data) in &resources {
            preset.resources.push(PresetResourceMeta {
                name: res_name.to_string(),
                kind: kind.clone(),
                path: format!("resources/{}", res_name),
            });
            resource_data.push((res_name.to_string(), data.to_vec()));
        }

        PresetBundle { preset, resource_data }
    }
}

// ---------------------------------------------------------------------------
// Preset definitions
// ---------------------------------------------------------------------------

fn soft_round() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(0.7);
    b.wire_pressure_to_size();
    b.wire_color();
    b.build("Soft Round", "basic")
}

fn hard_round() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(0.05);
    b.wire_pressure_to_size();
    b.wire_color();
    b.build("Hard Round", "basic")
}

fn ink_pen() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(0.1);
    // pressure → curve (approx sqrt) → stamp.size
    let curve = b.add_curve(vec![
        [0.0, 0.0], [0.25, 0.5], [0.5, 0.71], [0.75, 0.87], [1.0, 1.0],
    ]);
    b.wire(b.pen, "pressure", curve, "input");
    b.wire(curve, "output", b.stamp, "size");
    b.wire_pressure_to_opacity();
    b.wire_color();
    b.set_stabilize(0.6);
    b.build("Ink Pen", "inking")
}

fn airbrush() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(1.0);
    b.set_port(b.stamp, "size", 0.15);
    b.wire_pressure_to_opacity();
    b.wire_color();
    b.build("Airbrush", "basic")
}

fn scatter_brush() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(0.3);
    b.wire_pressure_to_size();
    let rand_x = b.add_random(0);
    let rand_y = b.add_random(0);
    b.wire(rand_x, "value", b.stamp, "scatter_x");
    b.wire(rand_y, "value", b.stamp, "scatter_y");
    b.wire_color();
    b.build("Scatter Brush", "effects")
}

fn calligraphy() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_image("calligraphy.png");
    b.wire_pressure_to_size();
    b.wire(b.pen, "tilt_direction", b.stamp, "rotation");
    b.wire_color();
    b.set_stabilize(0.6);

    let tip_bytes: &[u8] = include_bytes!("../../resources/brush_tips/calligraphy.png");
    b.build_with_resources("Calligraphy", "inking", vec![
        ("calligraphy.png", ResourceKind::BrushTip, tip_bytes),
    ])
}

fn textured_ink() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_image("ink_dry.png");
    b.wire_pressure_to_size();
    b.wire_pressure_to_opacity();
    let rand_rot = b.add_random(0);
    b.wire(rand_rot, "value", b.stamp, "rotation");
    b.wire_color();

    let tip_bytes: &[u8] = include_bytes!("../../resources/brush_tips/ink_dry.png");
    b.build_with_resources("Textured Ink", "effects", vec![
        ("ink_dry.png", ResourceKind::BrushTip, tip_bytes),
    ])
}

fn size_slider() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(0.5);
    let slider = b.add_user_input(
        "Size", 128.0, 1.0, 500.0, 1, "fa-solid fa-circle", "Brush diameter in pixels",
    );
    b.wire(slider, "value", b.stamp, "size");
    b.wire_color();
    b.build("Size Slider", "basic")
}

fn pencil() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(0.15);
    b.wire_pressure_to_size();
    b.wire_pressure_to_opacity();
    b.wire_color();

    // Insert texture overlay with Multiply blend (pencil grain).
    let tex = b.add_texture_overlay(0); // 0 = Multiply
    b.add_pattern("paper_grain.png", tex);

    // Subtle pencil feel.
    b.set_port(tex, "scale", 0.5);
    b.set_port(tex, "strength", 0.8);

    let pattern_bytes: &[u8] = include_bytes!("../../resources/brush_tips/paper_grain.png");
    b.build_with_resources("Pencil", "sketching", vec![
        ("paper_grain.png", ResourceKind::Pattern, pattern_bytes),
    ])
}

fn charcoal() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(0.6);
    b.wire_pressure_to_size();
    b.wire_pressure_to_opacity();
    b.wire_color();

    // Texture overlay with Subtract blend (charcoal grain — cuts into dab).
    let tex = b.add_texture_overlay(1); // 1 = Subtract
    b.add_pattern("canvas_grain.png", tex);

    b.set_port(tex, "scale", 0.7);
    b.set_port(tex, "strength", 0.9);

    let pattern_bytes: &[u8] = include_bytes!("../../resources/brush_tips/canvas_grain.png");
    b.build_with_resources("Charcoal", "sketching", vec![
        ("canvas_grain.png", ResourceKind::Pattern, pattern_bytes),
    ])
}

fn watercolor() -> PresetBundle {
    // Soft-edged wet paint: pen pressure drives size, color pulls in the
    // canvas-under-dab via the smudge bucket.  Smudge amount and length
    // are exposed as toolbar sliders — the whole point of this brush is
    // that the user can dial in pigment vs. canvas pickup.
    let mut b = PresetBuilder::new_with_dab_source("smudge_stamp");
    b.add_circle(0.85);
    b.wire_pressure_to_size();
    b.wire_pressure_to_opacity();
    b.wire_color();

    b.expose_port(b.stamp, "smudge", 0.6);
    b.expose_port(b.stamp, "smudge_length", 0.5);

    b.set_stabilize(0.4);
    b.build("Watercolor", "painting")
}

fn canvas_brush() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(0.4);
    b.wire_pressure_to_size();
    b.wire_color();

    // Texture overlay with Multiply blend and user-adjustable strength.
    let tex = b.add_texture_overlay(0); // 0 = Multiply
    b.add_pattern("canvas_grain.png", tex);

    b.set_port(tex, "scale", 1.0);
    b.expose_port(tex, "strength", 0.6);

    let pattern_bytes: &[u8] = include_bytes!("../../resources/brush_tips/canvas_grain.png");
    b.build_with_resources("Canvas Brush", "painting", vec![
        ("canvas_grain.png", ResourceKind::Pattern, pattern_bytes),
    ])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_presets_compile() {
        for bundle in all() {
            let result = crate::brush::compile_graph(&bundle.preset.graph);
            assert!(
                result.is_ok(),
                "preset '{}' failed to compile: {:?}",
                bundle.preset.name,
                result.err(),
            );
        }
    }

    #[test]
    fn builtin_presets_round_trip() {
        for bundle in all() {
            let name = bundle.preset.name.clone();
            let bytes = bundle.to_bytes().unwrap();
            let loaded = PresetBundle::from_bytes(&bytes).unwrap();
            assert_eq!(loaded.preset.name, name);
        }
    }

    #[test]
    fn builtin_presets_no_overlapping_nodes() {
        for mut bundle in all() {
            // Presets ship without positions; auto-layout before checking.
            if bundle.preset.graph.needs_layout() {
                bundle.preset.graph.auto_layout();
            }
            let positions: Vec<[i32; 2]> = bundle
                .preset
                .graph
                .nodes
                .values()
                .map(|n| [n.position[0] as i32, n.position[1] as i32])
                .collect();
            for (i, a) in positions.iter().enumerate() {
                for b in &positions[i + 1..] {
                    assert_ne!(
                        a, b,
                        "preset '{}' has overlapping nodes at {:?}",
                        bundle.preset.name, a,
                    );
                }
            }
        }
    }

    #[test]
    fn builtin_presets_unique_names() {
        let presets = all();
        let mut names: Vec<_> = presets.iter().map(|b| b.preset.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), presets.len(), "duplicate preset names");
    }

    #[test]
    fn watercolor_uses_smudge_stamp_and_wires_position() {
        // The whole point of the watercolor preset is to drive a smudge_stamp
        // dab source with pen position wired in so the shader can sample the
        // canvas under each dab.  Regression-guard both in one shot: if either
        // the dab source or the position wire is lost, smudge silently breaks.
        let bundle = watercolor();
        let smudge_stamp_id = bundle
            .preset
            .graph
            .nodes
            .iter()
            .find(|(_, node)| node.type_id == "smudge_stamp")
            .map(|(id, _)| *id)
            .expect("watercolor preset must contain a smudge_stamp node");

        let pen_id = bundle
            .preset
            .graph
            .nodes
            .iter()
            .find(|(_, node)| node.type_id == "pen_input")
            .map(|(id, _)| *id)
            .expect("watercolor preset must contain a pen_input node");

        let has_position_wire = bundle
            .preset
            .graph
            .connections
            .iter()
            .any(|c|
                c.from.node == pen_id && c.from.port == "position"
                    && c.to.node == smudge_stamp_id && c.to.port == "position"
            );
        assert!(has_position_wire,
            "pen_input.position must be wired into smudge_stamp.position");
    }
}