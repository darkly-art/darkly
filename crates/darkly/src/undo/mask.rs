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
            let c = node.common_mut();
            std::mem::swap(&mut c.has_mask, &mut self.had_mask);
            std::mem::swap(&mut c.mask_enabled, &mut self.mask_enabled);
            std::mem::swap(&mut c.show_mask, &mut self.show_mask);
        }
    }
}
