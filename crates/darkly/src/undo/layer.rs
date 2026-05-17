use super::UndoAction;
use crate::document::Document;
use crate::gpu::compositor::Compositor;
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

/// Undo action for duplicating a layer or group (deep copy of subtree).
///
/// Undo detaches the duplicated root (the entire subtree orphans together
/// because `detach_for_undo` walks the tree). Redo reinserts it at its
/// original anchor. Eviction disposes the duplicated subtree's GPU
/// textures **only when the dup is currently detached** — i.e. the action
/// was sitting on the redo stack when it got evicted. If the dup is
/// attached at eviction time, its texture is part of live document state
/// and must not be touched.
pub struct DuplicateAction {
    root_new_id: LayerId,
    parent: Option<LayerId>,
    position: usize,
    /// Every pixel-bearing node id (raster + mask) inside the duplicated
    /// subtree. Used by [`UndoAction::on_evict`] to dispose GPU textures
    /// only when the dup is in the detached (undone) state.
    tombstones: Vec<LayerId>,
    /// True after construction / `redo`, false after `undo`. Tracks whether
    /// the duplicated subtree is currently in the document tree.
    applied: bool,
}

impl DuplicateAction {
    pub fn new(
        root_new_id: LayerId,
        parent: Option<LayerId>,
        position: usize,
        tombstones: Vec<LayerId>,
    ) -> Self {
        DuplicateAction {
            root_new_id,
            parent,
            position,
            tombstones,
            applied: true,
        }
    }
}

impl UndoAction for DuplicateAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.detach_for_undo(self.root_new_id);
        self.applied = false;
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.reinsert_node(self.root_new_id, self.parent, self.position);
        self.applied = true;
        HashMap::new()
    }

    fn on_evict(&mut self, compositor: &mut Compositor) {
        // Only dispose when the dup is currently detached — otherwise the
        // tombstones are part of the live document state.
        if !self.applied {
            for id in self.tombstones.drain(..) {
                compositor.dispose_layer(id);
            }
        }
    }
}

/// Slot a detached source node owned by a [`BakeLayersAction`].
#[derive(Clone, Copy, Debug)]
pub struct BakeSourceSlot {
    pub id: LayerId,
    pub parent: Option<LayerId>,
    pub position: usize,
}

/// Undo action for merge-down and flatten-image. Both ops consume a set of
/// source layers / groups and emit a single baked raster — same shape,
/// different selection rules.
///
/// The action holds the detach/reinsert metadata for every source plus the
/// position metadata for the baked result. The source GPU textures are
/// **tombstoned** in the compositor (not disposed) while the action is on
/// either stack, so undo restores pixels for free. On redo the engine
/// re-runs `bake_subtree_to_layer` to recompose the result — cheaper than
/// snapshotting it.
pub struct BakeLayersAction {
    pub sources: Vec<BakeSourceSlot>,
    /// Pixel-bearing node ids inside the source subtrees — detached on
    /// the forward (applied) side, reattached on undo. Disposed at evict
    /// time **only if the action was applied** (sources currently detached).
    source_tombstones: Vec<LayerId>,

    pub result_id: LayerId,
    pub result_parent: Option<LayerId>,
    pub result_position: usize,
    /// The baked result's pixel-bearing node ids — typically just
    /// `[result_id]`. Disposed at evict time **only if the action was
    /// undone** (result currently detached).
    result_tombstones: Vec<LayerId>,

    /// True after construction / `redo`, false after `undo`. Determines
    /// which side is currently detached and therefore safe to dispose at
    /// eviction.
    applied: bool,
}

impl BakeLayersAction {
    pub fn new(
        sources: Vec<BakeSourceSlot>,
        source_tombstones: Vec<LayerId>,
        result_id: LayerId,
        result_parent: Option<LayerId>,
        result_position: usize,
        result_tombstones: Vec<LayerId>,
    ) -> Self {
        BakeLayersAction {
            sources,
            source_tombstones,
            result_id,
            result_parent,
            result_position,
            result_tombstones,
            applied: true,
        }
    }

    /// Source ids in bottom-to-top order — the order needed for compose.
    pub fn source_ids_bottom_to_top(&self) -> Vec<LayerId> {
        let mut ids: Vec<(usize, LayerId)> =
            self.sources.iter().map(|s| (s.position, s.id)).collect();
        ids.sort_by_key(|(p, _)| *p);
        ids.into_iter().map(|(_, id)| id).collect()
    }
}

impl UndoAction for BakeLayersAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // Detach the baked result; its texture lives on as a tombstone for
        // the next redo cycle (the result's texture is untouched between
        // undo and redo — nothing draws into detached textures — so no
        // recomposite is needed when redo brings it back).
        doc.detach_for_undo(self.result_id);

        // Reinsert sources in ascending position order — earlier slots first
        // so later positions remain valid as the tree grows back.
        let mut sources_sorted = self.sources.clone();
        sources_sorted.sort_by_key(|s| s.position);
        for slot in sources_sorted {
            doc.reinsert_node(slot.id, slot.parent, slot.position);
        }
        self.applied = false;
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // Detach sources first so their slots are gone before the result
        // claims its insertion position.
        for slot in &self.sources {
            doc.detach_for_undo(slot.id);
        }
        doc.reinsert_node(self.result_id, self.result_parent, self.result_position);
        self.applied = true;
        HashMap::new()
    }

    fn on_evict(&mut self, compositor: &mut Compositor) {
        // Dispose only the side currently detached — the opposite side is
        // live document state and must keep its textures.
        if self.applied {
            for id in self.source_tombstones.drain(..) {
                compositor.dispose_layer(id);
            }
        } else {
            for id in self.result_tombstones.drain(..) {
                compositor.dispose_layer(id);
            }
        }
    }
}
