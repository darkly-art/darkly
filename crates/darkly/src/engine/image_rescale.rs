//! Image rescale (Photoshop "Image Size").
//!
//! Unlike [`canvas_resize`](super::canvas_resize), which only moves the canvas
//! window, this **resamples every pixel-bearing node** (raster layers + mask
//! filters) to new document dimensions, scaling content about the canvas
//! origin. It is lossy and undoable: each node's pre-rescale pixels are
//! snapshotted, and the dims + per-node extents ride one [`ImageRescaleAction`]
//! (which also folds in the selection clear). The GPU resampling lives in
//! `Compositor::rescale_nodes`; the undo restore-across-extent-change lives in
//! `apply_undo` (`engine/rendering.rs`).

use darkly_macros::handlers;

use super::canvas_resize::MAX_CANVAS_DIM;
use super::DarklyEngine;
use crate::coord::CanvasRect;
use crate::document::{MAX_DPI, MIN_DPI};
use crate::gpu::compositor::scaled_extent_about;
use crate::gpu::region_store::UndoRegionEntry;
use crate::layer::LayerId;
use crate::undo::CanvasGeometryAction;

#[handlers]
impl DarklyEngine {
    /// Resample all layer + mask pixels to `(new_width, new_height)`, scaling
    /// content about the canvas origin (which stays fixed), and install the
    /// document's DPI.
    ///
    /// The sole mutator of [`crate::document::Document::dpi`], because
    /// resampling and DPI are the same artist-facing question: how many
    /// pixels cover the artwork, and how big is the artwork. `None` keeps the
    /// physical size by riding the resample factor; `Some` installs an
    /// explicit value. Passing the current dimensions with a `Some` is
    /// therefore a DPI-only edit, which resamples nothing.
    ///
    /// Lossy + undoable (dims, origin, DPI, per-node extents and the
    /// selection clear all ride one [`CanvasGeometryAction`]). No-ops on a
    /// zero/over-limit target, on an out-of-band explicit DPI, on a change
    /// that moves nothing, or if any node would exceed the max texture
    /// dimension after scaling. A resample clears the active selection
    /// (folded into the same undo step); a DPI-only edit does not.
    #[handler]
    pub fn rescale_image(&mut self, new_width: u32, new_height: u32, dpi: Option<f32>) {
        let (old_w, old_h) = (self.doc.width, self.doc.height);
        if new_width == 0
            || new_height == 0
            || new_width > MAX_CANVAS_DIM
            || new_height > MAX_CANVAS_DIM
        {
            return;
        }
        let dims_changed = (new_width, new_height) != (old_w, old_h);
        let sx = new_width as f32 / old_w as f32;
        let sy = new_height as f32 / old_h as f32;
        let origin = self.doc.canvas_origin;
        let old_dpi = self.doc.dpi;
        let new_dpi = match dpi {
            // An explicit value is the artist's, so an out-of-band one is a
            // caller error and is rejected rather than clamped, the way both
            // reference editors treat their own setters.
            Some(d) => {
                if !d.is_finite() || !(MIN_DPI..=MAX_DPI).contains(&d) {
                    return;
                }
                d
            }
            // Resampling changes how many pixels cover the artwork, not how
            // big the artwork is, so the DPI rides the scale factor and the
            // document's physical extent is preserved: a 1920 px document at
            // 100 DPI taken to 7680 px is 7680 at 400 DPI, still 19.2 inches
            // wide, and every brush keeps its physical mark size on it
            // (204.8 canvas px where it was 51.2). The vertical factor is the
            // one taken for a non-uniform rescale, matching the aspect the
            // artist sees. Clamped rather than rejected: this value is
            // engine-computed, so an out-of-band result is a degenerate
            // target, not a caller error.
            None => (old_dpi * sy).clamp(MIN_DPI, MAX_DPI),
        };
        if !dims_changed && (new_dpi - old_dpi).abs() < 1e-5 {
            return;
        }

        // Everything from here to the dims install is resampling work. A
        // DPI-only edit touches no pixels, so it must not commit a floating
        // selection, snapshot regions, clear the selection or scan node
        // extents: it changes one number on the document.
        let (bounds, regions, selection) = if dims_changed {
            self.auto_commit_floating();

            // Enumerate pixel-bearing nodes by capability: raster layers +
            // mask filters. Void layers answer `pixels() == None` and are
            // skipped (they regenerate to fill the new canvas).
            let mut candidate_ids: Vec<LayerId> = Vec::new();
            for l in self.doc.all_content_layers() {
                if l.pixels().is_some() {
                    candidate_ids.push(l.id());
                }
            }
            for m in self.doc.all_filters() {
                if m.pixels().is_some() {
                    candidate_ids.push(m.id);
                }
            }
            // Keep only nodes that own a GPU texture in the unified pool. The
            // selection filter's pixels live in the compositor's
            // selection_state (window-sized), not node_textures; it's cleared
            // separately below.
            let mut nodes: Vec<(LayerId, CanvasRect, wgpu::TextureFormat)> = Vec::new();
            for id in candidate_ids {
                if let Some(t) = self.compositor.node_texture(id) {
                    nodes.push((id, t.canvas_extent(), t.format()));
                }
            }

            // Refuse if any node would exceed the max texture dimension once scaled.
            for (_, old_extent, _) in &nodes {
                let ne = scaled_extent_about(*old_extent, origin, sx, sy);
                if ne.width > MAX_CANVAS_DIM || ne.height > MAX_CANVAS_DIM {
                    return;
                }
            }

            // 1. Snapshot each node's current pixels (old direction) for undo.
            let mut regions: Vec<UndoRegionEntry> = Vec::with_capacity(nodes.len());
            for (id, old_extent, format) in &nodes {
                let entry = self
                    .snapshot_region_entry(
                        *id,
                        *old_extent,
                        *format,
                        "rescale-save",
                        "rescale-commit",
                    )
                    .expect("node texture present");
                regions.push(entry);
            }

            // 2. Clear the active selection while dims are still old (the
            //    selection texture is window-sized), folded into this single
            //    undo step.
            let selection = self.clear_selection_collecting_undo();

            // 3. Resample all node textures on the GPU.
            let ids: Vec<LayerId> = nodes.iter().map(|(id, _, _)| *id).collect();
            self.gpu.encode("rescale-nodes", |enc| {
                self.compositor
                    .rescale_nodes(&self.gpu.device, &self.gpu.queue, enc, &ids, sx, sy);
            });

            // 4. Mirror the resampled extents into the document + record bounds.
            let mut bounds: Vec<(LayerId, CanvasRect, CanvasRect)> =
                Vec::with_capacity(nodes.len());
            for (id, old_extent, _) in &nodes {
                let new_extent = self
                    .compositor
                    .node_texture(*id)
                    .map(|t| t.canvas_extent())
                    .unwrap_or(*old_extent);
                self.doc.set_node_pixel_bounds(*id, new_extent);
                bounds.push((*id, *old_extent, new_extent));
            }
            (bounds, regions, selection)
        } else {
            (Vec::new(), Vec::new(), None)
        };

        // 5. Update canvas dimensions (origin fixed) + push the undo step.
        self.doc.width = new_width;
        self.doc.height = new_height;
        self.doc.dpi = new_dpi;
        self.push_undo(Box::new(CanvasGeometryAction::new(
            (old_w, old_h),
            (new_width, new_height),
            origin,
            origin,
            old_dpi,
            new_dpi,
            bounds,
            regions,
            selection,
        )));

        // 6. Reconcile window-sized GPU resources + present view to new dims.
        //    A DPI-only edit moved no pixels and no window.
        if dims_changed {
            self.apply_canvas_rect_to_compositor();
            self.compositor.mark_dirty();
        }
    }
}
