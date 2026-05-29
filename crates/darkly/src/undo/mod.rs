mod compound;
mod gpu_region;
mod layer;
mod modifier;
pub mod property;
mod selection;
mod tombstones;

pub use compound::CompoundAction;
pub use gpu_region::GpuRegionAction;
pub use layer::{
    BakeLayersAction, BakeSourceSlot, DuplicateAction, LayerAddAction, LayerMoveAction,
    LayerRemoveAction,
};
pub use modifier::{ModifierAddAction, ModifierRemoveAction, NodeLockedAction, NodeVisibleAction};
pub use property::PropertyAction;
pub use selection::SelectionAction;

use crate::document::Document;
use crate::gpu::compositor::Compositor;
use crate::gpu::region_store::UndoRegionEntry;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// A reversible action that can be undone and redone.
///
/// Each action stores enough state to flip-flop: calling `undo()` transforms
/// the internal state so that a subsequent `redo()` reverses it, and vice versa.
/// The action is moved between the undo and redo stacks as a single `Box<dyn UndoAction>`.
pub trait UndoAction {
    /// Reverse this action. Returns affected tile coordinates per layer for GPU dirty marking.
    /// For non-tile actions (layer structure, properties), returns an empty map.
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>>;

    /// Re-apply this action. Returns affected tile coordinates per layer for GPU dirty marking.
    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>>;

    /// Try to coalesce a property change into this action.
    /// Only `PropertyAction` overrides this — all others return false.
    fn try_coalesce_property(&mut self, _other: &PropertyAction) -> bool {
        false
    }

    /// If this is a GPU region action, return a mutable reference to its entry.
    /// The engine uses this to execute GPU texture restores during undo/redo,
    /// then swaps the entry with the forward/backward entry returned by `restore_region`.
    fn gpu_region_entry_mut(&mut self) -> Option<&mut UndoRegionEntry> {
        None
    }

    /// If this is a selection GPU action, return a mutable reference to its region entry.
    /// The engine uses this to restore the selection GPU texture during undo/redo.
    fn selection_region_entry_mut(&mut self) -> Option<&mut UndoRegionEntry> {
        None
    }

    /// Swap the selection active flag for undo/redo. `current_active` is the
    /// engine's current `gpu_selection.active` state. Returns the flag value
    /// the engine should set after the swap (i.e. the state before this action).
    /// Returns `None` for non-selection actions.
    fn swap_selection_active(&mut self, _current_active: bool) -> Option<bool> {
        None
    }

    /// Called when this action is permanently dropped from both undo and redo
    /// stacks (max_steps overflow, byte-cap eviction, redo-history clear on a
    /// fresh push, or teardown). Owners of tombstoned GPU textures dispose
    /// them here so the compositor's `node_textures` pool never outlives its
    /// owning action.
    fn on_evict(&mut self, _compositor: &mut Compositor) {}

    /// Approximate memory cost of this action, used by [`UndoStack`]'s memory
    /// cap to evict oldest actions when the total exceeds the budget.
    ///
    /// Defaults to `0` — most actions (layer add/remove, property changes,
    /// modifier add/remove) hold only structural metadata. GPU region actions
    /// override this to return the pixel byte_size; compound actions sum
    /// children.
    fn byte_cost(&self) -> u64 {
        0
    }
}

/// Memory budget for undo entries. Tiered for WASM vs native because the
/// binding constraint in production is the 32-bit WASM linear-memory heap
/// (shared with layer pixel caches, thumbnails, document state, …), not the
/// host's physical DRAM.
///
/// - **WASM: 128 MB.** Defensive against the 4 GB linear-memory ceiling and
///   stricter per-tab budgets browsers impose. Still admits ~8 full-canvas
///   2048² commits before the cap kicks in.
/// - **Native: 512 MB.** Wider headroom for desktop dev / CI / integration
///   tests so synthetic full-canvas workloads don't fight the budget.
#[cfg(target_arch = "wasm32")]
const DEFAULT_MEMORY_CAP: u64 = 128 << 20;
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_MEMORY_CAP: u64 = 512 << 20;

