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
    /// Create a new builder with the standard nodes and output wiring.
    ///
    /// Pre-wires: stamp.dab → color_output.dab, stamp.dab_size →
    /// color_output.dab_size, stamp.scatter_offset → color_output.scatter_offset,
    /// pen_input.position → color_output.position.
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

        // Standard output wiring (every preset needs this).
        let wires = [
            (stamp, "dab", color_output, "dab"),
            (stamp, "dab_size", color_output, "dab_size"),
            (stamp, "scatter_offset", color_output, "scatter_offset"),
            (pen, "position", color_output, "position"),
        ];
        for (from_node, from_port, to_node, to_port) in wires {
            graph.connect(
                PortRef { node: from_node, port: from_port.into() },
                PortRef { node: to_node, port: to_port.into() },
            ).unwrap();
        }

        PresetBuilder { graph, registry, pen, paint_color, stamp, color_output }
    }

    /// Add a circle node with the given softness, wired to stamp.tip.
    fn add_circle(&mut self, softness: f32) {
        let circle = self.graph.add_node(
            "circle",
            self.registry.get("circle").unwrap().ports.clone(),
            vec![],
        );
        let softness_const = self.add_constant(softness);
        self.wire(softness_const, "value", circle, "softness");
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

    /// Add a constant node with the given value.
    fn add_constant(&mut self, value: f32) -> NodeId {
        self.graph.add_node(
            "constant",
            self.registry.get("constant").unwrap().ports.clone(),
            vec![ParamValue::Float(value)],
        )
    }

    /// Add a curve node with the given gamma.
    fn add_curve(&mut self, gamma: f32) -> NodeId {
        self.graph.add_node(
            "curve",
            self.registry.get("curve").unwrap().ports.clone(),
            vec![ParamValue::Float(gamma)],
        )
    }

    /// Add a user_input node with the given label.
    fn add_user_input(&mut self, label: &str) -> NodeId {
        self.graph.add_node(
            "user_input",
            self.registry.get("user_input").unwrap().ports.clone(),
            vec![ParamValue::String(label.to_string()), ParamValue::Float(0.5)],
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

    /// Generic wire helper.
    fn wire(&mut self, from: NodeId, from_port: &str, to: NodeId, to_port: &str) {
        self.graph.connect(
            PortRef { node: from, port: from_port.into() },
            PortRef { node: to, port: to_port.into() },
        ).unwrap();
    }

    /// Build the preset (no resources).
    fn build(mut self, name: &str, category: &str) -> PresetBundle {
        self.graph.auto_layout();
        let mut preset = BrushPreset::from_graph(name, self.graph);
        preset.category = category.to_string();
        PresetBundle::without_resources(preset)
    }

    /// Build the preset with embedded PNG resources.
    fn build_with_resources(
        mut self,
        name: &str,
        category: &str,
        resources: Vec<(&str, &[u8])>,
    ) -> PresetBundle {
        self.graph.auto_layout();
        let mut preset = BrushPreset::from_graph(name, self.graph);
        preset.category = category.to_string();

        let mut resource_data = Vec::new();
        for (res_name, data) in &resources {
            preset.resources.push(PresetResourceMeta {
                name: res_name.to_string(),
                kind: ResourceKind::BrushTip,
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
    // pressure → curve(gamma=0.5) → stamp.size
    let curve = b.add_curve(0.5);
    b.wire(b.pen, "pressure", curve, "input");
    b.wire(curve, "output", b.stamp, "size");
    b.wire_pressure_to_opacity();
    b.wire_color();
    b.build("Ink Pen", "inking")
}

fn airbrush() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(1.0);
    let size = b.add_constant(0.15);
    b.wire(size, "value", b.stamp, "size");
    b.wire_pressure_to_opacity();
    b.wire_color();
    b.build("Airbrush", "basic")
}

fn scatter_brush() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(0.3);
    b.wire_pressure_to_size();
    b.wire(b.pen, "fuzzy_dab", b.stamp, "scatter_x");
    b.wire(b.pen, "fuzzy_dab", b.stamp, "scatter_y");
    b.wire_color();
    b.build("Scatter Brush", "effects")
}

fn calligraphy() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_image("calligraphy.png");
    b.wire_pressure_to_size();
    b.wire(b.pen, "tilt_direction", b.stamp, "rotation");
    b.wire_color();

    let tip_bytes: &[u8] = include_bytes!("../../resources/brush_tips/calligraphy.png");
    b.build_with_resources("Calligraphy", "inking", vec![("calligraphy.png", tip_bytes)])
}

fn textured_ink() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_image("ink_dry.png");
    b.wire_pressure_to_size();
    b.wire_pressure_to_opacity();
    b.wire(b.pen, "fuzzy_dab", b.stamp, "rotation");
    b.wire_color();

    let tip_bytes: &[u8] = include_bytes!("../../resources/brush_tips/ink_dry.png");
    b.build_with_resources("Textured Ink", "effects", vec![("ink_dry.png", tip_bytes)])
}

fn size_slider() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(0.5);
    let slider = b.add_user_input("Size");
    b.wire(slider, "value", b.stamp, "size");
    b.wire_color();
    b.build("Size Slider", "basic")
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
        for bundle in all() {
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
}
