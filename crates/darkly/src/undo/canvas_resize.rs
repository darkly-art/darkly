use super::UndoAction;
use crate::coord::CanvasPoint;
use crate::document::Document;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// Undo action for a canvas-window move/resize that does **not** rescale
/// content (translate / crop / crop-to-selection).
///
/// Document-only and exact: it swaps `Document::canvas_origin` plus
/// `width`/`height`. Content, layer extents, and the rest of the undo stack
/// stay put in the plane, so nothing pixel-level needs restoring. The GPU's
/// window-sized resources are reconciled by the engine's `apply_undo` hook,
/// which calls `Compositor::set_canvas_rect` whenever the document's canvas
/// rect diverges from the compositor's after an undo/redo.
///
/// Content-scaling resize is lossy; it rides a [`super::CompoundAction`] of
/// per-layer `GpuRegionAction` pixel snapshots wrapped around one of these
/// (for the dimensions), so the pixel restores and the dims swap undo together.
pub struct CanvasResizeAction {
    old_origin: CanvasPoint,
    old_width: u32,
    old_height: u32,
    new_origin: CanvasPoint,
    new_width: u32,
    new_height: u32,
}

impl CanvasResizeAction {
    /// `(origin, width, height)` of the canvas window before and after the op.
    pub fn new(old: (CanvasPoint, u32, u32), new: (CanvasPoint, u32, u32)) -> Self {
        CanvasResizeAction {
            old_origin: old.0,
            old_width: old.1,
            old_height: old.2,
            new_origin: new.0,
            new_width: new.1,
            new_height: new.2,
        }
    }

    fn apply(doc: &mut Document, origin: CanvasPoint, width: u32, height: u32) {
        doc.canvas_origin = origin;
        doc.width = width;
        doc.height = height;
    }
}

impl UndoAction for CanvasResizeAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        Self::apply(doc, self.old_origin, self.old_width, self.old_height);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        Self::apply(doc, self.new_origin, self.new_width, self.new_height);
        HashMap::new()
    }
}
