use super::UndoAction;
use crate::document::Document;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// Undo action for adding a layer/group.
///
/// Undo unlinks the node from the tree (it stays in the document's slotmap
/// orphaned, so the id is preserved).
/// Redo reinserts it at the original position.
pub struct LayerAddAction {
    layer_id: LayerId,
    parent: Option<LayerId>,
    position: usize,
}

impl LayerAddAction {
    pub fn new(layer_id: LayerId, parent: Option<LayerId>, position: usize) -> Self {
        LayerAddAction {
            layer_id,
            parent,
            position,
        }
    }
}

impl UndoAction for LayerAddAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.detach_for_undo(self.layer_id);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.reinsert_node(self.layer_id, self.parent, self.position);
        HashMap::new()
    }
}

/// Undo action for removing a layer/group.
///
/// The node stays in the document's slotmap as an orphan between detach
/// and reattach — the id (and all attached modifiers/descendants) survives
/// across undo/redo with no copy. Undo relinks it; redo unlinks again.
pub struct LayerRemoveAction {
    layer_id: LayerId,
    parent: Option<LayerId>,
    position: usize,
}

impl LayerRemoveAction {
    pub fn new(layer_id: LayerId, parent: Option<LayerId>, position: usize) -> Self {
        LayerRemoveAction {
            layer_id,
            parent,
            position,
        }
    }
}

impl UndoAction for LayerRemoveAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.reinsert_node(self.layer_id, self.parent, self.position);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.detach_for_undo(self.layer_id);
        HashMap::new()
    }
}

/// Undo action for moving a layer/group to a new position.
///
/// Stores the old and new positions. Undo moves back to old, redo moves to new.
pub struct LayerMoveAction {
    layer_id: LayerId,
    old_parent: Option<LayerId>,
    old_position: usize,
    new_parent: Option<LayerId>,
    new_position: usize,
}

impl LayerMoveAction {
    pub fn new(
        layer_id: LayerId,
        old_parent: Option<LayerId>,
        old_position: usize,
        new_parent: Option<LayerId>,
        new_position: usize,
    ) -> Self {
        LayerMoveAction {
            layer_id,
            old_parent,
            old_position,
            new_parent,
            new_position,
        }
    }
}

impl UndoAction for LayerMoveAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        if doc.detach_for_undo(self.layer_id).is_some() {
            doc.reinsert_node(self.layer_id, self.old_parent, self.old_position);
        }
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        if doc.detach_for_undo(self.layer_id).is_some() {
            doc.reinsert_node(self.layer_id, self.new_parent, self.new_position);
        }
        HashMap::new()
    }
}
