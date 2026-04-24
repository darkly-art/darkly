//! In-memory preset library with optional filesystem backing.
//!
//! Stores loaded `PresetBundle`s keyed by name.  On native targets,
//! can scan a directory for `.darkly-brush` files and save new presets
//! to disk.

use std::collections::HashMap;

use super::preset::{BrushPreset, PresetBundle};
use crate::brush::wire::BrushWireType;
use crate::nodegraph::Graph;

/// Summary info for listing presets without loading the full graph.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PresetInfo {
    pub name: String,
    pub category: String,
    pub author: String,
    pub description: String,
    pub tags: Vec<String>,
}

impl From<&BrushPreset> for PresetInfo {
    fn from(p: &BrushPreset) -> Self {
        PresetInfo {
            name: p.name.clone(),
            category: p.category.clone(),
            author: p.author.clone(),
            description: p.description.clone(),
            tags: p.tags.clone(),
        }
    }
}

/// In-memory library of brush presets.
pub struct PresetLibrary {
    presets: HashMap<String, PresetBundle>,
}

impl PresetLibrary {
    pub fn new() -> Self {
        PresetLibrary {
            presets: HashMap::new(),
        }
    }

    /// List all loaded presets (summary info only).
    pub fn list(&self) -> Vec<PresetInfo> {
        let mut infos: Vec<PresetInfo> = self
            .presets
            .values()
            .map(|b| PresetInfo::from(&b.preset))
            .collect();
        infos.sort_by(|a, b| a.name.cmp(&b.name));
        infos
    }

    /// Get a preset bundle by name.
    pub fn get(&self, name: &str) -> Option<&PresetBundle> {
        self.presets.get(name)
    }

    /// Get the graph for a preset by name.
    pub fn graph(&self, name: &str) -> Option<&Graph<BrushWireType>> {
        self.presets.get(name).map(|b| &b.preset.graph)
    }

    /// Add or replace a preset in the library.
    pub fn insert(&mut self, bundle: PresetBundle) {
        let name = bundle.preset.name.clone();
        self.presets.insert(name, bundle);
    }

    /// Remove a preset by name.  Returns true if it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        self.presets.remove(name).is_some()
    }

    /// Attach a baked `preview.png` to an existing preset. Used by the
    /// async thumbnail bake path — save returns immediately without a
    /// thumbnail, and this method installs the PNG once the readback
    /// completes on a later frame.
    pub fn set_thumbnail(&mut self, name: &str, png: Vec<u8>) -> bool {
        match self.presets.get_mut(name) {
            Some(bundle) => {
                bundle.thumbnail_png = Some(png);
                true
            }
            None => false,
        }
    }

    /// Import a preset from `.darkly-brush` ZIP bytes.
    pub fn import_bytes(&mut self, bytes: &[u8]) -> Result<String, String> {
        let bundle = PresetBundle::from_bytes(bytes)?;
        let name = bundle.preset.name.clone();
        self.insert(bundle);
        Ok(name)
    }

    /// Export a preset to `.darkly-brush` ZIP bytes.
    pub fn export_bytes(&self, name: &str) -> Result<Vec<u8>, String> {
        let bundle = self
            .presets
            .get(name)
            .ok_or_else(|| format!("preset '{}' not found", name))?;
        bundle.to_bytes()
    }

    /// Number of presets in the library.
    pub fn len(&self) -> usize {
        self.presets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
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
                match PresetBundle::load(&path) {
                    Ok(bundle) => {
                        self.insert(bundle);
                        count += 1;
                    }
                    Err(e) => {
                        log::warn!("skipping preset '{}': {e}", path.display());
                    }
                }
            }
        }
        Ok(count)
    }

    /// Save a preset to a directory as `<name>.darkly-brush`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save_to_directory(
        &self,
        name: &str,
        dir: &std::path::Path,
    ) -> Result<std::path::PathBuf, String> {
        let bundle = self
            .presets
            .get(name)
            .ok_or_else(|| format!("preset '{}' not found", name))?;
        let filename = sanitize_filename(name);
        let path = dir.join(format!("{filename}.darkly-brush"));
        bundle.save(&path)?;
        Ok(path)
    }
}

impl Default for PresetLibrary {
    fn default() -> Self {
        Self::new()
    }
}

/// Sanitize a preset name for use as a filename.
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
    use crate::brush::preset::BrushPreset;

    #[test]
    fn library_insert_list_get() {
        let mut lib = PresetLibrary::new();
        assert!(lib.is_empty());

        let preset = BrushPreset::from_graph("Alpha", brush::default_graph());
        lib.insert(PresetBundle::without_resources(preset));

        let preset2 = BrushPreset::from_graph("Beta", brush::default_graph());
        lib.insert(PresetBundle::without_resources(preset2));

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
        let mut lib = PresetLibrary::new();

        let preset = BrushPreset::from_graph("Roundtrip", brush::default_graph());
        let bundle = PresetBundle::without_resources(preset);
        let bytes = bundle.to_bytes().unwrap();

        let name = lib.import_bytes(&bytes).unwrap();
        assert_eq!(name, "Roundtrip");

        let exported = lib.export_bytes("Roundtrip").unwrap();
        let reloaded = PresetBundle::from_bytes(&exported).unwrap();
        assert_eq!(reloaded.preset.name, "Roundtrip");
    }

    #[test]
    fn library_remove() {
        let mut lib = PresetLibrary::new();
        let preset = BrushPreset::from_graph("ToRemove", brush::default_graph());
        lib.insert(PresetBundle::without_resources(preset));
        assert_eq!(lib.len(), 1);

        assert!(lib.remove("ToRemove"));
        assert!(lib.is_empty());
        assert!(!lib.remove("ToRemove"));
    }

    #[test]
    fn library_scan_directory() {
        let dir = std::env::temp_dir().join("darkly_preset_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Write two presets.
        for name in &["Scan A", "Scan B"] {
            let preset = BrushPreset::from_graph(*name, brush::default_graph());
            let bundle = PresetBundle::without_resources(preset);
            bundle
                .save(&dir.join(format!("{name}.darkly-brush")))
                .unwrap();
        }

        // Also write a non-preset file (should be ignored).
        std::fs::write(dir.join("readme.txt"), "not a preset").unwrap();

        let mut lib = PresetLibrary::new();
        let count = lib.scan_directory(&dir).unwrap();
        assert_eq!(count, 2);
        assert_eq!(lib.len(), 2);
        assert!(lib.get("Scan A").is_some());
        assert!(lib.get("Scan B").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
