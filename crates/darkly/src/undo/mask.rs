use super::UndoAction;
use crate::document::Document;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// Undo action for mask structural changes (add/remove mask, toggle mask_enabled/show_mask).
///
/// Stores only boolean flags — mask pixel data is GPU-authoritative and
/// preserved via `RegionStore` + `GpuRegionAction` (wrapped in a `CompoundAction`
/// alongside this action when removing a mask).
pub struct MaskPropertyAction {
    layer_id: LayerId,
    had_mask: bool,
    mask_enabled: bool,
    show_mask: bool,
}

impl MaskPropertyAction {
    pub fn new(layer_id: LayerId, had_mask: bool, mask_enabled: bool, show_mask: bool) -> Self {
        MaskPropertyAction {
            layer_id,
            had_mask,
            mask_enabled,
            show_mask,
        }
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
            let mut has = m.has_mask();
            std::mem::swap(&mut has, &mut self.had_mask);
            m.set_has_mask(has);
            let mut enabled = m.mask_enabled();
            std::mem::swap(&mut enabled, &mut self.mask_enabled);
            m.set_mask_enabled(enabled);
            let mut show = m.show_mask();
            std::mem::swap(&mut show, &mut self.show_mask);
            m.set_show_mask(show);
        }
    }
}
