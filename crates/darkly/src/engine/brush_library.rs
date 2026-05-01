//! Brush library management methods on DarklyEngine.

use super::{DarklyEngine, ReadbackContext};
use crate::brush::bundle::{Brush, BrushMetadata};
use crate::brush::library::BrushInfo;

/// Dimensions used for baked brush thumbnails. Matches the live editor
/// preview so brushes look identical in the picker grid.
pub(crate) const BRUSH_THUMBNAIL_SIZE: (u32, u32) = (320, 120);

/// Render canvas for stroke previews. Sized once, statically, with
/// enough headroom around `BRUSH_THUMBNAIL_SIZE` to fit endpoint dabs
/// at the largest preview-time radius any port's `preview_max` allows
/// (currently `stamp.size` ≤ 0.1 → ≤ 26 px radius). The pipeline never
/// inspects the brush graph to size this canvas — `apply_preview_overrides`
/// has already neutralized any port that would otherwise blow it out.
pub(crate) const BRUSH_STROKE_RENDER_SIZE: (u32, u32) = (384, 192);

/// Inset reserved on every edge of the stroke render canvas so endpoint
/// dabs at the preview-time cap radius fit fully inside without
/// touching the canvas border. Half the gap between the render canvas
/// and the cache:  (384-320)/2 = 32  ≥  ceil(0.1 * 256) + safety.
pub(crate) const BRUSH_STROKE_PATH_INSET: f32 = 32.0;

/// Render canvas for dab previews. Square and oversized relative to
/// what we cache — the readback handler bbox-crops the rendered dab and
/// downscales to a stable cache size, so brushes with small `size`
/// ports or with `scatter` displacement still produce a recognizable
/// thumbnail (no truncation, auto-centered).
pub(crate) const BRUSH_DAB_RENDER_SIZE: (u32, u32) = (256, 256);

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
        let fg = self.preview_theme_fg;
        let bg = self.preview_theme_bg;
        let mut graph = self.active_brush_graph.clone();
        graph.apply_preview_overrides();
        let (rw, rh) = BRUSH_STROKE_RENDER_SIZE;
        let path = crate::brush::preview_renderer::synthesize_preview_stroke(
            rw as f32,
            rh as f32,
            30,
            BRUSH_STROKE_PATH_INSET,
        );
        self.render_preview_and_request_readback(
            &graph,
            &path,
            rw,
            rh,
            fg,
            bg,
            ReadbackContext::BrushThumbnailForSave {
                name: name.to_string(),
                width: rw,
                height: rh,
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
        let fg = self.preview_theme_fg;
        let bg = self.preview_theme_bg;
        let mut graph = brush.metadata.graph.clone();
        graph.apply_preview_overrides();
        let (rw, rh) = BRUSH_STROKE_RENDER_SIZE;
        let path = crate::brush::preview_renderer::synthesize_preview_stroke(
            rw as f32,
            rh as f32,
            30,
            BRUSH_STROKE_PATH_INSET,
        );
        self.render_preview_and_request_readback(
            &graph,
            &path,
            rw,
            rh,
            fg,
            bg,
            ReadbackContext::BrushThumbnailForSave {
                name: name.to_string(),
                width: rw,
                height: rh,
            },
        );
        Vec::new()
    }

    /// Return the cached dab thumbnail PNG bytes for a library brush,
    /// kicking off an async bake if none exists yet. Same shape as
    /// `brush_thumbnail` but renders a single full-pressure dab instead
    /// of an S-curve, giving the picker a tip silhouette to show next
    /// to the stroke preview.
    pub fn brush_dab_thumbnail(&mut self, name: &str) -> Vec<u8> {
        if let Some(png) = self.brush_library.dab_thumbnail_png(name) {
            return png.to_vec();
        }
        let already_pending = self
            .readbacks
            .any(|c| matches!(c, ReadbackContext::BrushDabThumbnail { name: n, .. } if n == name));
        if already_pending {
            return Vec::new();
        }
        let Some(brush) = self.brush_library.get(name).cloned() else {
            return Vec::new();
        };
        // Image-based brushes need their tip texture on the GPU before
        // the bake — same path as the stroke thumbnail.
        self.ensure_brush_resources(&brush);
        let (w, h) = BRUSH_DAB_RENDER_SIZE;
        let fg = self.preview_theme_fg;
        let bg = self.preview_theme_bg;
        // Reset every exposed scrub (size, opacity, hardness, …) to its
        // registration default before rendering — same treatment the
        // active-dab preview applies. The dab thumbnail represents the
        // brush's identity (shape, texture, dynamics), so user-facing
        // scrubs that vary across instances of the same brush type
        // shouldn't bias the picker icon. Keeping the two paths
        // identical here also means `brush_dab_thumbnail(active_name)`
        // and `brush_active_dab_preview()` produce byte-identical PNGs,
        // so the picker tile and the BrushBar trigger always agree.
        let mut graph = brush.metadata.graph.clone();
        crate::brush::reset_exposed_scrubs(&mut graph);
        let path = crate::brush::preview_renderer::synthesize_preview_dab(w as f32, h as f32);
        self.render_preview_and_request_readback(
            &graph,
            &path,
            w,
            h,
            fg,
            bg,
            ReadbackContext::BrushDabThumbnail {
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
