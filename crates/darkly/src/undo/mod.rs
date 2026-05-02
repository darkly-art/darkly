mod compound;
mod gpu_region;
mod layer;
mod mask;
pub mod property;
mod selection;

pub use compound::CompoundAction;
pub use gpu_region::GpuRegionAction;
pub use layer::{LayerAddAction, LayerMoveAction, LayerRemoveAction};
pub use mask::MaskPropertyAction;
pub use property::PropertyAction;
pub use selection::SelectionAction;

use crate::document::Document;
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
}

pub struct UndoStack {
    undo_steps: Vec<Box<dyn UndoAction>>,
    redo_steps: Vec<Box<dyn UndoAction>>,
    max_steps: usize,
}

impl UndoStack {
    pub fn new(max_steps: usize) -> Self {
        UndoStack {
            undo_steps: Vec::new(),
            redo_steps: Vec::new(),
            max_steps,
        }
    }

    /// Push a completed action. Clears redo history.
    pub fn push(&mut self, action: Box<dyn UndoAction>) {
        self.redo_steps.clear();
        self.undo_steps.push(action);

        if self.undo_steps.len() > self.max_steps {
            let remove = self.undo_steps.len() - self.max_steps;
            self.undo_steps.drain(0..remove);
        }
    }

    /// Try to coalesce a `PropertyAction` with the most recent undo step.
    /// If the top of the stack is a `PropertyAction` on the same layer and same
    /// property kind, update its `new_value` instead of pushing a new entry.
    /// This collapses rapid slider drags into a single undo step.
    pub fn coalesce_property(&mut self, action: PropertyAction) {
        if let Some(top) = self.undo_steps.last_mut() {
            if top.try_coalesce_property(&action) {
                return;
            }
        }
        self.push(Box::new(action));
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

        let id = doc.add_raster_layer();

        // Record the add as undoable.
        let parent = doc.parent_of(id);
        let pos = doc.position_in_parent(id).unwrap();
        undo.push(Box::new(LayerAddAction::new(id, parent, pos)));

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

        let id = doc.add_raster_layer();

        // Remove the layer (undoable).
        let parent = doc.parent_of(id);
        let pos = doc.position_in_parent(id).unwrap();
        let node = doc.detach_for_undo(id).unwrap();
        undo.push(Box::new(LayerRemoveAction::new(node, parent, pos)));

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

        let l1 = doc.add_raster_layer();
        let l2 = doc.add_raster_layer();
        let l3 = doc.add_raster_layer();

        // Order: l1, l2, l3 (bottom to top).
        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l1, l2, l3]);

        // Move l1 to the top (after l3).
        let old_parent = doc.parent_of(l1);
        let old_pos = doc.position_in_parent(l1).unwrap();
        doc.move_layer(l1, crate::document::MoveTarget::After(l3));
        let new_parent = doc.parent_of(l1);
        let new_pos = doc.position_in_parent(l1).unwrap();

        undo.push(Box::new(LayerMoveAction::new(
            l1, old_parent, old_pos, new_parent, new_pos,
        )));

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

        let id = doc.add_raster_layer();

        // Change opacity.
        undo.push(Box::new(PropertyAction::new(
            id,
            Property::Opacity(1.0),
            Property::Opacity(0.5),
        )));
        if let Some(Layer::Raster(r)) = doc.layer_mut(id) {
            r.common.opacity = 0.5;
        }

        // Undo — opacity back to 1.0.
        undo.undo(&mut doc);
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            assert!((r.common.opacity - 1.0).abs() < f32::EPSILON);
        }

        // Redo — opacity back to 0.5.
        undo.redo(&mut doc);
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            assert!((r.common.opacity - 0.5).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn coalesce_opacity_slider_drag() {
        use super::property::Property;

        let mut doc = Document::new(128, 128);
        let mut undo = UndoStack::new(100);
        let id = doc.add_raster_layer();

        // Simulate a slider drag: opacity goes 1.0 → 0.9 → 0.7 → 0.5 → 0.3
        // Each step captures old from the document, applies new, then coalesces.
        let steps = [0.9_f32, 0.7, 0.5, 0.3];
        for &new_val in &steps {
            let old_val = match doc.layer(id) {
                Some(Layer::Raster(r)) => r.common.opacity,
                _ => unreachable!(),
            };
            if let Some(Layer::Raster(r)) = doc.layer_mut(id) {
                r.common.opacity = new_val;
            }
            undo.coalesce_property(PropertyAction::new(
                id,
                Property::Opacity(old_val),
                Property::Opacity(new_val),
            ));
        }

        // Should be exactly 1 undo step, not 4.
        assert!(undo.can_undo());
        assert_eq!(
            doc.layer(id).map(|l| match l {
                Layer::Raster(r) => r.common.opacity,
            }),
            Some(0.3),
        );

        // Single undo should restore original opacity (1.0), not 0.5.
        undo.undo(&mut doc);
        let after_undo = match doc.layer(id) {
            Some(Layer::Raster(r)) => r.common.opacity,
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
            Some(Layer::Raster(r)) => r.common.opacity,
            _ => unreachable!(),
        };
        assert!(
            (after_redo - 0.3).abs() < f32::EPSILON,
            "redo should restore final opacity 0.3, got {after_redo}"
        );
    }
}
