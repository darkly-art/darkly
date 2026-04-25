//! `.darkly-brush` bundle format — ZIP archive containing a JSON envelope
//! and optional binary resources (brush tips, textures).
//!
//! Format:
//!   preset.json        — metadata + serialized node graph (filename
//!                        grandfathered for compatibility with archives
//!                        already in the wild; do not rename)
//!   resources/<name>   — binary assets referenced by the graph

use std::io::{Cursor, Read, Write};

use serde::{Deserialize, Serialize};

use crate::brush::stabilizer::StabilizerConfig;
use crate::brush::wire::BrushWireType;
use crate::nodegraph::Graph;

/// Current format version.  Bump when the schema changes in a
/// backwards-incompatible way.
pub const FORMAT_VERSION: u32 = 1;

/// Metadata for a brush — the JSON-serialized envelope inside a
/// `.darkly-brush` archive.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrushMetadata {
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
    pub resources: Vec<BrushResourceMeta>,
}

/// Metadata for a resource embedded in the ZIP.
/// The actual bytes live in `Brush::resource_data`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrushResourceMeta {
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

/// A fully-loaded brush with its resource data in memory — the unit of
/// save/load/share.
#[derive(Clone, Debug)]
pub struct Brush {
    pub metadata: BrushMetadata,
    /// Resource data keyed by the `name` field in `BrushResourceMeta`.
    pub resource_data: Vec<(String, Vec<u8>)>,
    /// Optional pre-rendered preview PNG, stored in the ZIP as
    /// `preview.png`. Produced by the async thumbnail bake on brush save
    /// and consumed by the brush picker grid. `None` for brushes that
    /// haven't been baked (older archives, or freshly-saved brushes whose
    /// bake hasn't completed yet).
    pub thumbnail_png: Option<Vec<u8>>,
}

fn default_engine_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

