//! Brush preset management methods on DarklyEngine.

use super::DarklyEngine;
use crate::brush::preset::{BrushPreset, PresetBundle};
use crate::brush::preset_library::PresetInfo;

impl DarklyEngine {
    /// List all presets in the library (summary info only).
    pub fn brush_preset_list(&self) -> Vec<PresetInfo> {
        self.preset_library.list()
    }

    /// Load a preset by name and set it as the active brush graph.
    pub fn brush_preset_load(&mut self, name: &str) -> Result<(), String> {
        let graph = self
            .preset_library
            .graph(name)
            .ok_or_else(|| format!("preset '{}' not found", name))?
            .clone();
        let json = serde_json::to_string(&graph)
            .map_err(|e| format!("failed to serialize graph: {e}"))?;
        self.set_brush_graph(&json)
    }

    /// Save the active brush graph as a preset in the library.
    pub fn brush_preset_save(&mut self, name: &str, category: &str) -> Result<(), String> {
        let mut preset = BrushPreset::from_graph(name, self.active_brush_graph.clone());
        preset.category = category.to_string();
        self.preset_library
            .insert(PresetBundle::without_resources(preset));
        Ok(())
    }

    /// Export a preset to `.darkly-brush` ZIP bytes.
    pub fn brush_preset_export(&self, name: &str) -> Result<Vec<u8>, String> {
        self.preset_library.export_bytes(name)
    }

    /// Import a preset from `.darkly-brush` ZIP bytes into the library.
    pub fn brush_preset_import(&mut self, bytes: &[u8]) -> Result<String, String> {
        self.preset_library.import_bytes(bytes)
    }
}
