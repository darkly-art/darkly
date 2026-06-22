//! Canvas flip & 90°-rotate.
//!
//! Like [`image_rescale`](super::image_rescale), this permutes **every
//! pixel-bearing node** (raster layers + mask filters) — but exactly, via the
//! ortho GPU pass rather than a resample. Each node moves to its transformed
//! extent about the canvas window; rotations additionally swap the canvas
//! dimensions and recentre `canvas_origin` (GIMP's `offset = (old − new)/2`).
//! It is undoable through the shared [`CanvasGeometryAction`]: each node's
//! pre-transform pixels are snapshotted, and the dims/origin + per-node extents
//! ride one action.
//!
//! **Selection.** Flips and 180° rotation preserve the canvas dimensions, so
//! the selection mask is transformed alongside the content (matching Krita /
//! GIMP) with full undo. Rotate-90 swaps the dimensions, which the selection
//! undo-restore path does not round-trip; like image rescale it clears the
//! selection instead, folded into the same undo step. Carrying a
//! dimension-swapping selection through is a follow-up.

use super::DarklyEngine;
use crate::coord::{CanvasPoint, CanvasRect};
use crate::gpu::ortho_transform::OrthoXform;
use crate::gpu::region_store::UndoRegionEntry;
use crate::layer::LayerId;
use crate::undo::CanvasGeometryAction;

impl DarklyEngine {
    /// Flip or rotate the whole canvas by an orthogonal transform. Permutes
    /// every layer + mask exactly, transforms or clears the selection (see the
    /// module docs), swaps canvas dims/origin for rotations, and records one
    /// undoable [`CanvasGeometryAction`].
    pub fn transform_canvas(&mut self, xform: OrthoXform) {
        self.auto_commit_floating();

        let frame = self.doc.canvas_rect();
        let (old_w, old_h) = (self.doc.width, self.doc.height);
        let old_origin = self.doc.canvas_origin;

        // Enumerate pixel-bearing nodes that own a texture in the unified pool
        // (raster layers + mask filters; voids regenerate, selection lives in
        // selection_state). Identical capability scan to image rescale.
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
        let mut nodes: Vec<(LayerId, CanvasRect, wgpu::TextureFormat)> = Vec::new();
        for id in candidate_ids {
            if let Some(t) = self.compositor.node_texture(id) {
                nodes.push((id, t.canvas_extent(), t.format()));
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
                    "canvas-xform-save",
                    "canvas-xform-commit",
                )
                .expect("node texture present");
            regions.push(entry);
        }

        // 2. Selection: carry it through dimension-preserving transforms, clear
        //    it (folded) through dimension-swapping ones — see module docs.
        let selection = self.transform_or_clear_selection(xform);

        // 3. Permute all node textures on the GPU about the (old) canvas window.
        let ids: Vec<LayerId> = nodes.iter().map(|(id, _, _)| *id).collect();
        self.gpu.encode("canvas-xform-nodes", |enc| {
            self.compositor.ortho_transform_nodes(
                &self.gpu.device,
                &self.gpu.queue,
                enc,
                &ids,
                frame,
                xform,
            );
        });

        // 4. Mirror the permuted extents into the document.
        let mut bounds: Vec<(LayerId, CanvasRect, CanvasRect)> = Vec::with_capacity(nodes.len());
        for (id, old_extent, _) in &nodes {
            let new_extent = self
                .compositor
                .node_texture(*id)
                .map(|t| t.canvas_extent())
                .unwrap_or(*old_extent);
            self.doc.set_node_pixel_bounds(*id, new_extent);
            bounds.push((*id, *old_extent, new_extent));
        }

        // 5. Swap canvas dims + recentre origin for rotations (GIMP offset rule).
        let (new_w, new_h, new_origin) = if xform.swaps_dims() {
            let no = CanvasPoint::new(
                old_origin.x + (old_w as i32 - old_h as i32) / 2,
                old_origin.y + (old_h as i32 - old_w as i32) / 2,
            );
            (old_h, old_w, no)
        } else {
            (old_w, old_h, old_origin)
        };
        self.doc.width = new_w;
        self.doc.height = new_h;
        self.doc.canvas_origin = new_origin;

        self.push_undo(Box::new(CanvasGeometryAction::new(
            (old_w, old_h),
            (new_w, new_h),
            old_origin,
            new_origin,
            bounds,
            regions,
            selection,
        )));

        // 6. Reconcile window-sized GPU resources + present view to new dims.
        self.apply_canvas_rect_to_compositor();
        self.compositor.mark_dirty();
    }

    /// Selection bookkeeping for a canvas transform. Returns the folded
    /// `(was_active, entry)` for the undo action, or `None` if there was no
    /// selection. Dimension-preserving transforms permute the mask in place;
    /// dimension-swapping ones clear it (see module docs).
    fn transform_or_clear_selection(
        &mut self,
        xform: OrthoXform,
    ) -> Option<(bool, UndoRegionEntry)> {
        if !self.has_selection() {
            return None;
        }
        if xform.swaps_dims() {
            return self.clear_selection_collecting_undo();
        }

        // Carry the selection through: snapshot the full window, permute the
        // mask, commit the snapshot as the undo entry.
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);
        let was_active = true;
        self.gpu.encode("canvas-xform-sel", |enc| {
            self.compositor.ortho_transform_selection(
                &self.gpu.device,
                &self.gpu.queue,
                enc,
                xform,
            );
        });
        let entry = self.commit_selection_undo_entry(rect)?;
        // The permuted mask invalidates the window-local bounds + cpu cache and
        // the marching ants; a readback repopulates them (and the ants).
        self.set_selection_pixel_bounds(None);
        self.invalidate_selection_cpu_cache();
        self.kick_selection_readback();
        Some((was_active, entry))
    }
}
