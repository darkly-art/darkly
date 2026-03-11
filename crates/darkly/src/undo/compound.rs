use super::UndoAction;
use crate::document::Document;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// Groups multiple undo actions into a single step.
///
/// Actions are stored in forward (execution) order.
/// Undo iterates in reverse; redo iterates forward.
pub struct CompoundAction {
    actions: Vec<Box<dyn UndoAction>>,
}

impl CompoundAction {
    pub fn new(actions: Vec<Box<dyn UndoAction>>) -> Self {
        CompoundAction { actions }
    }
}

impl UndoAction for CompoundAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        let mut all = HashMap::new();
        for action in self.actions.iter_mut().rev() {
            for (id, coords) in action.undo(doc) {
                all.entry(id).or_insert_with(HashSet::new).extend(coords);
            }
        }
        all
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        let mut all = HashMap::new();
        for action in self.actions.iter_mut() {
            for (id, coords) in action.redo(doc) {
                all.entry(id).or_insert_with(HashSet::new).extend(coords);
            }
        }
        all
    }
}
