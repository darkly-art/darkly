use std::collections::{HashMap, HashSet};

use crate::coord::WindowRect;
use crate::document::Document;
use crate::layer::LayerId;

use super::UndoAction;

/// Exact document-side selection metadata paired with a selection GPU action.
pub struct SelectionMetadataAction {
    pixel_bounds: Option<WindowRect>,
    cpu_cache: Option<Vec<u8>>,
}

impl SelectionMetadataAction {
    pub fn new(pixel_bounds: Option<WindowRect>, cpu_cache: Option<Vec<u8>>) -> Self {
        Self {
            pixel_bounds,
            cpu_cache,
        }
    }

    fn swap(&mut self, doc: &mut Document) {
        let Some(id) = doc.selection_id() else {
            return;
        };
        let Some(selection) = doc
            .find_filter_mut(id)
            .and_then(|filter| filter.as_selection_mut())
        else {
            return;
        };
        std::mem::swap(&mut selection.pixel_bounds, &mut self.pixel_bounds);
        std::mem::swap(&mut selection.cpu_cache.data, &mut self.cpu_cache);
    }
}

impl UndoAction for SelectionMetadataAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }

    fn restores_selection_metadata(&self) -> bool {
        true
    }
}
