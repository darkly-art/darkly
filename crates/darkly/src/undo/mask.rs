use super::UndoAction;
use crate::document::Document;
use crate::layer::{Layer, LayerId};
use crate::tile::AlphaMask;
use std::collections::{HashMap, HashSet};

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
