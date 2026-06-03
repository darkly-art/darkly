//! In-memory brush library with optional filesystem backing.
//!
//! Stores loaded `Brush`es keyed by name.  On native targets, can scan
//! a directory for `.darkly-brush` files and save brushes to disk.

use std::collections::HashMap;

use super::bundle::{Brush, BrushMetadata};
use crate::brush::wire::BrushWireType;
use crate::nodegraph::Graph;

/// Summary info for listing brushes without loading the full graph.
#[derive(Clone, Debug, serde::Serialize)]
pub struct BrushInfo {
    pub name: String,
    pub category: String,
    pub author: String,
    pub description: String,
    pub tags: Vec<String>,
}

impl From<&BrushMetadata> for BrushInfo {
    fn from(p: &BrushMetadata) -> Self {
        BrushInfo {
            name: p.name.clone(),
            category: p.category.clone(),
            author: p.author.clone(),
            description: p.description.clone(),
            tags: p.tags.clone(),
        }
    }
}

/// In-memory library of brushes.
pub struct BrushLibrary {
    brushes: HashMap<String, Brush>,
    /// In-memory dab thumbnails for the picker tiles. Keyed by brush
    /// name. Not part of the `.darkly-brush` archive — purely a render
    /// cache that's rebuilt on theme change alongside the stroke
    /// thumbnails on each `Brush`.
    dab_thumbnails: HashMap<String, Vec<u8>>,
}

impl BrushLibrary {
    pub fn new() -> Self {
        BrushLibrary {
            brushes: HashMap::new(),
            dab_thumbnails: HashMap::new(),
        }
    }

    /// List all loaded brushes (summary info only).
    pub fn list(&self) -> Vec<BrushInfo> {
        let mut infos: Vec<BrushInfo> = self
            .brushes
            .values()
            .map(|b| BrushInfo::from(&b.metadata))
            .collect();
        infos.sort_by(|a, b| a.name.cmp(&b.name));
        infos
    }

    /// Get a brush by name.
    pub fn get(&self, name: &str) -> Option<&Brush> {
        self.brushes.get(name)
    }

    /// Get the graph for a brush by name.
    pub fn graph(&self, name: &str) -> Option<&Graph<BrushWireType>> {
        self.brushes.get(name).map(|b| &b.metadata.graph)
    }

    /// Add or replace a brush in the library.
    pub fn insert(&mut self, brush: Brush) {
        let name = brush.metadata.name.clone();
        self.brushes.insert(name, brush);
    }

    /// Remove a brush by name.  Returns true if it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        self.brushes.remove(name).is_some()
    }

    /// Read a brush's baked thumbnail PNG bytes. Returns `None` if the
    /// brush doesn't exist or its thumbnail hasn't been baked yet.
    pub fn thumbnail_png(&self, name: &str) -> Option<&[u8]> {
        self.brushes
            .get(name)
            .and_then(|b| b.thumbnail_png.as_deref())
    }

    /// Drop every baked stroke + dab thumbnail in the library. Called
    /// on theme change so the next picker refresh re-bakes against the
    /// new palette — without this, brushes stay frozen at whatever
    /// theme they were first viewed under.
    pub fn clear_thumbnails(&mut self) {
        for brush in self.brushes.values_mut() {
            brush.thumbnail_png = None;
        }
        self.dab_thumbnails.clear();
    }

    /// Read a brush's cached dab thumbnail PNG bytes. Returns `None` if
    /// the brush hasn't been baked yet.
    pub fn dab_thumbnail_png(&self, name: &str) -> Option<&[u8]> {
        self.dab_thumbnails.get(name).map(|v| v.as_slice())
    }

    /// Install a freshly-baked dab PNG for `name`. Used by the async
    /// thumbnail bake completion path.
    pub fn set_dab_thumbnail(&mut self, name: &str, png: Vec<u8>) {
        self.dab_thumbnails.insert(name.to_string(), png);
    }

    /// Attach a baked `preview.png` to an existing brush. Used by the
    /// async thumbnail bake path — save returns immediately without a
    /// thumbnail, and this method installs the PNG once the readback
    /// completes on a later frame.
    pub fn set_thumbnail(&mut self, name: &str, png: Vec<u8>) -> bool {
        match self.brushes.get_mut(name) {
            Some(brush) => {
                brush.thumbnail_png = Some(png);
                true
            }
            None => false,
        }
    }

    /// Import a brush from `.darkly-brush` ZIP bytes.
    pub fn import_bytes(&mut self, bytes: &[u8]) -> Result<String, String> {
        let brush = Brush::from_bytes(bytes)?;
        let name = brush.metadata.name.clone();
        self.insert(brush);
        Ok(name)
    }

    /// Export a brush to `.darkly-brush` ZIP bytes.
    pub fn export_bytes(&self, name: &str) -> Result<Vec<u8>, String> {
        let brush = self
            .brushes
            .get(name)
            .ok_or_else(|| format!("brush '{}' not found", name))?;
        brush.to_bytes()
    }

    /// Number of brushes in the library.
    pub fn len(&self) -> usize {
        self.brushes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.brushes.is_empty()
    }

    /// Scan a directory for `.darkly-brush` files and load them all.
    /// Errors on individual files are logged and skipped.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn scan_directory(&mut self, dir: &std::path::Path) -> Result<usize, String> {
        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("failed to read directory '{}': {e}", dir.display()))?;

        let mut count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("darkly-brush") {
                match Brush::load(&path) {
                    Ok(brush) => {
                        self.insert(brush);
                        count += 1;
                    }
                    Err(e) => {
                        log::warn!("skipping brush '{}': {e}", path.display());
                    }
                }
            }
        }
        Ok(count)
    }

    /// Save a brush to a directory as `<name>.darkly-brush`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save_to_directory(
        &self,
        name: &str,
        dir: &std::path::Path,
    ) -> Result<std::path::PathBuf, String> {
        let brush = self
            .brushes
            .get(name)
            .ok_or_else(|| format!("brush '{}' not found", name))?;
        let filename = sanitize_filename(name);
        let path = dir.join(format!("{filename}.darkly-brush"));
        brush.save(&path)?;
        Ok(path)
    }
}

