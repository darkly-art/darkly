//! `.darkly-brush` preset format — ZIP archive containing a JSON envelope
//! and optional binary resources (brush tips, textures).
//!
//! Format:
//!   preset.json        — metadata + serialized node graph
//!   resources/<name>   — binary assets referenced by the graph

use std::io::{Cursor, Read, Write};

use serde::{Deserialize, Serialize};

use crate::brush::stabilizer::StabilizerConfig;
use crate::brush::wire::BrushWireType;
use crate::nodegraph::Graph;

/// Current format version.  Bump when the schema changes in a
/// backwards-incompatible way.
pub const FORMAT_VERSION: u32 = 1;

/// A brush preset — the unit of save/load/share.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrushPreset {
    pub format_version: u32,
    pub name: String,
    #[serde(default = "default_engine_version")]
    pub engine_version: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub graph: Graph<BrushWireType>,
    /// Stabilizer configuration.  Default = no stabilization (pass-through).
    #[serde(default)]
    pub stabilizer: StabilizerConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<PresetResourceMeta>,
}

/// Metadata for a resource embedded in the ZIP.
/// The actual bytes live in `PresetBundle::resources`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PresetResourceMeta {
    pub name: String,
    pub kind: ResourceKind,
    /// Path inside the ZIP (e.g. "resources/tip.png").
    pub path: String,
}

/// Kind of embedded resource.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    BrushTip,
    Pattern,
}

/// A fully-loaded preset with its resource data in memory.
#[derive(Clone, Debug)]
pub struct PresetBundle {
    pub preset: BrushPreset,
    /// Resource data keyed by the `name` field in `PresetResourceMeta`.
    pub resource_data: Vec<(String, Vec<u8>)>,
}

fn default_engine_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

impl BrushPreset {
    /// Create a preset from just a graph (no resources).
    pub fn from_graph(name: impl Into<String>, graph: Graph<BrushWireType>) -> Self {
        BrushPreset {
            format_version: FORMAT_VERSION,
            name: name.into(),
            engine_version: default_engine_version(),
            category: String::new(),
            author: String::new(),
            description: String::new(),
            tags: Vec::new(),
            graph,
            stabilizer: StabilizerConfig::default(),
            resources: Vec::new(),
        }
    }
}

impl PresetBundle {
    /// Create a bundle from a preset with no resources.
    pub fn without_resources(preset: BrushPreset) -> Self {
        PresetBundle {
            preset,
            resource_data: Vec::new(),
        }
    }

    /// Serialize to `.darkly-brush` ZIP bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);

        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // Write preset.json
        let json = serde_json::to_string_pretty(&self.preset)
            .map_err(|e| format!("failed to serialize preset: {e}"))?;
        zip.start_file("preset.json", options)
            .map_err(|e| format!("zip write error: {e}"))?;
        zip.write_all(json.as_bytes())
            .map_err(|e| format!("zip write error: {e}"))?;

        // Write resources
        for (name, data) in &self.resource_data {
            // Find the matching metadata to get the ZIP path.
            let path = self
                .preset
                .resources
                .iter()
                .find(|r| r.name == *name)
                .map(|r| r.path.clone())
                .unwrap_or_else(|| format!("resources/{name}"));

            zip.start_file(&path, options)
                .map_err(|e| format!("zip write error: {e}"))?;
            zip.write_all(data)
                .map_err(|e| format!("zip write error: {e}"))?;
        }

