//! Undo action for moving the screen-space boundary — the divider the layer
//! panel draws across the stack.
//!
//! One drag is one scalar edit, so this restores a plain document field in the
//! shape of [`CanvasResizeAction`] and [`SelectionMetadataAction`]. There is no
//! per-layer flag to fan out over and no compound to assemble: the boundary is
//! a count of the root's trailing children, and a group crosses it whole.
//!
//! [`CanvasResizeAction`]: super::CanvasResizeAction
//! [`SelectionMetadataAction`]: super::SelectionMetadataAction

use super::UndoAction;
use crate::document::Document;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

pub struct ScreenSpaceBoundaryAction {
    old: usize,
    new: usize,
}

impl ScreenSpaceBoundaryAction {
    pub fn new(old: usize, new: usize) -> Self {
        ScreenSpaceBoundaryAction { old, new }
    }
}

impl UndoAction for ScreenSpaceBoundaryAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.screen_space_count = self.old;
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        doc.screen_space_count = self.new;
        HashMap::new()
    }
}
