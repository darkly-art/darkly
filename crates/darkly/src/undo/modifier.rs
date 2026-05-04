//! Undo actions for modifier-node mutations.
//!
//! Replaces the old `MaskPropertyAction` with generic node-add / node-remove
//! actions that work for any [`Modifier`] kind. Per the Modularity Principle,
//! adding a new modifier kind doesn't require new undo actions — these are
//! kind-uniform.
//!
//! Pixel data for pixel-bearing modifiers (today: masks) is preserved by
//! wrapping a `GpuRegionAction` alongside the [`ModifierRemoveAction`] in a
//! [`CompoundAction`] at the call site (see `engine/modifiers/mask.rs`).
//!
//! Detach/reattach uses the document's orphan-keep semantics: the modifier
//! stays in the slotmap with its id intact between unlink and relink. Both
//! actions only need ids — no value handles travel through the undo stack.

use super::UndoAction;
use crate::document::Document;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// Undo action for adding a modifier to a host.
///
/// Undo unlinks the modifier from its host (it stays in the document's
/// slotmap orphaned).
/// Redo relinks it on the same host.
pub struct ModifierAddAction {
    modifier_id: LayerId,
    host_id: LayerId,
}

impl ModifierAddAction {
    pub fn new(modifier_id: LayerId, host_id: LayerId) -> Self {
        ModifierAddAction {
            modifier_id,
            host_id,
        }
    }
}

impl UndoAction for ModifierAddAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.detach_modifier_for_undo(self.modifier_id);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.reinsert_modifier(self.modifier_id, self.host_id);
        HashMap::new()
    }
}

/// Undo action for removing a modifier from a host.
///
/// Undo relinks the orphaned modifier to its original host.
/// Redo unlinks it again.
pub struct ModifierRemoveAction {
    modifier_id: LayerId,
    host_id: LayerId,
}

impl ModifierRemoveAction {
    pub fn new(modifier_id: LayerId, host_id: LayerId) -> Self {
        ModifierRemoveAction {
            modifier_id,
            host_id,
        }
    }
}

impl UndoAction for ModifierRemoveAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.reinsert_modifier(self.modifier_id, self.host_id);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.detach_modifier_for_undo(self.modifier_id);
        HashMap::new()
    }
}

/// Undo action for toggling visibility on any node — layer, group, or modifier.
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
        } else if let Some(modifier) = doc.find_modifier_mut(self.node_id) {
            std::mem::swap(&mut modifier.common.visible, &mut self.saved);
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

/// Undo action for toggling lock on any node — layer, group, or modifier.
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
        } else if let Some(modifier) = doc.find_modifier_mut(self.node_id) {
            std::mem::swap(&mut modifier.common.locked, &mut self.saved);
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
