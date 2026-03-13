use super::UndoAction;
use crate::document::Document;
use crate::paint::TransactionMemento;
use crate::layer::{Layer, LayerId};
use std::collections::{HashMap, HashSet};

/// Undo action for paint operations on either layer tiles or mask tiles.
///
/// Wraps a `TransactionMemento` which is either `Tiles` (RGBA layer data)
/// or `Mask` (AlphaF32 mask data). The action flip-flops on each undo/redo.
pub struct TileAction {
    memento: TransactionMemento,
}

impl TileAction {
    pub fn new(memento: TransactionMemento) -> Self {
        TileAction { memento }
    }
}

impl UndoAction for TileAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        match &self.memento {
            TransactionMemento::Tiles(mementos) => {
                let mut new_mementos = HashMap::new();
                let mut all_affected = HashMap::new();

                for (&layer_id, memento) in mementos {
                    if let Some(Layer::Raster(r)) = doc.layer_mut(layer_id) {
                        let (forward, affected) = r.tiles.rollback(memento);
                        new_mementos.insert(layer_id, forward);
                        all_affected.insert(layer_id, affected);
                    }
                }

                self.memento = TransactionMemento::Tiles(new_mementos);
                all_affected
            }
            TransactionMemento::Mask(layer_id, memento) => {
                let layer_id = *layer_id;
                if let Some(Layer::Raster(r)) = doc.layer_mut(layer_id) {
                    if let Some(mask) = &mut r.mask {
                        let (forward, affected) = mask.rollback(memento);
                        let dirty = doc.mask_dirty.entry(layer_id).or_default();
                        for &(tx, ty) in &affected {
                            dirty.mark(tx, ty);
                        }
                        self.memento = TransactionMemento::Mask(layer_id, forward);
                    }
                }
                HashMap::new()
            }
        }
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        match &self.memento {
            TransactionMemento::Tiles(mementos) => {
                let mut new_mementos = HashMap::new();
                let mut all_affected = HashMap::new();

                for (&layer_id, memento) in mementos {
                    if let Some(Layer::Raster(r)) = doc.layer_mut(layer_id) {
                        let (backward, affected) = r.tiles.rollforward(memento);
                        new_mementos.insert(layer_id, backward);
                        all_affected.insert(layer_id, affected);
                    }
                }

                self.memento = TransactionMemento::Tiles(new_mementos);
                all_affected
            }
            TransactionMemento::Mask(layer_id, memento) => {
                let layer_id = *layer_id;
                if let Some(Layer::Raster(r)) = doc.layer_mut(layer_id) {
                    if let Some(mask) = &mut r.mask {
                        let (backward, affected) = mask.rollforward(memento);
                        let dirty = doc.mask_dirty.entry(layer_id).or_default();
                        for &(tx, ty) in &affected {
                            dirty.mark(tx, ty);
                        }
                        self.memento = TransactionMemento::Mask(layer_id, backward);
                    }
                }
                HashMap::new()
            }
        }
    }
}
