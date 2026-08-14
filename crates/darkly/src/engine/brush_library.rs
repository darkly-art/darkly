//! Brush library management methods on DarklyEngine.

use darkly_macros::handlers;

use super::{DarklyEngine, ReadbackContext};
use crate::brush::bundle::{Brush, BrushMetadata};
use crate::brush::library::BrushInfo;

/// Dimensions used for baked brush thumbnails. Matches the live editor
/// preview so brushes look identical in the picker grid.
pub const BRUSH_THUMBNAIL_SIZE: (u32, u32) = (320, 120);

/// Render canvas for stroke previews. Generously oversized — not derived
/// from any per-brush geometry. `apply_preview_overrides` neutralizes the
/// preview-time size to a known cap (round base radius ≤ ~26 px), but a
/// broad-nib tip can stretch that footprint ~10× via anisotropy (see
/// `shape::extent`), so the canvas simply reserves enough clearance on every
/// edge (≳ 300 px, see `BRUSH_STROKE_PATH_INSET_FRACTION`) that the
/// neutralized preview stroke can never reach the border. The pipeline does
/// not inspect the graph to size this; the changed-pixel crop in
/// `frame_stroke_thumbnail` frames the actual inked region afterward.
pub(crate) const BRUSH_STROKE_RENDER_SIZE: (u32, u32) = (1024, 768);

/// Fraction of the smaller render-canvas edge reserved as a margin on every
/// side when laying the synthetic S-curve. Proportional (not a hardcoded
/// pixel count) so the clearance scales with the canvas — at
/// `BRUSH_STROKE_RENDER_SIZE` this yields ≳ 300 px, enough for a worst-case
/// anisotropic nib at the preview-time radius cap to stay clear of the
/// border. The changed-pixel crop, not this value, decides the framed size.
pub(crate) const BRUSH_STROKE_PATH_INSET_FRACTION: f32 = 0.4;

/// Canonical brush size dab previews render at, independent of the brush's
/// own `brush_settings.size` (and independent of whether the subgraph even has
/// a `brush_settings` node — per-node previews don't). Chosen so the rasterized
/// tip is large enough that the crop-and-downscale to `DAB_THUMBNAIL_OUTPUT_SIZE`
/// preserves real detail instead of upscaling a ~50 px dab. At
/// `radius = size * DAB_REFERENCE_SIZE * 0.5` this is ≈ 77 px, so a round dab
/// crops to ≈ 185 px — comfortably above the output size. `BRUSH_DAB_RENDER_SIZE`
/// is sized to keep the worst-case anisotropic nib clear of the border at this
/// radius.
pub(crate) const DAB_PREVIEW_BASE_SIZE: f32 = 0.3;

/// Render canvas for dab previews. Square and generously oversized for the
/// same reason as `BRUSH_STROKE_RENDER_SIZE`: the readback handler
/// bbox-crops the rendered dab and downscales to a stable cache size, so the
/// canvas only needs enough headroom that no tip at `DAB_PREVIEW_BASE_SIZE`
/// (including a broad calligraphy nib stretched ~10× by anisotropy, see
/// `circle::extent`, or a `scatter`-displaced dab) touches the border: a
/// ≈ 77 px radius × 10 = 768 px reach from the centred dab stays inside the
/// 896 px half-canvas.
pub(crate) const BRUSH_DAB_RENDER_SIZE: (u32, u32) = (1792, 1792);

#[handlers]
impl DarklyEngine {
    /// List all brushes in the library (summary info only).
    #[handler]
    pub fn brush_list(&self) -> Vec<BrushInfo> {
        self.brush_library.list()
    }

    /// Load a brush by name and set it as the active brush graph.
    #[handler]
    pub fn brush_load(&mut self, name: &str) -> Result<(), String> {
        let brush = self
            .brush_library
            .get(name)
            .ok_or_else(|| format!("brush '{}' not found", name))?
            .clone();

        let json = serde_json::to_string(&brush.metadata.graph)
            .map_err(|e| format!("failed to serialize graph: {e}"))?;
        self.set_brush_graph(&json)?;

        Ok(())
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
    #[handler]
    pub fn brush_save(&mut self, name: &str, category: &str) -> Result<(), String> {
        let mut metadata = BrushMetadata::from_graph(name, self.active_brush_graph());
        metadata.category = category.to_string();
        self.brush_library.insert(Brush::from_metadata(metadata));
        // Saving establishes a new "brush baseline" — what the user just
        // saved IS what reset-to-default should now return to.
        self.snapshot_brush_defaults();

        // Kick off the thumbnail bake. Uses theme colors (not the active
        // fg) so the picker grid looks consistent across brushes. The shared
        // helper applies preview overrides so the saved brush thumbnails are
        // size-invariant — the picker grid should show brush identity, not a
        // snapshot of whatever scrub value the user happened to have when
        // saving.
        self.request_stroke_preview_readback(
            self.active_brush_graph(),
            |width, height, backdrop| ReadbackContext::BrushThumbnailForSave {
                name: name.to_string(),
                width,
                height,
                backdrop,
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
    #[handler(returns = bytes)]
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
        self.request_stroke_preview_readback(
            brush.metadata.graph.clone(),
            |width, height, backdrop| ReadbackContext::BrushThumbnailForSave {
                name: name.to_string(),
                width,
                height,
                backdrop,
            },
        );
        Vec::new()
    }

    /// Return the cached dab thumbnail PNG bytes for a library brush,
    /// kicking off an async bake if none exists yet. Same shape as
    /// `brush_thumbnail` but renders a single full-pressure dab instead
    /// of an S-curve, giving the picker a tip silhouette to show next
    /// to the stroke preview.
    #[handler(returns = bytes)]
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
        // The shared helper resets every exposed scrub (size, opacity,
        // hardness, …) to its registration default before rendering — same
        // treatment the active-dab preview applies. Keeping the two paths on
        // one helper means `brush_dab_thumbnail(active_name)` and
        // `brush_active_dab_preview()` produce byte-identical PNGs, so the
        // picker tile and the BrushBar trigger always agree.
        self.request_dab_preview_readback(brush.metadata.graph.clone(), |width, height| {
            ReadbackContext::BrushDabThumbnail {
                name: name.to_string(),
                width,
                height,
            }
        });
        Vec::new()
    }

    /// Import a brush from `.darkly-brush` ZIP bytes into the library.
    #[handler]
    pub fn brush_import(&mut self, bytes: &[u8]) -> Result<String, String> {
        self.brush_library.import_bytes(bytes)
    }
}