pub struct UndoStack {
    undo_steps: Vec<Box<dyn UndoAction>>,
    redo_steps: Vec<Box<dyn UndoAction>>,
    max_steps: usize,
    /// Soft cap on `sum(byte_cost) across both stacks`. When exceeded after
    /// a push, oldest entries are evicted FIFO until the total fits.
    memory_cap: u64,
    /// Running sum of `byte_cost` over `undo_steps + redo_steps`. Maintained
    /// incrementally on push/eviction to avoid an O(n) recompute per push.
    total_bytes: u64,
}

impl UndoStack {
    pub fn new(max_steps: usize) -> Self {
        UndoStack {
            undo_steps: Vec::new(),
            redo_steps: Vec::new(),
            max_steps,
            memory_cap: DEFAULT_MEMORY_CAP,
            total_bytes: 0,
        }
    }

    /// Push a completed action. Clears redo history.
    ///
    /// Takes `&mut Document` so the chokepoint can set the sticky
    /// `Document::dirty` flag — every undoable mutation funnels through
    /// here, which makes this the only place dirty-tracking has to live.
    /// Undo/redo deliberately don't touch the flag (an undo back to the
    /// original state still leaves the doc "dirty" from the user's POV).
    ///
    /// Returns every action that leaves the stack as a result of this push:
    /// the entire previous redo history (cleared because a fresh action
    /// invalidates it) plus any actions evicted by the `max_steps` cap. The
    /// caller is responsible for invoking `on_evict` on each — typically
    /// through [`crate::engine::DarklyEngine::push_undo`], which threads the
    /// compositor in.
    #[must_use = "evicted actions must have on_evict called to release tombstones"]
    pub fn push(
        &mut self,
        doc: &mut Document,
        action: Box<dyn UndoAction>,
    ) -> Vec<Box<dyn UndoAction>> {
        doc.dirty = true;
        let mut evicted: Vec<Box<dyn UndoAction>> = self.redo_steps.drain(..).collect();
        for a in &evicted {
            self.total_bytes = self.total_bytes.saturating_sub(a.byte_cost());
        }
        self.total_bytes = self.total_bytes.saturating_add(action.byte_cost());
        self.undo_steps.push(action);

        // Step cap.
        if self.undo_steps.len() > self.max_steps {
            let remove = self.undo_steps.len() - self.max_steps;
            let drained: Vec<_> = self.undo_steps.drain(0..remove).collect();
            for a in &drained {
                self.total_bytes = self.total_bytes.saturating_sub(a.byte_cost());
            }
            evicted.extend(drained);
        }

        // Memory cap — drop oldest until the running total fits. Only
        // entries with non-zero byte_cost meaningfully shrink the total;
        // structural-only actions still leave the stack but contribute zero
        // to the budget, so the loop terminates after evicting whichever
        // GPU-region action ends up at the front.
        while self.total_bytes > self.memory_cap && !self.undo_steps.is_empty() {
            let a = self.undo_steps.remove(0);
            self.total_bytes = self.total_bytes.saturating_sub(a.byte_cost());
            evicted.push(a);
        }

        evicted
    }

    /// Try to coalesce a `PropertyAction` with the most recent undo step.
    /// If the top of the stack is a `PropertyAction` on the same layer and same
    /// property kind, update its `new_value` instead of pushing a new entry.
    /// This collapses rapid slider drags into a single undo step.
    ///
    /// Marks the document dirty whether the action coalesces or pushes
    /// fresh — coalescing means the user did something mid-drag, which
    /// is just as much "unsaved work" as a brand-new action.
    ///
    /// Returns evicted actions like [`Self::push`] — empty when the action
    /// coalesces into the existing top step (no stack change).
    #[must_use = "evicted actions must have on_evict called to release tombstones"]
    pub fn coalesce_property(
        &mut self,
        doc: &mut Document,
        action: PropertyAction,
    ) -> Vec<Box<dyn UndoAction>> {
        doc.dirty = true;
        if let Some(top) = self.undo_steps.last_mut() {
            if top.try_coalesce_property(&action) {
                return Vec::new();
            }
        }
        self.push(doc, Box::new(action))
    }

