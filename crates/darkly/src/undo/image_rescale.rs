//! Undo action for image rescale (Photoshop "Image Size").
//!
//! Image rescale is lossy: it resamples every pixel-bearing node to new
//! dimensions. This action carries the document-side swap (canvas width/height
//! plus each node's `PixelBuffer.bounds`) and the per-node GPU pixel snapshots
//! that the engine's `apply_undo` restores — across the node texture extent
//! change (see `engine/rendering.rs`). It also folds in the selection clear so
//! a rescale-with-active-selection undoes in a single step.
//!
//! Why a dedicated action rather than reusing [`CanvasResizeAction`]: this one
//! must additionally swap per-node `PixelBuffer.bounds` (canvas resize moves
//! only the window, never layer extents) and keep `canvas_origin` fixed while
//! changing width/height.
//!
//! [`CanvasResizeAction`]: super::CanvasResizeAction

use super::UndoAction;
use crate::coord::CanvasRect;
use crate::document::Document;
use crate::gpu::compositor::Compositor;
use crate::gpu::region_store::UndoRegionEntry;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// The selection clear folded into a rescale, so undo restores the selection
/// in the same step. Mirrors [`super::SelectionAction`]'s two fields.
struct SelectionPart {
    was_active: bool,
    entry: UndoRegionEntry,
}

pub struct ImageRescaleAction {
    old_w: u32,
    old_h: u32,
    new_w: u32,
    new_h: u32,
    /// Per pixel-bearing node: `(id, old_extent, new_extent)`. The bounds swap
    /// is what drives `apply_undo`'s per-node texture-extent reconcile so the
    /// region restores land at the correct layer-local coords either way.
    bounds: Vec<(LayerId, CanvasRect, CanvasRect)>,
    /// Old-direction pixel snapshots, one per node (same order as `bounds`).
    regions: Vec<UndoRegionEntry>,
    /// Present only if a selection was active and cleared by the rescale.
    selection: Option<SelectionPart>,
}

impl ImageRescaleAction {
    pub fn new(
        old_dims: (u32, u32),
        new_dims: (u32, u32),
        bounds: Vec<(LayerId, CanvasRect, CanvasRect)>,
        regions: Vec<UndoRegionEntry>,
        selection: Option<(bool, UndoRegionEntry)>,
    ) -> Self {
        ImageRescaleAction {
            old_w: old_dims.0,
            old_h: old_dims.1,
            new_w: new_dims.0,
            new_h: new_dims.1,
            bounds,
            regions,
            selection: selection.map(|(was_active, entry)| SelectionPart { was_active, entry }),
        }
    }
}

impl UndoAction for ImageRescaleAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.width = self.old_w;
        doc.height = self.old_h;
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
