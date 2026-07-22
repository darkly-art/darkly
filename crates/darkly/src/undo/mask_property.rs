use super::UndoAction;
use crate::document::{Document, FilterKind};
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// Undo action for relationship properties owned by a mask filter entity.
pub struct MaskLinkedToHostAction {
    mask_id: LayerId,
    saved: bool,
}

impl MaskLinkedToHostAction {
    pub fn new(mask_id: LayerId, saved: bool) -> Self {
        Self { mask_id, saved }
    }

    fn swap(&mut self, doc: &mut Document) {
        let Some(filter) = doc.find_filter_mut(self.mask_id) else {
            return;
        };
        let FilterKind::Mask(mask) = &mut filter.kind else {
            return;
        };
        std::mem::swap(&mut mask.linked_to_host, &mut self.saved);
    }
}

impl UndoAction for MaskLinkedToHostAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }
}
