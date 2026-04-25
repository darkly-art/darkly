//! Brush library management methods on DarklyEngine.

use super::{DarklyEngine, ReadbackContext};
use crate::brush::bundle::{Brush, BrushMetadata};
use crate::brush::library::BrushInfo;

/// Dimensions used for baked brush thumbnails. Matches the live editor
/// preview so brushes look identical in the picker grid.
pub(crate) const BRUSH_THUMBNAIL_SIZE: (u32, u32) = (320, 120);

impl DarklyEngine {
    /// List all brushes in the library (summary info only).
    pub fn brush_list(&self) -> Vec<BrushInfo> {
        self.brush_library.list()
    }

    /// Load a brush by name and set it as the active brush graph.
    ///
    /// Also uploads any brush tip resources to the GPU dab pool cache.
    /// Returns `true` if the brush has no saved positions and the
    /// frontend should run auto-layout with DOM-measured sizes.
    pub fn brush_load(&mut self, name: &str) -> Result<bool, String> {
        let brush = self
            .brush_library
            .get(name)
            .ok_or_else(|| format!("brush '{}' not found", name))?
            .clone();

        // Upload brush tip resources to the GPU.
        self.upload_brush_resources(&brush);

        let json = serde_json::to_string(&brush.metadata.graph)
            .map_err(|e| format!("failed to serialize graph: {e}"))?;
        self.set_brush_graph(&json)?;

        Ok(self.active_brush_graph.needs_layout())
    }

    /// Save the active brush graph as a brush in the library.
    ///
    /// Returns immediately with the brush registered (no thumbnail yet).
    /// A theme-colored preview render is scheduled; when its readback
    /// lands, the resulting PNG is installed on the library entry via
    /// `BrushLibrary::set_thumbnail`. Callers that export the brush
    /// before the bake completes simply get an archive without
    /// `preview.png` — loads still work, pickers fall back to whatever
    /// placeholder they prefer.
    pub fn brush_save(&mut self, name: &str, category: &str) -> Result<(), String> {
        let mut metadata = BrushMetadata::from_graph(name, self.active_brush_graph.clone());
        metadata.category = category.to_string();
        self.brush_library
            .insert(Brush::without_resources(metadata));
        // Saving establishes a new "brush baseline" — what the user just
        // saved IS what reset-to-default should now return to.
        self.snapshot_brush_defaults();

        // Kick off the thumbnail bake. Uses theme colors (not the active
        // fg) so the picker grid looks consistent across brushes.
        let (w, h) = BRUSH_THUMBNAIL_SIZE;
        let fg = self.preview_theme_fg;
        let bg = self.preview_theme_bg;
        self.render_preview_and_request_readback(
            w,
            h,
            fg,
            bg,
            ReadbackContext::BrushThumbnailForSave {
                name: name.to_string(),
                width: w,
                height: h,
            },
        );
        Ok(())
    }

    /// Export a brush to `.darkly-brush` ZIP bytes.
    pub fn brush_export(&self, name: &str) -> Result<Vec<u8>, String> {
        self.brush_library.export_bytes(name)
    }

    /// Import a brush from `.darkly-brush` ZIP bytes into the library.
    ///
    /// Uploads brush tip resources to the GPU if the brush is loaded.
    pub fn brush_import(&mut self, bytes: &[u8]) -> Result<String, String> {
        self.brush_library.import_bytes(bytes)
    }

    /// Upload image resources from a brush bundle to the GPU.
    ///
    /// Populates `self.resource_handles` so Image nodes can resolve their
    /// `resource_name` param to a `TextureHandle` at evaluation time.
    /// Handles both `BrushTip` and `Pattern` resource kinds — both are
    /// uploaded as static textures and accessed identically by node evaluators.
    fn upload_brush_resources(&mut self, brush: &Brush) {
        self.dab_pool.clear_static();
        self.resource_handles.clear();

        for meta in &brush.metadata.resources {
            let Some(data) = brush.resource(meta.name.as_str()) else {
                log::warn!("brush resource '{}' not found in bundle", meta.name);
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