impl BrushMetadata {
    /// Create metadata from just a graph (no resources).
    pub fn from_graph(name: impl Into<String>, graph: Graph<BrushWireType>) -> Self {
        BrushMetadata {
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

impl Brush {
    /// Create a brush from metadata with no resources.
    pub fn without_resources(metadata: BrushMetadata) -> Self {
        Brush {
            metadata,
            resource_data: Vec::new(),
            thumbnail_png: None,
        }
    }

    /// ZIP entry path for the JSON envelope. Filename is grandfathered;
    /// archives in the wild reference it under this name.
    const METADATA_JSON_PATH: &'static str = "preset.json";

    /// ZIP entry path for the optional preview PNG.
    const PREVIEW_PNG_PATH: &'static str = "preview.png";

    /// Serialize to `.darkly-brush` ZIP bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);

        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // Write the JSON envelope.
        let json = serde_json::to_string_pretty(&self.metadata)
            .map_err(|e| format!("failed to serialize brush metadata: {e}"))?;
        zip.start_file(Self::METADATA_JSON_PATH, options)
            .map_err(|e| format!("zip write error: {e}"))?;
        zip.write_all(json.as_bytes())
            .map_err(|e| format!("zip write error: {e}"))?;

        // Write resources
        for (name, data) in &self.resource_data {
            // Find the matching metadata to get the ZIP path.
            let path = self
                .metadata
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

        // Optional pre-baked preview PNG for the brush picker grid.
        if let Some(png) = &self.thumbnail_png {
            zip.start_file(Self::PREVIEW_PNG_PATH, options)
                .map_err(|e| format!("zip write error: {e}"))?;
            zip.write_all(png)
                .map_err(|e| format!("zip write error: {e}"))?;
        }

        let cursor = zip
            .finish()
            .map_err(|e| format!("zip finalize error: {e}"))?;
        Ok(cursor.into_inner())
    }

    /// Deserialize from `.darkly-brush` ZIP bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let cursor = Cursor::new(bytes);
        let mut archive =
            zip::ZipArchive::new(cursor).map_err(|e| format!("invalid ZIP archive: {e}"))?;

        // Read the JSON envelope.
        let mut metadata: BrushMetadata = {
            let mut file = archive
                .by_name(Self::METADATA_JSON_PATH)
                .map_err(|e| format!("missing {}: {e}", Self::METADATA_JSON_PATH))?;
            let mut json = String::new();
            file.read_to_string(&mut json)
                .map_err(|e| format!("failed to read {}: {e}", Self::METADATA_JSON_PATH))?;
            serde_json::from_str(&json)
                .map_err(|e| format!("invalid {}: {e}", Self::METADATA_JSON_PATH))?
        };

        // Migrate: the stamp node's per-dab alpha port was renamed from
        // "opacity" to "flow" during the paint refactor. Any brush saved
        // before that carries the old name; rewrite in place so compilation
        // finds the right port. Silent one-way upgrade — old brushes keep
        // working without a format bump.
        migrate_stamp_opacity_to_flow(&mut metadata.graph);

        // Migrate: the `preview_output` node was removed when terminals
        // gained a `render_preview` lifecycle hook. Drop any legacy
        // `preview_output` nodes and install the new
        // `stamp.preview → color_output.brush_preview` wire so loaded
        // brushes continue to show a hover preview.
        migrate_drop_preview_output(&mut metadata.graph);

        // Migrate: `scatter_x`/`scatter_y` inputs and the `scatter_offset`
        // output were removed from `stamp` (and `scatter_offset` from
        // `color_output`) when scatter became its own node on the position
        // pipeline. Strip the dead ports/wires, and for typical shapes
        // splice in a `scatter` node that reproduces the original effect.
        migrate_stamp_scatter_to_node(&mut metadata.graph);

        if metadata.format_version > FORMAT_VERSION {
            return Err(format!(
                "brush format version {} is newer than supported version {FORMAT_VERSION}",
                metadata.format_version
            ));
        }

        // Read resource data
        let mut resource_data = Vec::new();
        for meta in &metadata.resources {
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

        // Read the optional preview PNG — older archives don't have one
        // and we treat that as `None`, not an error.
        let thumbnail_png = match archive.by_name(Self::PREVIEW_PNG_PATH) {
            Ok(mut file) => {
                let mut data = Vec::with_capacity(file.size() as usize);
                file.read_to_end(&mut data)
                    .map_err(|e| format!("failed to read preview.png: {e}"))?;
                Some(data)
            }
            Err(_) => None,
        };

        Ok(Brush {
            metadata,
            resource_data,
            thumbnail_png,
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
        std::fs::write(path, bytes).map_err(|e| format!("failed to write brush: {e}"))
    }

    /// Load from a file path.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("failed to read brush file: {e}"))?;
        Self::from_bytes(&bytes)
    }
}

/// Rewrite every reference to a `stamp` node's "opacity" port so that it
/// uses the new "flow" name. Applies to:
/// - the node's own `ports` vector (so `set_port_default` finds it),
/// - any `Connection` that routes to/from the old name.
///
/// Pre-refactor brushes stored the per-dab alpha port as "opacity". The
/// refactor separated that from stroke-level opacity by renaming to "flow";
/// this migration keeps legacy brushes loading silently.
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
/// `stamp.preview → color_output.brush_preview` wire so loaded brushes
/// keep showing a hover preview after the preview-system redesign.
///
/// Strategy:
/// 1. Find every `preview_output` node, remove their incoming wires, and
///    delete the nodes themselves.
/// 2. If the graph has exactly one `color_output` and one `stamp`, and
///    `color_output.brush_preview` is unconnected, add the wire.
///
/// We don't try to be clever about graphs with multiple stamps or
/// terminals — those are unusual; the loaded brush will simply have no
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
        graph.connections.retain(|c| {
            !preview_output_ids.contains(&c.to.node) && !preview_output_ids.contains(&c.from.node)
        });
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

    let already_wired = graph
        .connections
        .iter()
        .any(|c| c.to.node == color_id && c.to.port == "brush_preview");
    if already_wired {
        return;
    }

    // Make sure the new ports exist on the loaded node instances —
    // pre-refactor brushes snapshot their port lists, so the in-memory
    // `color_output.ports` doesn't include `brush_preview` and
    // `stamp.ports` doesn't include `preview`. Patch them in from the
    // current registration so `connect` accepts the wire.
    let registry = crate::brush::BrushNodeRegistry::new();
    for (id, type_id) in [(stamp_id, "stamp"), (color_id, "color_output")] {
        let Some(reg) = registry.get(type_id) else {
            continue;
        };
        let Some(node) = graph.nodes.get_mut(&id) else {
            continue;
        };
        for reg_port in &reg.ports {
            let exists = node.ports.iter().any(|p| p.name == reg_port.name);
            if !exists {
                node.ports.push(reg_port.clone());
            }
        }
    }

    let _ = graph.connect(
        PortRef {
            node: stamp_id,
            port: "preview".into(),
        },
        PortRef {
            node: color_id,
            port: "brush_preview".into(),
        },
    );
}

/// Drop legacy scatter ports/wires from `stamp` (`scatter_x`, `scatter_y`,
/// `scatter_offset`) and `color_output` (`scatter_offset`), and — when
/// the graph has the single-stamp/single-color_output shape — splice in a
/// `scatter` node on the position wire so the brush keeps producing
/// scatter. Legacy graphs without any scatter wiring just get the dead
/// ports cleaned up.
fn migrate_stamp_scatter_to_node(graph: &mut Graph<BrushWireType>) {
    use crate::nodegraph::{NodeId, PortRef};

    const LEGACY_STAMP_SCATTER_PORTS: &[&str] = &["scatter_x", "scatter_y", "scatter_offset"];

    let stamp_ids: Vec<NodeId> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.type_id == "stamp")
        .map(|(id, _)| *id)
        .collect();
    let color_output_ids: Vec<NodeId> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.type_id == "color_output")
        .map(|(id, _)| *id)
        .collect();

    // A stamp "had scatter" if something was wired into scatter_x/y or
    // something was reading scatter_offset. Port presence alone isn't
    // enough — the user may never have touched those ports.
    let needs_splice = stamp_ids.iter().any(|sid| {
        graph.connections.iter().any(|c| {
            (c.to.node == *sid && (c.to.port == "scatter_x" || c.to.port == "scatter_y"))
                || (c.from.node == *sid && c.from.port == "scatter_offset")
        })
    });

    // Strip legacy connections.
    graph.connections.retain(|c| {
        if stamp_ids.contains(&c.to.node)
            && LEGACY_STAMP_SCATTER_PORTS.contains(&c.to.port.as_str())
        {
            return false;
        }
        if stamp_ids.contains(&c.from.node) && c.from.port == "scatter_offset" {
            return false;
        }
        if color_output_ids.contains(&c.to.node) && c.to.port == "scatter_offset" {
            return false;
        }
        true
    });

    // Strip legacy ports from in-memory node instances. Snapshot ports
    // live on each instance, so a pre-refactor brush carries them even
    // after the stamp registration drops them.
    for sid in &stamp_ids {
        if let Some(node) = graph.nodes.get_mut(sid) {
            node.ports
                .retain(|p| !LEGACY_STAMP_SCATTER_PORTS.contains(&p.name.as_str()));
        }
    }
    for cid in &color_output_ids {
        if let Some(node) = graph.nodes.get_mut(cid) {
            node.ports.retain(|p| p.name != "scatter_offset");
        }
    }

    if !needs_splice || stamp_ids.len() != 1 || color_output_ids.len() != 1 {
        return;
    }
    let stamp_id = stamp_ids[0];
    let color_id = color_output_ids[0];

    let registry = crate::brush::BrushNodeRegistry::new();
    let Some(scatter_reg) = registry.get("scatter") else {
        return;
    };
    let Some(split_reg) = registry.get("split_vec2") else {
        return;
    };

    let scatter_id = graph.add_node("scatter", scatter_reg.ports.clone(), vec![]);
    let split_id = graph.add_node("split_vec2", split_reg.ports.clone(), vec![]);
    let _ = graph.set_port_default(scatter_id, "amount_x", 1.0);
    let _ = graph.set_port_default(scatter_id, "amount_y", 1.0);
    let _ = graph.set_port_exposed(scatter_id, "amount_x", true);
    let _ = graph.set_port_exposed(scatter_id, "amount_y", true);

    // Splice scatter onto whatever feeds color_output.position (usually
    // pen.position; could be anything). If nothing's wired, the scatter
    // node still gets connected outbound so its output drives position.
    let existing_position: Option<PortRef> = graph
        .connections
        .iter()
        .find(|c| c.to.node == color_id && c.to.port == "position")
        .map(|c| c.from.clone());
    if let Some(pos_from) = existing_position {
        graph.disconnect(
            &pos_from,
            &PortRef {
                node: color_id,
                port: "position".into(),
            },
        );
        let _ = graph.connect(
            pos_from,
            PortRef {
                node: scatter_id,
                port: "position".into(),
            },
        );
    }
    let _ = graph.connect(
        PortRef {
            node: scatter_id,
            port: "position".into(),
        },
        PortRef {
            node: color_id,
            port: "position".into(),
        },
    );
    // stamp.dab_size (Vec2) → split_vec2 → scatter.dab_size (Scalar)
    let _ = graph.connect(
        PortRef {
            node: stamp_id,
            port: "dab_size".into(),
        },
        PortRef {
            node: split_id,
            port: "vec".into(),
        },
    );
    let _ = graph.connect(
        PortRef {
            node: split_id,
            port: "x".into(),
        },
        PortRef {
            node: scatter_id,
            port: "dab_size".into(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush;

    #[test]
    fn round_trip_no_resources() {
        let graph = brush::default_graph();
        let metadata = BrushMetadata::from_graph("Test Brush", graph.clone());
        let brush = Brush::without_resources(metadata);

        let bytes = brush.to_bytes().unwrap();
        let loaded = Brush::from_bytes(&bytes).unwrap();

        assert_eq!(loaded.metadata.name, "Test Brush");
        assert_eq!(loaded.metadata.format_version, FORMAT_VERSION);

        // Verify graph round-trips: same nodes and connections.
        // Compare as serde_json::Value to avoid HashMap key ordering differences.
        let orig_val = serde_json::to_value(&brush.metadata.graph).unwrap();
        let loaded_val = serde_json::to_value(&loaded.metadata.graph).unwrap();
        assert_eq!(orig_val, loaded_val);
    }

    #[test]
    fn round_trip_with_resources() {
        let graph = brush::default_graph();
        let tip_data = vec![0x89, 0x50, 0x4E, 0x47, 1, 2, 3, 4, 5]; // fake PNG
        let mut metadata = BrushMetadata::from_graph("Tip Brush", graph);
        metadata.resources.push(BrushResourceMeta {
            name: "tip.png".into(),
            kind: ResourceKind::BrushTip,
            path: "resources/tip.png".into(),
        });
        let brush = Brush {
            metadata,
            resource_data: vec![("tip.png".into(), tip_data.clone())],
            thumbnail_png: None,
        };

        let bytes = brush.to_bytes().unwrap();
        let loaded = Brush::from_bytes(&bytes).unwrap();

        assert_eq!(loaded.metadata.name, "Tip Brush");
        assert_eq!(loaded.metadata.resources.len(), 1);
        assert_eq!(loaded.metadata.resources[0].kind, ResourceKind::BrushTip);
        assert_eq!(loaded.resource("tip.png").unwrap(), &tip_data);
    }

    #[test]
    fn future_version_rejected() {
        let graph = brush::default_graph();
        let mut metadata = BrushMetadata::from_graph("Future", graph);
        metadata.format_version = FORMAT_VERSION + 1;
        let brush = Brush::without_resources(metadata);

        let bytes = brush.to_bytes().unwrap();
        let err = Brush::from_bytes(&bytes).unwrap_err();
        assert!(err.contains("newer than supported"), "got: {err}");
    }

    #[test]
    fn corrupt_zip_returns_error() {
        let err = Brush::from_bytes(b"not a zip").unwrap_err();
        assert!(err.contains("invalid ZIP"), "got: {err}");
    }

    #[test]
    fn missing_metadata_json_returns_error() {
        // Create a valid ZIP with no envelope JSON.
        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("dummy.txt", opts).unwrap();
        zip.write_all(b"hello").unwrap();
        let cursor = zip.finish().unwrap();
        let bytes = cursor.into_inner();

        let err = Brush::from_bytes(&bytes).unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn legacy_stamp_opacity_migrates_to_flow() {
        use crate::brush::BrushNodeRegistry;
        use crate::nodegraph::{Graph, PortRef};

        // Build a graph the old way: stamp has an "opacity" port (not "flow")
        // and a wire from pen_input.pressure → stamp.opacity. Simulates a
        // brush saved before the Flow/Opacity rename.
        let registry = BrushNodeRegistry::new();
        let mut graph: Graph<BrushWireType> = Graph::new();

        let pen = graph.add_node(
            "pen_input",
            registry.get("pen_input").unwrap().ports.clone(),
            vec![],
        );

        // Clone the stamp port defs and rename "flow" back to "opacity" to
        // mimic the pre-refactor layout.
        let mut stamp_ports = registry.get("stamp").unwrap().ports.clone();
        for p in stamp_ports.iter_mut() {
            if p.name == "flow" {
                p.name = "opacity".into();
                p.label = "Opacity".into();
            }
        }
        let stamp = graph.add_node(
            "stamp",
            stamp_ports,
            vec![crate::gpu::params::ParamValue::Int(0)],
        );

        graph
            .connect(
                PortRef {
                    node: pen,
                    port: "pressure".into(),
                },
                PortRef {
                    node: stamp,
                    port: "opacity".into(),
                },
            )
            .expect("legacy wire should connect");

        // Round-trip through the brush ZIP so the migration runs on load.
        let metadata = BrushMetadata::from_graph("Legacy", graph);
        let brush = Brush::without_resources(metadata);
        let bytes = brush.to_bytes().unwrap();
        let loaded = Brush::from_bytes(&bytes).unwrap();

        // The stamp's port should now be called "flow".
        let stamp_node = loaded
            .metadata
            .graph
            .nodes
            .get(&stamp)
            .expect("stamp survived round-trip");
        let has_flow = stamp_node.ports.iter().any(|p| p.name == "flow");
        let has_opacity = stamp_node.ports.iter().any(|p| p.name == "opacity");
        assert!(has_flow, "migrated stamp has a flow port");
        assert!(!has_opacity, "migrated stamp has no opacity port");

        // Wires should be rewritten too — pressure → stamp.flow.
        let rewritten = loaded
            .metadata
            .graph
            .connections
            .iter()
            .any(|c| c.to.node == stamp && c.to.port == "flow");
        assert!(
            rewritten,
            "legacy wire pen→stamp.opacity should rewrite to pen→stamp.flow"
        );
        let stale = loaded
            .metadata
            .graph
            .connections
            .iter()
            .any(|c| c.to.node == stamp && c.to.port == "opacity");
        assert!(
            !stale,
            "no wire should still reference the old opacity port"
        );

        // Compiles cleanly with the new port name.
        let compile = crate::brush::compile_graph(&loaded.metadata.graph);
        assert!(
            compile.is_ok(),
            "migrated graph should compile: {:?}",
            compile.err()
        );
    }

    #[test]
    fn legacy_stamp_scatter_migrates_to_scatter_node() {
        use crate::brush::BrushNodeRegistry;
        use crate::nodegraph::{Graph, PortDef, PortDir, PortRef};

        // Build a graph the old way: stamp has `scatter_x`/`scatter_y`
        // input ports and a `scatter_offset` output port; color_output
        // has a `scatter_offset` input port, with the scatter_offset
        // wire connecting them. This matches the pre-refactor shape of
        // the Scatter Brush.
        let registry = BrushNodeRegistry::new();
        let mut graph: Graph<BrushWireType> = Graph::new();

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
            vec![],
        );

        // Stamp ports: current registration + legacy scatter inputs/output.
        let mut stamp_ports = registry.get("stamp").unwrap().ports.clone();
        let legacy_scatter_in = |name: &str| PortDef {
            name: name.into(),
            dir: PortDir::Input,
            wire_type: BrushWireType::Scalar,
            min: -1.0,
            max: 1.0,
            default: 0.0,
            description: String::new(),
            unit_type: Default::default(),
            icon: String::new(),
            label: String::new(),
            exposed: false,
        };
        stamp_ports.push(legacy_scatter_in("scatter_x"));
        stamp_ports.push(legacy_scatter_in("scatter_y"));
        stamp_ports.push(PortDef {
            name: "scatter_offset".into(),
            dir: PortDir::Output,
            wire_type: BrushWireType::Vec2,
            min: 0.0,
            max: 0.0,
            default: 0.0,
            description: String::new(),
            unit_type: Default::default(),
            icon: String::new(),
            label: String::new(),
            exposed: false,
        });
        let stamp = graph.add_node(
            "stamp",
            stamp_ports,
            vec![crate::gpu::params::ParamValue::Int(0)],
        );

        // color_output ports: current registration + legacy scatter_offset input.
        let mut out_ports = registry.get("color_output").unwrap().ports.clone();
        out_ports.push(PortDef {
            name: "scatter_offset".into(),
            dir: PortDir::Input,
            wire_type: BrushWireType::Vec2,
            min: 0.0,
            max: 0.0,
            default: 0.0,
            description: String::new(),
            unit_type: Default::default(),
            icon: String::new(),
            label: String::new(),
            exposed: false,
        });
        let color_output = graph.add_node("color_output", out_ports, vec![]);

        // Scatter was typically driven by two per-dab random nodes.
        let rand_x = graph.add_node(
            "random",
            registry.get("random").unwrap().ports.clone(),
            vec![crate::gpu::params::ParamValue::Int(0)],
        );
        let rand_y = graph.add_node(
            "random",
            registry.get("random").unwrap().ports.clone(),
            vec![crate::gpu::params::ParamValue::Int(0)],
        );

        let wires = [
            (circle, "texture", stamp, "tip"),
            (pen, "pressure", stamp, "size"),
            (paint_color, "color", stamp, "color"),
            (stamp, "dab", color_output, "dab"),
            (stamp, "dab_size", color_output, "dab_size"),
            (pen, "position", color_output, "position"),
            (rand_x, "value", stamp, "scatter_x"),
            (rand_y, "value", stamp, "scatter_y"),
            (stamp, "scatter_offset", color_output, "scatter_offset"),
        ];
        for (fn_, fp, tn, tp) in wires {
            graph
                .connect(
                    PortRef {
                        node: fn_,
                        port: fp.into(),
                    },
                    PortRef {
                        node: tn,
                        port: tp.into(),
                    },
                )
                .expect("legacy wire should connect");
        }

        let metadata = BrushMetadata::from_graph("Legacy Scatter", graph);
        let brush = Brush::without_resources(metadata);
        let bytes = brush.to_bytes().unwrap();
        let loaded = Brush::from_bytes(&bytes).unwrap();
        let g = &loaded.metadata.graph;

        // Legacy ports are gone from stamp and color_output.
        let stamp_node = g.nodes.get(&stamp).expect("stamp survived");
        for dead in &["scatter_x", "scatter_y", "scatter_offset"] {
            assert!(
                !stamp_node.ports.iter().any(|p| p.name == *dead),
                "stamp still has legacy {dead} port"
            );
        }
        let color_node = g.nodes.get(&color_output).expect("color_output survived");
        assert!(
            !color_node.ports.iter().any(|p| p.name == "scatter_offset"),
            "color_output still has legacy scatter_offset port"
        );

        // No connections reference the dead ports.
        for c in &g.connections {
            assert!(
                c.to.port != "scatter_x"
                    && c.to.port != "scatter_y"
                    && c.to.port != "scatter_offset"
                    && c.from.port != "scatter_offset",
                "dead scatter wire survived migration: {:?} → {:?}",
                c.from,
                c.to
            );
        }

        // A scatter node was spliced between pen.position and
        // color_output.position.
        let scatter_id = g
            .nodes
            .iter()
            .find(|(_, n)| n.type_id == "scatter")
            .map(|(id, _)| *id)
            .expect("migration should add a scatter node");
        assert!(
            g.connections.iter().any(|c| c.from.node == pen
                && c.from.port == "position"
                && c.to.node == scatter_id
                && c.to.port == "position"),
            "pen.position → scatter.position wire missing"
        );
        assert!(
            g.connections.iter().any(|c| c.from.node == scatter_id
                && c.from.port == "position"
                && c.to.node == color_output
                && c.to.port == "position"),
            "scatter.position → color_output.position wire missing"
        );
        // stamp.dab_size (Vec2) → split_vec2 → scatter.dab_size (Scalar).
        let split_id = g
            .nodes
            .iter()
            .find(|(_, n)| n.type_id == "split_vec2")
            .map(|(id, _)| *id)
            .expect("migration should add a split_vec2 node");
        assert!(
            g.connections.iter().any(|c| c.from.node == stamp
                && c.from.port == "dab_size"
                && c.to.node == split_id
                && c.to.port == "vec"),
            "stamp.dab_size → split_vec2.vec wire missing"
        );
        assert!(
            g.connections.iter().any(|c| c.from.node == split_id
                && c.from.port == "x"
                && c.to.node == scatter_id
                && c.to.port == "dab_size"),
            "split_vec2.x → scatter.dab_size wire missing"
        );

        // Migrated graph compiles.
        let compile = crate::brush::compile_graph(g);
        assert!(
            compile.is_ok(),
            "migrated graph should compile: {:?}",
            compile.err()
        );
    }

    #[test]
    fn thumbnail_png_round_trip() {
        // A brush with a baked thumbnail should serialize the PNG as a
        // `preview.png` ZIP entry and reload it back into `thumbnail_png`.
        let graph = brush::default_graph();
        let metadata = BrushMetadata::from_graph("Thumbnailed", graph);
        let png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        let mut brush = Brush::without_resources(metadata);
        brush.thumbnail_png = Some(png.clone());

        let bytes = brush.to_bytes().unwrap();
        let loaded = Brush::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.thumbnail_png, Some(png));
    }

    #[test]
    fn thumbnail_absent_loads_as_none() {
        // Archives without `preview.png` — the case for older brushes and
        // freshly-saved ones whose bake hasn't landed yet — must load as
        // `thumbnail_png: None`, not error.
        let graph = brush::default_graph();
        let metadata = BrushMetadata::from_graph("Bare", graph);
        let brush = Brush::without_resources(metadata);
        let bytes = brush.to_bytes().unwrap();

        let loaded = Brush::from_bytes(&bytes).unwrap();
        assert!(loaded.thumbnail_png.is_none());
    }

    #[test]
    fn unknown_fields_ignored() {
        // Simulate a brush envelope with extra fields (forward-compat).
        let graph = brush::default_graph();
        let metadata = BrushMetadata::from_graph("Compat", graph);
        let mut json_val: serde_json::Value = serde_json::to_value(&metadata).unwrap();
        json_val["unknown_field"] = serde_json::json!("should be ignored");
        json_val["nested_unknown"] = serde_json::json!({"a": 1, "b": [2,3]});

        let json_str = serde_json::to_string_pretty(&json_val).unwrap();

        // Build a ZIP with the modified JSON.
        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(Brush::METADATA_JSON_PATH, opts).unwrap();
        zip.write_all(json_str.as_bytes()).unwrap();
        let cursor = zip.finish().unwrap();
        let bytes = cursor.into_inner();

        // Should load successfully, ignoring unknown fields.
        let loaded = Brush::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.metadata.name, "Compat");
    }
}
