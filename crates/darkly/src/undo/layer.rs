use super::tombstones::Tombstones;
use super::UndoAction;
use crate::document::{Document, TreeSlot};
use crate::gpu::compositor::Compositor;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// Undo action for adding an entity — layer, group, or filter.
///
/// Undo unlinks the entity from its parent (it stays in the document's slotmap
/// orphaned, so the id is preserved).
/// Redo reinserts it under the same parent at the original position.
///
/// Kind-uniform: `Document`'s detach/reattach pair routes by the entity's own
/// kind, so attaching a mask to a host and a layer to a group are the same
/// operation from here. Adding a new entity kind needs no new action.
pub struct EntityAddAction {
    layer_id: LayerId,
    slot: TreeSlot,
}

impl EntityAddAction {
    pub fn new(layer_id: LayerId, slot: TreeSlot) -> Self {
        EntityAddAction { layer_id, slot }
    }
}

impl UndoAction for EntityAddAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.detach_for_undo(self.layer_id);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.reinsert_entity(self.layer_id, self.slot);
        HashMap::new()
    }
}

/// Undo action for removing an entity — layer, group, or filter.
///
/// The entity stays in the document's slotmap as an orphan between detach
/// and reattach — the id (and all attached filters/descendants) survives
/// across undo/redo with no copy. The subtree's GPU textures are
/// tombstoned so the pixels survive too; they're disposed only when this
/// action is evicted from the undo stack while the deletion is still in
/// effect (i.e. the user never undid it). Undo relinks the entity; redo
/// unlinks again.
///
/// Callers that manage a filter's pixels themselves — `remove_mask` saves the
/// mask texture into a `GpuRegionAction` and disposes it eagerly — pass an
/// empty tombstone set.
pub struct EntityRemoveAction {
    layer_id: LayerId,
    slot: TreeSlot,
    /// Pixel-bearing node ids inside the removed subtree. Detached on the
    /// applied side; disposed by [`UndoAction::on_evict`] only when the
    /// action is evicted while still applied.
    tombstones: Tombstones,
    /// True after construction / `redo`, false after `undo`. Tracks
    /// whether the removed subtree is currently detached from the tree.
    applied: bool,
}

impl EntityRemoveAction {
    pub fn new(layer_id: LayerId, slot: TreeSlot, tombstones: Vec<LayerId>) -> Self {
        EntityRemoveAction {
            layer_id,
            slot,
            tombstones: Tombstones::new(tombstones, /* detached_when_applied: */ true),
            applied: true,
        }
    }
}

impl UndoAction for EntityRemoveAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.reinsert_entity(self.layer_id, self.slot);
        self.applied = false;
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.detach_for_undo(self.layer_id);
        self.applied = true;
        HashMap::new()
    }

    fn on_evict(&mut self, compositor: &mut Compositor) {
        self.tombstones
            .dispose_if_detached(self.applied, compositor);
    }
}

/// Undo action for moving a layer/group to a new position.
///
/// Stores the old and new positions. Undo moves back to old, redo moves to new.
pub struct LayerMoveAction {
    layer_id: LayerId,
    old: TreeSlot,
    new: TreeSlot,
}

impl LayerMoveAction {
    pub fn new(layer_id: LayerId, old: TreeSlot, new: TreeSlot) -> Self {
        LayerMoveAction { layer_id, old, new }
    }
}

impl UndoAction for LayerMoveAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        if doc.detach_for_undo(self.layer_id).is_some() {
            doc.reinsert_entity(self.layer_id, self.old);
        }
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        if doc.detach_for_undo(self.layer_id).is_some() {
            doc.reinsert_entity(self.layer_id, self.new);
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
    slot: TreeSlot,
    /// Every pixel-bearing node id (raster + mask) inside the duplicated
    /// subtree. Disposed by [`UndoAction::on_evict`] only when the dup is
    /// in the detached (undone) state.
    tombstones: Tombstones,
    /// True after construction / `redo`, false after `undo`. Tracks whether
    /// the duplicated subtree is currently in the document tree.
    applied: bool,
}

impl DuplicateAction {
    pub fn new(root_new_id: LayerId, slot: TreeSlot, tombstones: Vec<LayerId>) -> Self {
        DuplicateAction {
            root_new_id,
            slot,
            tombstones: Tombstones::new(tombstones, /* detached_when_applied: */ false),
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
        doc.reinsert_entity(self.root_new_id, self.slot);
        self.applied = true;
        HashMap::new()
    }

    fn on_evict(&mut self, compositor: &mut Compositor) {
        self.tombstones
            .dispose_if_detached(self.applied, compositor);
    }
}

/// Slot a detached source node owned by a [`BakeLayersAction`].
#[derive(Clone, Copy, Debug)]
pub struct BakeSourceSlot {
    pub id: LayerId,
    pub slot: TreeSlot,
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
    source_tombstones: Tombstones,

    pub result_id: LayerId,
    pub result_slot: TreeSlot,
    /// The baked result's pixel-bearing node ids — typically just
    /// `[result_id]`. Disposed at evict time **only if the action was
    /// undone** (result currently detached).
    result_tombstones: Tombstones,

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
        result_slot: TreeSlot,
        result_tombstones: Vec<LayerId>,
    ) -> Self {
        BakeLayersAction {
            sources,
            source_tombstones: Tombstones::new(
                source_tombstones,
                /* detached_when_applied: */ true,
            ),
            result_id,
            result_slot,
            result_tombstones: Tombstones::new(
                result_tombstones,
                /* detached_when_applied: */ false,
            ),
            applied: true,
        }
    }

    /// Source ids in bottom-to-top order — the order needed for compose.
    pub fn source_ids_bottom_to_top(&self) -> Vec<LayerId> {
        let mut ids: Vec<(usize, LayerId)> = self
            .sources
            .iter()
            .map(|s| (s.slot.position, s.id))
            .collect();
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
        sources_sorted.sort_by_key(|s| s.slot.position);
        for source in sources_sorted {
            doc.reinsert_entity(source.id, source.slot);
        }
        self.applied = false;
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // Detach sources first so their slots are gone before the result
        // claims its insertion position.
        for source in &self.sources {
            doc.detach_for_undo(source.id);
        }
        doc.reinsert_entity(self.result_id, self.result_slot);
        self.applied = true;
        HashMap::new()
    }

    fn on_evict(&mut self, compositor: &mut Compositor) {
        // Each tombstone set carries its own polarity; the helper disposes
        // the side currently detached and leaves the live side alone.
        self.source_tombstones
            .dispose_if_detached(self.applied, compositor);
        self.result_tombstones
            .dispose_if_detached(self.applied, compositor);
    }
}