    /// Drain both stacks for teardown. Caller must run `on_evict` on every
    /// returned action so tombstoned textures release.
    #[must_use = "drained actions must have on_evict called to release tombstones"]
    pub fn drain_all(&mut self) -> Vec<Box<dyn UndoAction>> {
        let mut all: Vec<Box<dyn UndoAction>> = self.undo_steps.drain(..).collect();
        all.append(&mut self.redo_steps);
        self.total_bytes = 0;
        all
    }

    /// Undo the most recent action. Returns affected tile coords per layer.
    pub fn undo(&mut self, doc: &mut Document) -> Option<HashMap<LayerId, HashSet<(i32, i32)>>> {
        let mut action = self.undo_steps.pop()?;
        let affected = action.undo(doc);
        self.redo_steps.push(action);
        Some(affected)
    }

    /// Redo the most recently undone action. Returns affected tile coords per layer.
    pub fn redo(&mut self, doc: &mut Document) -> Option<HashMap<LayerId, HashSet<(i32, i32)>>> {
        let mut action = self.redo_steps.pop()?;
        let affected = action.redo(doc);
        self.undo_steps.push(action);
        Some(affected)
    }

    /// Pop the top undo action without executing it.
    /// The caller is responsible for calling `action.undo(doc)` and then
    /// `complete_undo(action)` to move it to the redo stack.
    pub fn pop_for_undo(&mut self) -> Option<Box<dyn UndoAction>> {
        self.undo_steps.pop()
    }

    /// Move an action to the redo stack after the caller has executed its undo.
    pub fn complete_undo(&mut self, action: Box<dyn UndoAction>) {
        self.redo_steps.push(action);
    }

    /// Pop the top redo action without executing it.
    pub fn pop_for_redo(&mut self) -> Option<Box<dyn UndoAction>> {
        self.redo_steps.pop()
    }

    /// Move an action to the undo stack after the caller has executed its redo.
    pub fn complete_redo(&mut self, action: Box<dyn UndoAction>) {
        self.undo_steps.push(action);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_steps.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_steps.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::Layer;

    #[test]
    fn undo_layer_add_remove() {
        let mut doc = Document::new(128, 128);
        let mut undo = UndoStack::new(100);

        let id = doc.add_raster_layer(None);

        // Record the add as undoable.
        let parent = doc.parent_of(id);
        let pos = doc.position_in_parent(id).unwrap();
        let _ = undo.push(&mut doc, Box::new(LayerAddAction::new(id, parent, pos)));

        assert_eq!(doc.flat_layers().len(), 1);

        // Undo the add — layer should be removed.
        undo.undo(&mut doc);
        assert_eq!(doc.flat_layers().len(), 0);

        // Redo — layer comes back.
        undo.redo(&mut doc);
        assert_eq!(doc.flat_layers().len(), 1);
    }

    #[test]
    fn undo_layer_remove() {
        let mut doc = Document::new(128, 128);
        let mut undo = UndoStack::new(100);

        let id = doc.add_raster_layer(None);

        // Remove the layer (undoable).
        let parent = doc.parent_of(id);
        let pos = doc.position_in_parent(id).unwrap();
        let node = doc.detach_for_undo(id).unwrap();
        let _ = undo.push(
            &mut doc,
            Box::new(LayerRemoveAction::new(node, parent, pos, Vec::new())),
        );

        assert_eq!(doc.flat_layers().len(), 0);

        // Undo the remove — layer should come back.
        undo.undo(&mut doc);
        assert_eq!(doc.flat_layers().len(), 1);

        // Redo the remove — layer gone again.
        undo.redo(&mut doc);
        assert_eq!(doc.flat_layers().len(), 0);
    }

    #[test]
    fn undo_layer_move() {
        let mut doc = Document::new(128, 128);
        let mut undo = UndoStack::new(100);

        let l1 = doc.add_raster_layer(None);
        let l2 = doc.add_raster_layer(None);
        let l3 = doc.add_raster_layer(None);

        // Order: l1, l2, l3 (bottom to top).
        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l1, l2, l3]);

