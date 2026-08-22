//! Brush library management methods on DarklyEngine.

use darkly_macros::handlers;

use super::{DarklyEngine, ReadbackContext};
use crate::brush::library::{self as library, BrushInfo, LibrarySnapshot};
use crate::brush::metadata::{Brush, BrushMetadata};

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
    /// Every brush and every pack, in one round trip.
    ///
    /// One call rather than two so the halves cannot disagree across a
    /// concurrent mutation — a member id naming a brush the caller has not
    /// been told about is the inconsistency this rules out.
    #[handler]
    pub fn library_list(&self) -> LibrarySnapshot {
        library::with(|lib| lib.snapshot())
    }

    /// List all brushes in the library (summary info only).
    #[handler]
    pub fn brush_list(&self) -> Vec<BrushInfo> {
        library::with(|lib| lib.list())
    }

    /// Load a brush by name and set it as the active brush graph.
    #[handler]
    pub fn brush_load(&mut self, name: &str) -> Result<(), String> {
        // The library borrow ends before `set_brush_graph`, which takes
        // `&mut self` and would otherwise re-enter it.
        let json = library::with(|lib| {
            let brush = lib
                .by_name(name)
                .ok_or_else(|| format!("brush '{name}' not found"))?;
            serde_json::to_string(&brush.metadata.graph)
                .map_err(|e| format!("failed to serialize graph: {e}"))
        })?;
        self.set_brush_graph(&json)?;
        Ok(())
    }

    /// Save the active brush graph as a brush in the library, under the
    /// caller-supplied `id`.
    ///
    /// The id comes from the frontend because this crate has no random-number
    /// source; saving over an existing id replaces that brush, which is what
    /// "save" means when the painter is editing one they already have.
    ///
    /// Returns immediately with the brush registered (no thumbnail yet). A
    /// theme-colored preview render is scheduled; when its readback lands, the
    /// resulting PNG is installed on the library entry via
    /// `BrushLibrary::set_thumbnail`.
    #[handler]
    pub fn brush_save(&mut self, id: &str, name: &str) -> Result<(), String> {
        if id.trim().is_empty() {
            return Err("a brush needs an id".into());
        }
        let metadata = BrushMetadata::from_graph(id, name, self.active_brush_graph());
        library::with_mut(|lib| lib.insert(Brush::from_metadata(metadata)));
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
                id: id.to_string(),
                width,
                height,
                backdrop,
            },
        );
        Ok(())
    }

    /// A library brush's graph as portable YAML, without making it active.
    ///
    /// Reading a brush should not disturb what the painter is painting with,
    /// which is why this exists alongside `brush_graph_export_yaml` (the
    /// *active* graph) rather than callers loading each brush in turn.
    #[handler]
    pub fn brush_export_yaml(&self, id: &str) -> Result<String, String> {
        let graph = library::with(|lib| {
            lib.get(id)
                .map(|b| b.metadata.graph.clone())
                .ok_or_else(|| format!("brush '{id}' not found"))
        })?;
        let portable = crate::brush::portable::PortableBrush::from_graph_only(
            &graph,
            crate::brush::registry(),
        )?;
        serde_yaml_ng::to_string(&portable).map_err(|e| format!("YAML serialize error: {e}"))
    }

    /// Rename a brush. Touches no pack and no recents entry — both hold ids.
    #[handler]
    pub fn brush_rename(&mut self, id: &str, name: &str) -> Result<(), String> {
        library::with_mut(|lib| lib.rename(id, name))
    }

    /// Delete a brush, removing it from every pack that held it.
    #[handler]
    pub fn brush_delete(&mut self, id: &str) -> Result<(), String> {
        library::with_mut(|lib| {
            lib.delete_brush(id)
                .then_some(())
                .ok_or_else(|| format!("brush '{id}' not found"))
        })
    }

    /// Create a brush pack under a caller-supplied id.
    #[handler]
    pub fn pack_create(
        &mut self,
        id: &str,
        name: &str,
        description: &str,
        icon: &str,
        primary: &str,
        secondary: &str,
    ) -> Result<(), String> {
        library::with_mut(|lib| lib.create_pack(id, name, description, icon, primary, secondary))
    }

    /// Change a pack's name, description, icon or colors.
    #[handler]
    pub fn pack_edit(
        &mut self,
        id: &str,
        name: &str,
        description: &str,
        icon: &str,
        primary: &str,
        secondary: &str,
    ) -> Result<(), String> {
        library::with_mut(|lib| lib.edit_pack(id, name, description, icon, primary, secondary))
    }

    /// Delete a pack. Its brushes survive.
    #[handler]
    pub fn pack_delete(&mut self, id: &str) -> Result<(), String> {
        library::with_mut(|lib| lib.delete_pack(id))
    }

    /// Copy a brush into a pack. It does not leave any pack it is already in.
    #[handler]
    pub fn pack_add_brush(&mut self, pack: &str, brush: &str) -> Result<(), String> {
        library::with_mut(|lib| lib.add_to_pack(pack, brush))
    }

    #[handler]
    pub fn pack_remove_brush(&mut self, pack: &str, brush: &str) -> Result<(), String> {
        library::with_mut(|lib| lib.remove_from_pack(pack, brush))
    }

    #[handler]
    pub fn pack_reorder_brush(
        &mut self,
        pack: &str,
        brush: &str,
        index: u32,
    ) -> Result<(), String> {
        library::with_mut(|lib| lib.reorder_in_pack(pack, brush, index as usize))
    }

    /// Import a `.darkly-brush` archive as a new pack under `id`.
    #[handler]
    pub fn pack_import(&mut self, id: &str, bytes: &[u8]) -> Result<String, String> {
        library::with_mut(|lib| lib.import_pack(id, bytes))
    }

    /// Export a pack as `.darkly-brush` bytes.
    pub fn pack_export(&self, id: &str) -> Result<Vec<u8>, String> {
        library::with(|lib| lib.export_pack(id))
    }

    /// Return the cached PNG thumbnail bytes for a library brush, kicking
    /// off an async bake if none exists yet. Returns an empty vector when
    /// the bake is in flight (or the brush is missing); the frontend polls
    /// on rAF until non-empty bytes arrive. Subsequent calls hit the cache.
    #[handler(returns = bytes)]
    pub fn brush_thumbnail(&mut self, name: &str) -> Vec<u8> {
        // Resolve and copy out under one short borrow: the bake below takes
        // `&mut self`, so nothing may still be borrowing the library.
        let resolved = library::with(|lib| {
            lib.by_name(name).map(|b| {
                (
                    b.id().to_string(),
                    b.thumbnail_png.clone(),
                    b.metadata.graph.clone(),
                )
            })
        });
        let Some((id, cached, graph)) = resolved else {
            return Vec::new();
        };
        if let Some(png) = cached {
            return png;
        }
        // A bake for this brush is already pending — don't queue another;
        // racing readbacks would step on each other's library entry.
        let already_pending = self
            .readbacks
            .any(|c| matches!(c, ReadbackContext::BrushThumbnailForSave { id: i, .. } if *i == id));
        if already_pending {
            return Vec::new();
        }
        self.request_stroke_preview_readback(graph, |width, height, backdrop| {
            ReadbackContext::BrushThumbnailForSave {
                id: id.clone(),
                width,
                height,
                backdrop,
            }
        });
        Vec::new()
    }

    /// Return the cached dab thumbnail PNG bytes for a library brush,
    /// kicking off an async bake if none exists yet. Same shape as
    /// `brush_thumbnail` but renders a single full-pressure dab instead
    /// of an S-curve, giving the picker a tip silhouette to show next
    /// to the stroke preview.
    #[handler(returns = bytes)]
    pub fn brush_dab_thumbnail(&mut self, name: &str) -> Vec<u8> {
        let resolved = library::with(|lib| {
            lib.by_name(name).map(|b| {
                let id = b.id().to_string();
                let cached = lib.dab_thumbnail_png(&id).map(<[u8]>::to_vec);
                (id, cached, b.metadata.graph.clone())
            })
        });
        let Some((id, cached, graph)) = resolved else {
            return Vec::new();
        };
        if let Some(png) = cached {
            return png;
        }
        let already_pending = self
            .readbacks
            .any(|c| matches!(c, ReadbackContext::BrushDabThumbnail { id: i, .. } if *i == id));
        if already_pending {
            return Vec::new();
        }
        // The shared helper resets every exposed scrub (size, opacity,
        // hardness, …) to its registration default before rendering — same
        // treatment the active-dab preview applies. Keeping the two paths on
        // one helper means `brush_dab_thumbnail(active_name)` and
        // `brush_active_dab_preview()` produce byte-identical PNGs, so the
        // picker tile and the BrushBar trigger always agree.
        self.request_dab_preview_readback(graph, |width, height| {
            ReadbackContext::BrushDabThumbnail {
                id: id.clone(),
                width,
                height,
            }
        });
        Vec::new()
    }
}
