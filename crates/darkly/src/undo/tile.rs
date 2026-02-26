use super::UndoAction;
use crate::document::Document;
use crate::layer::{Layer, LayerId};
use crate::tile::Memento;
use std::collections::{HashMap, HashSet};

/// Undo action for tile (paint) operations.
///
/// Wraps the existing memento system. The action flip-flops: after `undo()`,
/// internal mementos hold the forward state (for redo). After `redo()`, they
/// hold the backward state (for undo) again.
pub struct TileAction {
    mementos: HashMap<LayerId, Memento>,
}

impl TileAction {
    pub fn new(mementos: HashMap<LayerId, Memento>) -> Self {
        TileAction { mementos }
    }
}

impl UndoAction for TileAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        let mut new_mementos = HashMap::new();
        let mut all_affected = HashMap::new();

        for (&layer_id, memento) in &self.mementos {
            if let Some(Layer::Raster(r)) = doc.layer_mut(layer_id) {
                let (forward, affected) = r.tiles.rollback(memento);
                new_mementos.insert(layer_id, forward);
                all_affected.insert(layer_id, affected);
            }
        }

        self.mementos = new_mementos;
        all_affected
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        let mut new_mementos = HashMap::new();
        let mut all_affected = HashMap::new();

        for (&layer_id, memento) in &self.mementos {
            if let Some(Layer::Raster(r)) = doc.layer_mut(layer_id) {
                let (backward, affected) = r.tiles.rollforward(memento);
                new_mementos.insert(layer_id, backward);
                all_affected.insert(layer_id, affected);
            }
        }

        self.mementos = new_mementos;
        all_affected
    }
}
