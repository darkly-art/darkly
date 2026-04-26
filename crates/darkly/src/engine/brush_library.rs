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
        self.ensure_brush_resources(&brush);

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
        let graph = self.active_brush_graph.clone();
        let path =
            crate::brush::preview_renderer::synthesize_preview_stroke(w as f32, h as f32, 30);
        self.render_preview_and_request_readback(
            &graph,
            &path,
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

    /// Return the cached PNG thumbnail bytes for a library brush, kicking
    /// off an async bake if none exists yet. Returns an empty vector when
    /// the bake is in flight (or the brush is missing); the frontend polls
    /// on rAF until non-empty bytes arrive. Subsequent calls hit the cache.
    pub fn brush_thumbnail(&mut self, name: &str) -> Vec<u8> {
        if let Some(png) = self.brush_library.thumbnail_png(name) {
            return png.to_vec();
        }
        // A bake for this brush is already pending — don't queue another;
        // racing readbacks would step on each other's library entry.
        let already_pending = self.readbacks.any(
            |c| matches!(c, ReadbackContext::BrushThumbnailForSave { name: n, .. } if n == name),
        );
        if already_pending {
            return Vec::new();
        }
        let Some(brush) = self.brush_library.get(name).cloned() else {
            return Vec::new();
        };
        // Image-based brushes (Calligraphy, Textured Ink, Pencil, ...)
        // need their tip/pattern textures on the GPU before the bake;
        // without this, picker tiles for inactive image brushes render
        // bg-only.
        self.ensure_brush_resources(&brush);
        let (w, h) = BRUSH_THUMBNAIL_SIZE;
        let fg = self.preview_theme_fg;
        let bg = self.preview_theme_bg;
        let path =
            crate::brush::preview_renderer::synthesize_preview_stroke(w as f32, h as f32, 30);
        self.render_preview_and_request_readback(
            &brush.metadata.graph,
            &path,
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
        Vec::new()
    }

    /// Import a brush from `.darkly-brush` ZIP bytes into the library.
    ///
    /// Uploads brush tip resources to the GPU if the brush is loaded.
    pub fn brush_import(&mut self, bytes: &[u8]) -> Result<String, String> {
        self.brush_library.import_bytes(bytes)
    }

    /// Ensure every image resource referenced by `brush` is uploaded to
    /// the GPU and registered in `self.resource_handles`. Idempotent —
    /// names already present are skipped, so loading the active brush,
    /// baking inactive picker thumbnails, and reloading the same brush
    /// all share one cache entry per resource.
    ///
    /// Handles both `BrushTip` and `Pattern` resource kinds — both are
    /// uploaded as static textures and accessed identically by node
    /// evaluators.
    ///
    /// v1 keeps every uploaded resource for the engine's lifetime. With
    /// only built-in brushes that's a fixed, tiny set; v2 imported
    /// brushes will need a name-collision strategy and eviction policy.
    pub(crate) fn ensure_brush_resources(&mut self, brush: &Brush) {
        for meta in &brush.metadata.resources {
            if self.resource_handles.contains_key(&meta.name) {
                continue;
            }
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