        let cursor = zip.finish().map_err(|e| format!("zip finalize error: {e}"))?;
        Ok(cursor.into_inner())
    }

    /// Deserialize from `.darkly-brush` ZIP bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let cursor = Cursor::new(bytes);
        let mut archive =
            zip::ZipArchive::new(cursor).map_err(|e| format!("invalid ZIP archive: {e}"))?;

        // Read preset.json
        let mut preset: BrushPreset = {
            let mut file = archive
                .by_name("preset.json")
                .map_err(|e| format!("missing preset.json: {e}"))?;
            let mut json = String::new();
            file.read_to_string(&mut json)
                .map_err(|e| format!("failed to read preset.json: {e}"))?;
            serde_json::from_str(&json)
                .map_err(|e| format!("invalid preset.json: {e}"))?
        };

        // Migrate: the stamp node's per-dab alpha port was renamed from
        // "opacity" to "flow" during the paint refactor. Any preset saved
        // before that carries the old name; rewrite in place so compilation
        // finds the right port. Silent one-way upgrade — old presets keep
        // working without a format bump.
        migrate_stamp_opacity_to_flow(&mut preset.graph);

        // Migrate: the `preview_output` node was removed when terminals
        // gained a `render_preview` lifecycle hook. Drop any legacy
        // `preview_output` nodes and install the new
        // `stamp.preview → color_output.brush_preview` wire so loaded
        // presets continue to show a hover preview.
        migrate_drop_preview_output(&mut preset.graph);

        if preset.format_version > FORMAT_VERSION {
            return Err(format!(
                "preset format version {} is newer than supported version {FORMAT_VERSION}",
                preset.format_version
            ));
        }

        // Read resource data
        let mut resource_data = Vec::new();
        for meta in &preset.resources {
            match archive.by_name(&meta.path) {
                Ok(mut file) => {
                    let mut data = Vec::with_capacity(file.size() as usize);
                    file.read_to_end(&mut data)
                        .map_err(|e| format!("failed to read resource '{}': {e}", meta.name))?;
                    resource_data.push((meta.name.clone(), data));
                }
                Err(e) => {
                    return Err(format!(
                        "resource '{}' referenced at '{}' not found in ZIP: {e}",
                        meta.name, meta.path
                    ));
                }
            }
        }

        Ok(PresetBundle {
            preset,
            resource_data,
        })
    }

    /// Look up resource data by name.
    pub fn resource(&self, name: &str) -> Option<&[u8]> {
        self.resource_data
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| d.as_slice())
    }

    /// Save to a file path.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let bytes = self.to_bytes()?;
        std::fs::write(path, bytes).map_err(|e| format!("failed to write preset: {e}"))
    }

    /// Load from a file path.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("failed to read preset file: {e}"))?;
        Self::from_bytes(&bytes)
    }
}

/// Rewrite every reference to a `stamp` node's "opacity" port so that it
/// uses the new "flow" name. Applies to:
/// - the node's own `ports` vector (so `set_port_default` finds it),
/// - any `Connection` that routes to/from the old name.
///
/// Pre-refactor presets stored the per-dab alpha port as "opacity". The
/// refactor separated that from stroke-level opacity by renaming to "flow";
/// this migration keeps legacy presets loading silently.
fn migrate_stamp_opacity_to_flow(graph: &mut Graph<BrushWireType>) {
    use crate::nodegraph::NodeId;

    // Collect stamp node ids up-front — we mutate both `nodes` and
    // `connections`, and don't want to hold a borrow across that.
    let stamp_ids: Vec<NodeId> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.type_id == "stamp")
        .map(|(id, _)| *id)
        .collect();
    if stamp_ids.is_empty() {
        return;
    }

    for id in &stamp_ids {
        if let Some(node) = graph.nodes.get_mut(id) {
            for port in node.ports.iter_mut() {
                if port.name == "opacity" {
                    port.name = "flow".into();
                }
            }
        }
    }

    for conn in graph.connections.iter_mut() {
        if stamp_ids.contains(&conn.to.node) && conn.to.port == "opacity" {
            conn.to.port = "flow".into();
        }
        if stamp_ids.contains(&conn.from.node) && conn.from.port == "opacity" {
            conn.from.port = "flow".into();
        }
    }
}

