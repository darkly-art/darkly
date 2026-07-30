use std::collections::{HashMap, HashSet};

use crate::coord::CanvasRect;
use crate::document::Document;
use crate::layer::LayerId;

use super::UndoAction;

/// Swaps a pixel entity's authoritative extent. GPU realization is reconciled
/// generically by the engine's undo executor before region restoration.
pub struct PixelBoundsAction {
    node_id: LayerId,
    bounds: CanvasRect,
}

impl PixelBoundsAction {
    pub fn new(node_id: LayerId, previous_bounds: CanvasRect) -> Self {
        Self {
            node_id,
            bounds: previous_bounds,
        }
    }

    fn swap(&mut self, doc: &mut Document) {
        let Some(current) = doc.node_pixel_bounds(self.node_id) else {
            return;
        };
        doc.set_node_pixel_bounds(self.node_id, self.bounds);
        self.bounds = current;
    }
}

impl UndoAction for PixelBoundsAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.swap(doc);
        HashMap::new()
    }
}
