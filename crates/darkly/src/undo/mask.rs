use super::UndoAction;
use crate::document::Document;
use crate::layer::{Layer, LayerId};
use crate::tile::{AlphaF32, AlphaMask, Memento};
use std::collections::{HashMap, HashSet};

/// Undo action for mask tile (paint) operations.
/// Flip-flop: after undo(), internal memento holds forward state (for redo).
pub struct MaskTileAction {
    layer_id: LayerId,
    memento: Memento<AlphaF32>,
}

impl MaskTileAction {
    pub fn new(layer_id: LayerId, memento: Memento<AlphaF32>) -> Self {
        MaskTileAction { layer_id, memento }
    }
}

impl UndoAction for MaskTileAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        if let Some(Layer::Raster(r)) = doc.layer_mut(self.layer_id) {
            if let Some(mask) = &mut r.mask {
                let (forward, affected) = mask.rollback(&self.memento);
                self.memento = forward;
                // Populate mask_dirty so compositor re-uploads
                let dirty = doc.mask_dirty.entry(self.layer_id).or_default();
                for &(tx, ty) in &affected {
                    dirty.mark(tx, ty);
                }
            }
        }
        // Mask changes don't affect layer tiles, but we need to trigger recomposite.
        // Return empty — the engine handles mark_dirty() separately.
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        if let Some(Layer::Raster(r)) = doc.layer_mut(self.layer_id) {
            if let Some(mask) = &mut r.mask {
                let (backward, affected) = mask.rollforward(&self.memento);
                self.memento = backward;
                let dirty = doc.mask_dirty.entry(self.layer_id).or_default();
                for &(tx, ty) in &affected {
                    dirty.mark(tx, ty);
                }
            }
        }
        HashMap::new()
    }
}

/// Undo action for mask structural changes (add/remove mask, toggle mask_enabled/show_mask).
/// Swaps the full mask state on undo/redo.
pub struct MaskPropertyAction {
    layer_id: LayerId,
    mask: Option<AlphaMask>,
    mask_enabled: bool,
    show_mask: bool,
}

impl MaskPropertyAction {
    pub fn new(
        layer_id: LayerId,
        mask: Option<AlphaMask>,
        mask_enabled: bool,
        show_mask: bool,
    ) -> Self {
        MaskPropertyAction { layer_id, mask, mask_enabled, show_mask }
    }
}

impl UndoAction for MaskPropertyAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }
}

impl MaskPropertyAction {
    fn swap(&mut self, doc: &mut Document) {
        if let Some(Layer::Raster(r)) = doc.layer_mut(self.layer_id) {
            std::mem::swap(&mut r.mask, &mut self.mask);
            std::mem::swap(&mut r.mask_enabled, &mut self.mask_enabled);
            std::mem::swap(&mut r.show_mask, &mut self.show_mask);

            // Update mask_dirty tracking
            if r.mask.is_some() {
                doc.mask_dirty.entry(self.layer_id).or_default();
            } else {
                doc.mask_dirty.remove(&self.layer_id);
            }
        }
    }
}
