//! Undo actions for per-node property flags shared by every entity kind —
//! layers, groups, and filters alike.
//!
//! Structural add/remove of a filter needs no action of its own: a filter is an
//! entity like any other, so [`EntityAddAction`] and [`EntityRemoveAction`]
//! cover it. Pixel data for pixel-bearing filters (today: masks) is preserved
//! by wrapping a `GpuRegionAction` alongside the removal in a `CompoundAction`
//! at the call site (see `engine/filters/mask.rs`).
//!
//! [`EntityAddAction`]: super::EntityAddAction
//! [`EntityRemoveAction`]: super::EntityRemoveAction

use super::UndoAction;
use crate::document::Document;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// Undo action for toggling visibility on any node — layer, group, or filter.
/// Stores the current value and swaps it on undo/redo.
pub struct NodeVisibleAction {
    node_id: LayerId,
    saved: bool,
}

impl NodeVisibleAction {
    pub fn new(node_id: LayerId, saved: bool) -> Self {
        NodeVisibleAction { node_id, saved }
    }

    fn swap(&mut self, doc: &mut Document) {
        if let Some(node) = doc.find_node_mut(self.node_id) {
            std::mem::swap(&mut node.common_mut().visible, &mut self.saved);
        } else if let Some(filter) = doc.find_filter_mut(self.node_id) {
            std::mem::swap(&mut filter.common.visible, &mut self.saved);
        }
    }
}

impl UndoAction for NodeVisibleAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }
}

/// Undo action for toggling lock on any node — layer, group, or filter.
pub struct NodeLockedAction {
    node_id: LayerId,
    saved: bool,
}

impl NodeLockedAction {
    pub fn new(node_id: LayerId, saved: bool) -> Self {
        NodeLockedAction { node_id, saved }
    }

    fn swap(&mut self, doc: &mut Document) {
        if let Some(node) = doc.find_node_mut(self.node_id) {
            std::mem::swap(&mut node.common_mut().locked, &mut self.saved);
        } else if let Some(filter) = doc.find_filter_mut(self.node_id) {
            std::mem::swap(&mut filter.common.locked, &mut self.saved);
        }
    }
}

impl UndoAction for NodeLockedAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }
}
