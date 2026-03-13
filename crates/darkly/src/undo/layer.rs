use super::UndoAction;
use crate::document::Document;
use crate::layer::{LayerId, LayerNode};
use std::collections::{HashMap, HashSet};

/// Undo action for adding a layer/group.
///
/// Undo removes it from the tree (storing the detached node).
/// Redo reinserts it at the original position.
pub struct LayerAddAction {
    layer_id: LayerId,
    parent: Option<LayerId>,
    position: usize,
    /// Holds the detached node between undo and redo.
    detached: Option<LayerNode>,
}

impl LayerAddAction {
    pub fn new(layer_id: LayerId, parent: Option<LayerId>, position: usize) -> Self {
        LayerAddAction {
            layer_id,
            parent,
            position,
            detached: None,
        }
    }
}

impl UndoAction for LayerAddAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // Remove the added layer.
        self.detached = doc.detach_for_undo(self.layer_id);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // Reinsert the layer.
        if let Some(node) = self.detached.take() {
            doc.reinsert_node(node, self.parent, self.position);
        }
        HashMap::new()
    }


}

/// Undo action for removing a layer/group.
///
/// Undo reinserts the removed node at its original position.
/// Redo removes it again.
pub struct LayerRemoveAction {
    parent: Option<LayerId>,
    position: usize,
    /// Holds the removed node. Present after construction and after redo.
    /// Absent after undo (node is back in the tree).
    detached: Option<LayerNode>,
}

impl LayerRemoveAction {
    pub fn new(node: LayerNode, parent: Option<LayerId>, position: usize) -> Self {
        LayerRemoveAction {
            parent,
            position,
            detached: Some(node),
        }
    }
}

impl UndoAction for LayerRemoveAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // Reinsert the removed node.
        if let Some(node) = self.detached.take() {
            doc.reinsert_node(node, self.parent, self.position);
        }
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // Remove it again. We need the layer_id from the detached node.
        // The node was reinserted by undo, so find it from parent+position.
        let container = match self.parent {
            Some(pid) => {
                if let Some(node) = doc.find_node(pid) {
                    if let LayerNode::Group(g) = node {
                        g.children.get(self.position).map(|n| n.id())
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            None => doc.root.children.get(self.position).map(|n| n.id()),
        };

        if let Some(layer_id) = container {
            self.detached = doc.detach_for_undo(layer_id);
        }
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
        // Move back to old position.
        if let Some(node) = doc.detach_for_undo(self.layer_id) {
            doc.reinsert_node(node, self.old_parent, self.old_position);
        }
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // Move to new position.
        if let Some(node) = doc.detach_for_undo(self.layer_id) {
            doc.reinsert_node(node, self.new_parent, self.new_position);
        }
        HashMap::new()
    }


}