impl Default for BrushLibrary {
    fn default() -> Self {
        Self::new()
    }
}

/// Sanitize a brush name for use as a filename.
#[cfg(not(target_arch = "wasm32"))]
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush;
    use crate::brush::bundle::BrushMetadata;

    #[test]
    fn library_insert_list_get() {
        let mut lib = BrushLibrary::new();
        assert!(lib.is_empty());

        let metadata = BrushMetadata::from_graph("Alpha", brush::default_graph());
        lib.insert(Brush::from_metadata(metadata));

        let metadata2 = BrushMetadata::from_graph("Beta", brush::default_graph());
        lib.insert(Brush::from_metadata(metadata2));

        assert_eq!(lib.len(), 2);

        let list = lib.list();
        assert_eq!(list.len(), 2);
        // Sorted by name.
        assert_eq!(list[0].name, "Alpha");
        assert_eq!(list[1].name, "Beta");

        assert!(lib.get("Alpha").is_some());
        assert!(lib.get("Missing").is_none());
    }

    #[test]
    fn library_import_export_round_trip() {
        let mut lib = BrushLibrary::new();

        let metadata = BrushMetadata::from_graph("Roundtrip", brush::default_graph());
        let brush = Brush::from_metadata(metadata);
        let bytes = brush.to_bytes().unwrap();

        let name = lib.import_bytes(&bytes).unwrap();
        assert_eq!(name, "Roundtrip");

        let exported = lib.export_bytes("Roundtrip").unwrap();
        let reloaded = Brush::from_bytes(&exported).unwrap();
        assert_eq!(reloaded.metadata.name, "Roundtrip");
    }

    #[test]
    fn library_remove() {
        let mut lib = BrushLibrary::new();
        let metadata = BrushMetadata::from_graph("ToRemove", brush::default_graph());
        lib.insert(Brush::from_metadata(metadata));
        assert_eq!(lib.len(), 1);

        assert!(lib.remove("ToRemove"));
        assert!(lib.is_empty());
        assert!(!lib.remove("ToRemove"));
    }

    #[test]
    fn library_scan_directory() {
        let dir = std::env::temp_dir().join("darkly_brush_library_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Write two brushes.
        for name in &["Scan A", "Scan B"] {
            let metadata = BrushMetadata::from_graph(*name, brush::default_graph());
            let brush = Brush::from_metadata(metadata);
            brush
                .save(&dir.join(format!("{name}.darkly-brush")))
                .unwrap();
        }

        // Also write a non-brush file (should be ignored).
        std::fs::write(dir.join("readme.txt"), "not a brush").unwrap();

        let mut lib = BrushLibrary::new();
        let count = lib.scan_directory(&dir).unwrap();
        assert_eq!(count, 2);
        assert_eq!(lib.len(), 2);
        assert!(lib.get("Scan A").is_some());
        assert!(lib.get("Scan B").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
