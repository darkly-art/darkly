//! Undo actions for filter-node mutations.
//!
//! Replaces the old `MaskPropertyAction` with generic node-add / node-remove
//! actions that work for any [`Filter`] kind. Per the Modularity Principle,
//! adding a new filter kind doesn't require new undo actions — these are
//! kind-uniform.
//!
//! Pixel data for pixel-bearing filters (today: masks) is preserved by
//! wrapping a `GpuRegionAction` alongside the [`FilterRemoveAction`] in a
//! [`CompoundAction`] at the call site (see `engine/filters/mask.rs`).
//!
//! Detach/reattach uses the document's orphan-keep semantics: the filter
//! stays in the slotmap with its id intact between unlink and relink. Both
//! actions only need ids — no value handles travel through the undo stack.

use super::UndoAction;
use crate::document::Document;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// Undo action for adding a filter to a host.
///
/// Undo unlinks the filter from its host (it stays in the document's
/// slotmap orphaned).
/// Redo relinks it on the same host.
pub struct FilterAddAction {
    filter_id: LayerId,
    host_id: LayerId,
}

impl FilterAddAction {
    pub fn new(filter_id: LayerId, host_id: LayerId) -> Self {
        FilterAddAction { filter_id, host_id }
    }
}

impl UndoAction for FilterAddAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.detach_filter_for_undo(self.filter_id);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.reinsert_filter(self.filter_id, self.host_id);
        HashMap::new()
    }
}

/// Undo action for removing a filter from a host.
///
/// Undo relinks the orphaned filter to its original host.
/// Redo unlinks it again.
pub struct FilterRemoveAction {
    filter_id: LayerId,
    host_id: LayerId,
}

impl FilterRemoveAction {
    pub fn new(filter_id: LayerId, host_id: LayerId) -> Self {
        FilterRemoveAction { filter_id, host_id }
    }
}

impl UndoAction for FilterRemoveAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.reinsert_filter(self.filter_id, self.host_id);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.detach_filter_for_undo(self.filter_id);
        HashMap::new()
    }
}

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
