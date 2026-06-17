//! Undo action for canvas-geometry edits that resample or permute every
//! pixel-bearing node: **image rescale** (Photoshop "Image Size") and **canvas
//! flip / rotate**.
//!
//! Both swap the canvas dimensions (and, for rotate-90, the `canvas_origin`)
//! plus every node's `PixelBuffer.bounds`, and carry per-node GPU pixel
//! snapshots that the engine's `apply_undo` restores across the node texture
//! extent change (see `engine/rendering.rs`). They differ only in *how* the
//! GPU pixels were produced (bilinear resample vs. exact permutation) — which
//! is the compositor's concern, not the undo stack's — so one action serves
//! both. A folded selection entry lets an op with an active selection undo in
//! a single step.
//!
//! Why dedicated rather than reusing [`CanvasResizeAction`]: this one must
//! additionally swap per-node `PixelBuffer.bounds` (a canvas-window move never
//! touches layer extents) and the canvas dimensions independently of origin.
//!
//! [`CanvasResizeAction`]: super::CanvasResizeAction

use super::UndoAction;
use crate::coord::{CanvasPoint, CanvasRect};
use crate::document::Document;
use crate::gpu::compositor::Compositor;
use crate::gpu::region_store::UndoRegionEntry;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// The selection clear/transform folded into a geometry edit, so undo restores
/// the selection in the same step. Mirrors [`super::SelectionAction`]'s fields.
struct SelectionPart {
    was_active: bool,
    entry: UndoRegionEntry,
}

pub struct CanvasGeometryAction {
    old_w: u32,
    old_h: u32,
    new_w: u32,
    new_h: u32,
    /// Canvas window origin in plane space. Equal old/new for rescale and flips
    /// (which keep the window put); recentred by rotate-90 (GIMP offset rule).
    old_origin: CanvasPoint,
    new_origin: CanvasPoint,
    /// Per pixel-bearing node: `(id, old_extent, new_extent)`. The bounds swap
    /// drives `apply_undo`'s per-node texture-extent reconcile so the region
    /// restores land at the correct layer-local coords either way.
    bounds: Vec<(LayerId, CanvasRect, CanvasRect)>,
    /// Old-direction pixel snapshots, one per node (same order as `bounds`).
    regions: Vec<UndoRegionEntry>,
    /// Present only if a selection was active and cleared/transformed by the op.
    selection: Option<SelectionPart>,
}

impl CanvasGeometryAction {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        old_dims: (u32, u32),
        new_dims: (u32, u32),
        old_origin: CanvasPoint,
        new_origin: CanvasPoint,
        bounds: Vec<(LayerId, CanvasRect, CanvasRect)>,
        regions: Vec<UndoRegionEntry>,
        selection: Option<(bool, UndoRegionEntry)>,
    ) -> Self {
        CanvasGeometryAction {
            old_w: old_dims.0,
            old_h: old_dims.1,
            new_w: new_dims.0,
            new_h: new_dims.1,
            old_origin,
            new_origin,
            bounds,
            regions,
            selection: selection.map(|(was_active, entry)| SelectionPart { was_active, entry }),
        }
    }
}

impl UndoAction for CanvasGeometryAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.width = self.old_w;
        doc.height = self.old_h;
        doc.canvas_origin = self.old_origin;
        for (id, old_extent, _) in &self.bounds {
            doc.set_node_pixel_bounds(*id, *old_extent);
        }
        // GPU pixel + selection restores are handled by the engine via
        // `gpu_region_entries_mut` / `selection_region_entry_mut`.
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.width = self.new_w;
        doc.height = self.new_h;
        doc.canvas_origin = self.new_origin;
        for (id, _, new_extent) in &self.bounds {
            doc.set_node_pixel_bounds(*id, *new_extent);
        }
        HashMap::new()
    }

    fn gpu_region_entries_mut(&mut self) -> Vec<&mut UndoRegionEntry> {
        self.regions.iter_mut().collect()
    }

    fn selection_region_entry_mut(&mut self) -> Option<&mut UndoRegionEntry> {
        self.selection.as_mut().map(|s| &mut s.entry)
    }

    fn swap_selection_active(&mut self, current_active: bool) -> Option<bool> {
        self.selection.as_mut().map(|s| {
            let restore_to = s.was_active;
            s.was_active = current_active;
            restore_to
        })
    }

    fn byte_cost(&self) -> u64 {
        let regions: u64 = self
            .regions
            .iter()
            .map(|e| e.byte_size)
            .fold(0, u64::saturating_add);
        let sel = self
            .selection
            .as_ref()
            .map(|s| s.entry.byte_size)
            .unwrap_or(0);
        regions.saturating_add(sel)
    }

    fn on_evict(&mut self, _compositor: &mut Compositor) {
        // Storage is action-owned (per-entry buffers / heap), released when the
        // action drops. Override exists to document the contract — same as
        // `GpuRegionAction::on_evict`.
    }
}
