//! `.darkly-brush` preset format — ZIP archive containing a JSON envelope
//! and optional binary resources (brush tips, textures).
//!
//! Format:
//!   preset.json        — metadata + serialized node graph
//!   resources/<name>   — binary assets referenced by the graph

use std::io::{Cursor, Read, Write};

use serde::{Deserialize, Serialize};

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
        let preset: BrushPreset = {
            let mut file = archive
                .by_name("preset.json")
                .map_err(|e| format!("missing preset.json: {e}"))?;
            let mut json = String::new();
            file.read_to_string(&mut json)
                .map_err(|e| format!("failed to read preset.json: {e}"))?;
            serde_json::from_str(&json)
                .map_err(|e| format!("invalid preset.json: {e}"))?
        };

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
