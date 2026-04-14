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
    ///
    /// Also uploads any brush tip resources to the GPU dab pool cache.
    /// Returns `true` if the preset has no saved positions and the
    /// frontend should run auto-layout with DOM-measured sizes.
    pub fn brush_preset_load(&mut self, name: &str) -> Result<bool, String> {
        let bundle = self
            .preset_library
            .get(name)
            .ok_or_else(|| format!("preset '{}' not found", name))?
            .clone();

        // Upload brush tip resources to the GPU.
        self.upload_preset_resources(&bundle);

        let json = serde_json::to_string(&bundle.preset.graph)
            .map_err(|e| format!("failed to serialize graph: {e}"))?;
        self.set_brush_graph(&json)?;

        // Update stabilizer configuration from the preset.
        self.active_stabilizer_config = bundle.preset.stabilizer.clone();

        Ok(self.active_brush_graph.needs_layout())
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
    ///
    /// Uploads brush tip resources to the GPU if the preset is loaded.
    pub fn brush_preset_import(&mut self, bytes: &[u8]) -> Result<String, String> {
        self.preset_library.import_bytes(bytes)
    }

    /// Upload image resources from a preset bundle to the GPU.
    ///
    /// Populates `self.resource_handles` so Image nodes can resolve their
    /// `resource_name` param to a `TextureHandle` at evaluation time.
    /// Handles both `BrushTip` and `Pattern` resource kinds — both are
    /// uploaded as static textures and accessed identically by node evaluators.
    fn upload_preset_resources(&mut self, bundle: &PresetBundle) {
        self.dab_pool.clear_static();
        self.resource_handles.clear();

        for meta in &bundle.preset.resources {
            let Some(data) = bundle.resource(meta.name.as_str()) else {
                log::warn!("preset resource '{}' not found in bundle", meta.name);
                continue;
            };
            match image::load_from_memory(data) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let handle = self.dab_pool.upload_image(
                        &self.gpu.device,
                        &self.gpu.queue,
                        &meta.name,
                        w,
                        h,
                        rgba.as_raw(),
                    );
                    self.resource_handles.insert(meta.name.clone(), handle);
                }
                Err(e) => {
                    log::warn!("failed to decode resource '{}': {e}", meta.name);
                }
            }
        }
    }
}