/// Drop legacy `preview_output` nodes and install the new
/// `stamp.preview → color_output.brush_preview` wire so loaded presets
/// keep showing a hover preview after the preview-system redesign.
///
/// Strategy:
/// 1. Find every `preview_output` node, remove their incoming wires, and
///    delete the nodes themselves.
/// 2. If the graph has exactly one `color_output` and one `stamp`, and
///    `color_output.brush_preview` is unconnected, add the wire.
///
/// We don't try to be clever about graphs with multiple stamps or
/// terminals — those are unusual; the loaded preset will simply have no
/// preview wired (the engine short-circuits and shows the system cursor).
fn migrate_drop_preview_output(graph: &mut Graph<BrushWireType>) {
    use crate::nodegraph::{NodeId, PortRef};

    // 1. Drop all `preview_output` nodes + their wires.
    let preview_output_ids: Vec<NodeId> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.type_id == "preview_output")
        .map(|(id, _)| *id)
        .collect();
    if !preview_output_ids.is_empty() {
        graph
            .connections
            .retain(|c| !preview_output_ids.contains(&c.to.node)
                     && !preview_output_ids.contains(&c.from.node));
        for id in &preview_output_ids {
            graph.nodes.remove(id);
        }
    }

    // 2. Install the default preview wire if the typical shape applies.
    let stamps: Vec<NodeId> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.type_id == "stamp")
        .map(|(id, _)| *id)
        .collect();
    let color_outputs: Vec<NodeId> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.type_id == "color_output")
        .map(|(id, _)| *id)
        .collect();
    if stamps.len() != 1 || color_outputs.len() != 1 {
        return;
    }
    let stamp_id = stamps[0];
    let color_id = color_outputs[0];

    let already_wired = graph.connections.iter().any(|c| {
        c.to.node == color_id && c.to.port == "brush_preview"
    });
    if already_wired {
        return;
    }

    // Make sure the new ports exist on the loaded node instances —
    // pre-refactor presets snapshot their port lists, so the in-memory
    // `color_output.ports` doesn't include `brush_preview` and
    // `stamp.ports` doesn't include `preview`. Patch them in from the
    // current registration so `connect` accepts the wire.
    let registry = crate::brush::BrushNodeRegistry::new();
    for (id, type_id) in [(stamp_id, "stamp"), (color_id, "color_output")] {
        let Some(reg) = registry.get(type_id) else { continue };
        let Some(node) = graph.nodes.get_mut(&id) else { continue };
        for reg_port in &reg.ports {
            let exists = node.ports.iter().any(|p| p.name == reg_port.name);
            if !exists {
                node.ports.push(reg_port.clone());
            }
        }
    }

    let _ = graph.connect(
        PortRef { node: stamp_id, port: "preview".into() },
        PortRef { node: color_id, port: "brush_preview".into() },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush;

    #[test]
    fn round_trip_no_resources() {
        let graph = brush::default_graph();
        let preset = BrushPreset::from_graph("Test Brush", graph.clone());
        let bundle = PresetBundle::without_resources(preset);

        let bytes = bundle.to_bytes().unwrap();
        let loaded = PresetBundle::from_bytes(&bytes).unwrap();

        assert_eq!(loaded.preset.name, "Test Brush");
        assert_eq!(loaded.preset.format_version, FORMAT_VERSION);

        // Verify graph round-trips: same nodes and connections.
        // Compare as serde_json::Value to avoid HashMap key ordering differences.
        let orig_val = serde_json::to_value(&bundle.preset.graph).unwrap();
        let loaded_val = serde_json::to_value(&loaded.preset.graph).unwrap();
        assert_eq!(orig_val, loaded_val);
    }

    #[test]
    fn round_trip_with_resources() {
        let graph = brush::default_graph();
        let tip_data = vec![0x89, 0x50, 0x4E, 0x47, 1, 2, 3, 4, 5]; // fake PNG
        let mut preset = BrushPreset::from_graph("Tip Brush", graph);
        preset.resources.push(PresetResourceMeta {
            name: "tip.png".into(),
            kind: ResourceKind::BrushTip,
            path: "resources/tip.png".into(),
        });
        let bundle = PresetBundle {
            preset,
            resource_data: vec![("tip.png".into(), tip_data.clone())],
        };

        let bytes = bundle.to_bytes().unwrap();
        let loaded = PresetBundle::from_bytes(&bytes).unwrap();

        assert_eq!(loaded.preset.name, "Tip Brush");
        assert_eq!(loaded.preset.resources.len(), 1);
        assert_eq!(loaded.preset.resources[0].kind, ResourceKind::BrushTip);
        assert_eq!(loaded.resource("tip.png").unwrap(), &tip_data);
    }

    #[test]
    fn future_version_rejected() {
        let graph = brush::default_graph();
        let mut preset = BrushPreset::from_graph("Future", graph);
        preset.format_version = FORMAT_VERSION + 1;
        let bundle = PresetBundle::without_resources(preset);

        let bytes = bundle.to_bytes().unwrap();
        let err = PresetBundle::from_bytes(&bytes).unwrap_err();
        assert!(err.contains("newer than supported"), "got: {err}");
    }

    #[test]
    fn corrupt_zip_returns_error() {
        let err = PresetBundle::from_bytes(b"not a zip").unwrap_err();
        assert!(err.contains("invalid ZIP"), "got: {err}");
    }

    #[test]
    fn missing_preset_json_returns_error() {
        // Create a valid ZIP with no preset.json.
        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("dummy.txt", opts).unwrap();
        zip.write_all(b"hello").unwrap();
        let cursor = zip.finish().unwrap();
        let bytes = cursor.into_inner();

        let err = PresetBundle::from_bytes(&bytes).unwrap_err();
        assert!(err.contains("missing preset.json"), "got: {err}");
    }

    #[test]
    fn legacy_stamp_opacity_migrates_to_flow() {
        use crate::brush::BrushNodeRegistry;
        use crate::nodegraph::{Graph, PortRef};

        // Build a graph the old way: stamp has an "opacity" port (not "flow")
        // and a wire from pen_input.pressure → stamp.opacity. Simulates a
        // preset saved before the Flow/Opacity rename.
        let registry = BrushNodeRegistry::new();
        let mut graph: Graph<BrushWireType> = Graph::new();

        let pen = graph.add_node("pen_input",
            registry.get("pen_input").unwrap().ports.clone(), vec![]);

        // Clone the stamp port defs and rename "flow" back to "opacity" to
        // mimic the pre-refactor layout.
        let mut stamp_ports = registry.get("stamp").unwrap().ports.clone();
        for p in stamp_ports.iter_mut() {
            if p.name == "flow" {
                p.name = "opacity".into();
                p.label = "Opacity".into();
            }
        }
        let stamp = graph.add_node("stamp", stamp_ports, vec![
            crate::gpu::params::ParamValue::Int(0),
        ]);

        graph.connect(
            PortRef { node: pen, port: "pressure".into() },
            PortRef { node: stamp, port: "opacity".into() },
        ).expect("legacy wire should connect");

        // Round-trip through the preset ZIP so the migration runs on load.
        let preset = BrushPreset::from_graph("Legacy", graph);
        let bundle = PresetBundle::without_resources(preset);
        let bytes = bundle.to_bytes().unwrap();
        let loaded = PresetBundle::from_bytes(&bytes).unwrap();

        // The stamp's port should now be called "flow".
        let stamp_node = loaded.preset.graph.nodes.get(&stamp)
            .expect("stamp survived round-trip");
        let has_flow = stamp_node.ports.iter().any(|p| p.name == "flow");
        let has_opacity = stamp_node.ports.iter().any(|p| p.name == "opacity");
        assert!(has_flow, "migrated stamp has a flow port");
        assert!(!has_opacity, "migrated stamp has no opacity port");

        // Wires should be rewritten too — pressure → stamp.flow.
        let rewritten = loaded.preset.graph.connections.iter().any(|c| {
            c.to.node == stamp && c.to.port == "flow"
        });
        assert!(rewritten,
            "legacy wire pen→stamp.opacity should rewrite to pen→stamp.flow");
        let stale = loaded.preset.graph.connections.iter().any(|c| {
            c.to.node == stamp && c.to.port == "opacity"
        });
        assert!(!stale, "no wire should still reference the old opacity port");

        // Compiles cleanly with the new port name.
        let compile = crate::brush::compile_graph(&loaded.preset.graph);
        assert!(compile.is_ok(),
            "migrated graph should compile: {:?}", compile.err());
    }

    #[test]
    fn unknown_fields_ignored() {
        // Simulate a preset with extra fields (forward-compat).
        let graph = brush::default_graph();
        let preset = BrushPreset::from_graph("Compat", graph);
        let mut json_val: serde_json::Value =
            serde_json::to_value(&preset).unwrap();
        json_val["unknown_field"] = serde_json::json!("should be ignored");
        json_val["nested_unknown"] = serde_json::json!({"a": 1, "b": [2,3]});

        let json_str = serde_json::to_string_pretty(&json_val).unwrap();

        // Build a ZIP with the modified JSON.
        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("preset.json", opts).unwrap();
        zip.write_all(json_str.as_bytes()).unwrap();
        let cursor = zip.finish().unwrap();
        let bytes = cursor.into_inner();

        // Should load successfully, ignoring unknown fields.
        let loaded = PresetBundle::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.preset.name, "Compat");
    }
}