        // Move l1 to the top (after l3).
        let old_parent = doc.parent_of(l1);
        let old_pos = doc.position_in_parent(l1).unwrap();
        doc.move_layer(l1, crate::document::MoveTarget::After(l3));
        let new_parent = doc.parent_of(l1);
        let new_pos = doc.position_in_parent(l1).unwrap();

        let _ = undo.push(
            &mut doc,
            Box::new(LayerMoveAction::new(
                l1, old_parent, old_pos, new_parent, new_pos,
            )),
        );

        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l2, l3, l1]);

        // Undo — back to original order.
        undo.undo(&mut doc);
        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l1, l2, l3]);

        // Redo — moved again.
        undo.redo(&mut doc);
        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l2, l3, l1]);
    }

    #[test]
    fn undo_property_change() {
        use super::property::Property;

        let mut doc = Document::new(128, 128);
        let mut undo = UndoStack::new(100);

        let id = doc.add_raster_layer(None);

        // Change opacity.
        let _ = undo.push(
            &mut doc,
            Box::new(PropertyAction::new(
                id,
                Property::Opacity(1.0),
                Property::Opacity(0.5),
            )),
        );
        if let Some(Layer::Raster(r)) = doc.layer_mut(id) {
            r.blend.opacity = 0.5;
        }

        // Undo — opacity back to 1.0.
        undo.undo(&mut doc);
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            assert!((r.blend.opacity - 1.0).abs() < f32::EPSILON);
        }

        // Redo — opacity back to 0.5.
        undo.redo(&mut doc);
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            assert!((r.blend.opacity - 0.5).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn coalesce_opacity_slider_drag() {
        use super::property::Property;

        let mut doc = Document::new(128, 128);
        let mut undo = UndoStack::new(100);
        let id = doc.add_raster_layer(None);

        // Simulate a slider drag: opacity goes 1.0 → 0.9 → 0.7 → 0.5 → 0.3
        // Each step captures old from the document, applies new, then coalesces.
        let steps = [0.9_f32, 0.7, 0.5, 0.3];
        for &new_val in &steps {
            let old_val = match doc.layer(id) {
                Some(Layer::Raster(r)) => r.blend.opacity,
                _ => unreachable!(),
            };
            if let Some(Layer::Raster(r)) = doc.layer_mut(id) {
                r.blend.opacity = new_val;
            }
            let _ = undo.coalesce_property(
                &mut doc,
                PropertyAction::new(id, Property::Opacity(old_val), Property::Opacity(new_val)),
            );
        }

        // Should be exactly 1 undo step, not 4.
        assert!(undo.can_undo());
        assert_eq!(doc.layer(id).map(|l| l.blend().opacity), Some(0.3),);

        // Single undo should restore original opacity (1.0), not 0.5.
        undo.undo(&mut doc);
        let after_undo = match doc.layer(id) {
            Some(Layer::Raster(r)) => r.blend.opacity,
            _ => unreachable!(),
        };
        assert!(
            (after_undo - 1.0).abs() < f32::EPSILON,
            "undo should restore original opacity 1.0, got {after_undo}"
        );

        // No more undo steps (the drag was one step).
        assert!(!undo.can_undo());

        // Redo should go back to 0.3.
        undo.redo(&mut doc);
        let after_redo = match doc.layer(id) {
            Some(Layer::Raster(r)) => r.blend.opacity,
            _ => unreachable!(),
        };
        assert!(
            (after_redo - 0.3).abs() < f32::EPSILON,
            "redo should restore final opacity 0.3, got {after_redo}"
        );
    }

    #[test]
    fn dirty_flag_set_by_undo_push() {
        // The single test that proves the chokepoint works — `push` is
        // the one place dirty-tracking lives, so a passing push must
        // flip the bit and every higher-level mutation is wired up by
        // construction.
        let mut doc = Document::new(64, 64);
        let mut undo = UndoStack::new(50);
        assert!(!doc.dirty, "fresh doc starts clean");

        let id = doc.add_raster_layer(None);
        let parent = doc.parent_of(id);
        let pos = doc.position_in_parent(id).unwrap();
        let _ = undo.push(&mut doc, Box::new(LayerAddAction::new(id, parent, pos)));
        assert!(doc.dirty, "push must flip dirty");
    }

    #[test]
    fn dirty_flag_set_by_coalesce_property() {
        // Slider drags go through `coalesce_property` rather than
        // `push`. The chokepoint flips dirty in both — a mid-drag
        // coalesce is just as much "unsaved work" as a fresh push.
        use super::property::Property;

        let mut doc = Document::new(64, 64);
        let mut undo = UndoStack::new(50);
        let id = doc.add_raster_layer(None);
        doc.dirty = false; // reset after the add_raster_layer setup

        let _ = undo.coalesce_property(
            &mut doc,
            PropertyAction::new(id, Property::Opacity(1.0), Property::Opacity(0.5)),
        );
        assert!(doc.dirty, "first coalesce push flips dirty");

        doc.dirty = false;
        let _ = undo.coalesce_property(
            &mut doc,
            PropertyAction::new(id, Property::Opacity(0.5), Property::Opacity(0.3)),
        );
        assert!(
            doc.dirty,
            "subsequent coalesce (merging into existing step) still flips dirty"
        );
    }

    #[test]
    fn dirty_flag_sticky_through_undo_redo() {
        // Sticky semantics: undoing back to the original state must
        // *not* clear dirty. The user worked, then undid; from a
        // "should this prompt before close?" POV the file is still
        // different from the last saved state.
        let mut doc = Document::new(64, 64);
        let mut undo = UndoStack::new(50);

        let id = doc.add_raster_layer(None);
        let parent = doc.parent_of(id);
        let pos = doc.position_in_parent(id).unwrap();
        let _ = undo.push(&mut doc, Box::new(LayerAddAction::new(id, parent, pos)));
        assert!(doc.dirty);

        undo.undo(&mut doc);
        assert!(
            doc.dirty,
            "undo back to original state must NOT clear dirty"
        );

        undo.redo(&mut doc);
        assert!(doc.dirty, "redo also leaves dirty set");
    }

    #[test]
    fn undo_add_raster_layer_with_anchor_restores_position() {
        // Adding a layer with an anchor lands it above the anchor; undo +
        // redo must replay it back to the same anchored slot, not to the top
        // of root.
        let mut doc = Document::new(128, 128);
        let mut undo = UndoStack::new(100);

        let l1 = doc.add_raster_layer(None);
        let l2 = doc.add_raster_layer(None);

        // "Add above l1" — should land between l1 and l2.
        let new_id = doc.add_raster_layer(Some(l1));
        let parent = doc.parent_of(new_id);
        let pos = doc.position_in_parent(new_id).unwrap();
        let _ = undo.push(&mut doc, Box::new(LayerAddAction::new(new_id, parent, pos)));

        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l1, new_id, l2]);

        // Undo removes the layer.
        undo.undo(&mut doc);
        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l1, l2]);

        // Redo restores the layer at its anchored position, not at the top.
        undo.redo(&mut doc);
        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l1, new_id, l2]);
    }
}
