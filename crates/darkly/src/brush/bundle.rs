//! `.darkly-brush` bundle format — ZIP archive containing a JSON envelope
//! and an optional pre-baked thumbnail.
//!
//! Format:
//!   brush.json   — metadata + serialized node graph
//!   preview.png  — optional pre-baked thumbnail

use std::io::{Cursor, Read, Write};

use serde::{Deserialize, Serialize};

use crate::brush::stabilizer::StabilizerConfig;
use crate::brush::wire::BrushWireType;
use crate::nodegraph::Graph;

/// Metadata for a brush — the JSON-serialized envelope inside a
/// `.darkly-brush` archive.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrushMetadata {
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
}

/// A fully-loaded brush — the unit of save/load/share.
#[derive(Clone, Debug)]
pub struct Brush {
    pub metadata: BrushMetadata,
    /// Optional pre-rendered preview PNG, stored in the ZIP as
    /// `preview.png`. Produced by the async thumbnail bake on brush save
    /// and consumed by the brush picker grid. `None` for freshly-saved
    /// brushes whose bake hasn't completed yet.
    pub thumbnail_png: Option<Vec<u8>>,
}

fn default_engine_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

impl BrushMetadata {
    /// Create metadata from just a graph.
    pub fn from_graph(name: impl Into<String>, graph: Graph<BrushWireType>) -> Self {
        BrushMetadata {
            name: name.into(),
            engine_version: default_engine_version(),
            category: String::new(),
            author: String::new(),
            description: String::new(),
            tags: Vec::new(),
            graph,
            stabilizer: StabilizerConfig::default(),
        }
    }
}

impl Brush {
    /// Create a brush from metadata.
    pub fn from_metadata(metadata: BrushMetadata) -> Self {
        Brush {
            metadata,
            thumbnail_png: None,
        }
    }

    /// ZIP entry path for the JSON envelope.
    const METADATA_JSON_PATH: &'static str = "brush.json";

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
        let metadata: BrushMetadata = {
            let mut file = archive
                .by_name(Self::METADATA_JSON_PATH)
                .map_err(|e| format!("missing {}: {e}", Self::METADATA_JSON_PATH))?;
            let mut json = String::new();
            file.read_to_string(&mut json)
                .map_err(|e| format!("failed to read {}: {e}", Self::METADATA_JSON_PATH))?;
            serde_json::from_str(&json)
                .map_err(|e| format!("invalid {}: {e}", Self::METADATA_JSON_PATH))?
        };

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
            thumbnail_png,
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush;

    #[test]
    fn round_trip_no_resources() {
        let graph = brush::default_graph();
        let metadata = BrushMetadata::from_graph("Test Brush", graph.clone());
        let brush = Brush::from_metadata(metadata);

        let bytes = brush.to_bytes().unwrap();
        let loaded = Brush::from_bytes(&bytes).unwrap();

        assert_eq!(loaded.metadata.name, "Test Brush");

        // Verify graph round-trips: same nodes and connections.
        // Compare as serde_json::Value to avoid HashMap key ordering differences.
        let orig_val = serde_json::to_value(&brush.metadata.graph).unwrap();
        let loaded_val = serde_json::to_value(&loaded.metadata.graph).unwrap();
        assert_eq!(orig_val, loaded_val);
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
    fn thumbnail_png_round_trip() {
        // A brush with a baked thumbnail should serialize the PNG as a
        // `preview.png` ZIP entry and reload it back into `thumbnail_png`.
        let graph = brush::default_graph();
        let metadata = BrushMetadata::from_graph("Thumbnailed", graph);
        let png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        let mut brush = Brush::from_metadata(metadata);
        brush.thumbnail_png = Some(png.clone());

        let bytes = brush.to_bytes().unwrap();
        let loaded = Brush::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.thumbnail_png, Some(png));
    }

    #[test]
    fn thumbnail_absent_loads_as_none() {
        // Archives without `preview.png` — the case for freshly-saved
        // brushes whose bake hasn't landed yet — must load as
        // `thumbnail_png: None`, not error.
        let graph = brush::default_graph();
        let metadata = BrushMetadata::from_graph("Bare", graph);
        let brush = Brush::from_metadata(metadata);
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
