use super::UndoAction;
use crate::document::Document;
use crate::layer::LayerId;
use crate::tile::AlphaMask;
use std::collections::{HashMap, HashSet};

/// Undo action for mask structural changes (add/remove mask, toggle mask_enabled/show_mask).
/// Swaps the full mask state on undo/redo. Works for both raster layers and groups.
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
        if let Some(node) = doc.find_node_mut(self.layer_id) {
            let m = node.as_masked_mut();
            std::mem::swap(m.mask_mut(), &mut self.mask);
            let mut enabled = m.mask_enabled();
            std::mem::swap(&mut enabled, &mut self.mask_enabled);
            m.set_mask_enabled(enabled);
            let mut show = m.show_mask();
            std::mem::swap(&mut show, &mut self.show_mask);
            m.set_show_mask(show);

            // Update mask_dirty tracking
            if m.mask().is_some() {
                doc.mask_dirty.entry(self.layer_id).or_default();
            } else {
                doc.mask_dirty.remove(&self.layer_id);
            }
        }
    }
}
