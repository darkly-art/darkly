use crate::document::Document;
use crate::layer::LayerId;
use crate::tile::AlphaMask;
use std::collections::{HashMap, HashSet};

use super::UndoAction;

/// Undoable selection change. Stores the previous selection state and swaps on undo/redo.
pub struct SelectionAction {
    snapshot: Option<AlphaMask>,
}

impl SelectionAction {
    pub fn new(snapshot: Option<AlphaMask>) -> Self {
        SelectionAction { snapshot }
    }
}

impl UndoAction for SelectionAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        std::mem::swap(&mut doc.selection, &mut self.snapshot);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        std::mem::swap(&mut doc.selection, &mut self.snapshot);
        HashMap::new()
    }
}
