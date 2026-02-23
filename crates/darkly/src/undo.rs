use crate::dirty::DirtyRegion;
use crate::document::Document;
use crate::layer::{Layer, LayerId};
use crate::tile::Memento;
use std::collections::{HashMap, HashSet};

/// One undo step: a per-layer memento capturing only the tiles that changed.
pub struct UndoStep {
    /// Backward mementos: rolling these back restores the pre-step state.
    mementos: HashMap<LayerId, Memento>,
}

impl UndoStep {
    pub fn new(mementos: HashMap<LayerId, Memento>) -> Self {
        UndoStep { mementos }
    }
}

/// Forward step produced by undoing: rolling these forward re-applies the change.
struct RedoStep {
    mementos: HashMap<LayerId, Memento>,
}

pub struct UndoStack {
    undo_steps: Vec<UndoStep>,
    redo_steps: Vec<RedoStep>,
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

    /// Push a completed undo step (produced by `Document::commit_transaction`).
    /// Clears redo history.
    pub fn push(&mut self, step: UndoStep) {
        self.redo_steps.clear();
        self.undo_steps.push(step);

        // Enforce limit.
        if self.undo_steps.len() > self.max_steps {
            let remove = self.undo_steps.len() - self.max_steps;
            self.undo_steps.drain(0..remove);
        }
    }

    /// Undo the most recent step. Returns the set of affected tile coords per
    /// layer so the caller can mark them dirty for GPU re-upload.
    pub fn undo(&mut self, doc: &mut Document) -> Option<HashMap<LayerId, HashSet<(i32, i32)>>> {
        let step = self.undo_steps.pop()?;
        let mut all_affected: HashMap<LayerId, HashSet<(i32, i32)>> = HashMap::new();
        let mut forward_mementos: HashMap<LayerId, Memento> = HashMap::new();

        for (&layer_id, memento) in &step.mementos {
            if let Some(Layer::Raster(r)) = doc.layer_mut(layer_id) {
                let (forward, affected) = r.tiles.rollback(memento);
                forward_mementos.insert(layer_id, forward);
                all_affected.insert(layer_id, affected);
            }
        }

        self.redo_steps.push(RedoStep {
            mementos: forward_mementos,
        });

        Some(all_affected)
    }

    /// Redo the most recently undone step.
    pub fn redo(&mut self, doc: &mut Document) -> Option<HashMap<LayerId, HashSet<(i32, i32)>>> {
        let step = self.redo_steps.pop()?;
        let mut all_affected: HashMap<LayerId, HashSet<(i32, i32)>> = HashMap::new();
        let mut backward_mementos: HashMap<LayerId, Memento> = HashMap::new();

        for (&layer_id, memento) in &step.mementos {
            if let Some(Layer::Raster(r)) = doc.layer_mut(layer_id) {
                let (backward, affected) = r.tiles.rollforward(memento);
                backward_mementos.insert(layer_id, backward);
                all_affected.insert(layer_id, affected);
            }
        }

        self.undo_steps.push(UndoStep {
            mementos: backward_mementos,
        });

        Some(all_affected)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_steps.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_steps.is_empty()
    }
}

/// Mark the affected tiles as dirty so the compositor re-uploads them.
pub fn mark_affected_dirty(
    dirty: &mut HashMap<LayerId, DirtyRegion>,
    affected: &HashMap<LayerId, HashSet<(i32, i32)>>,
) {
    for (&layer_id, tiles) in affected {
        let region = dirty.entry(layer_id).or_insert_with(DirtyRegion::new);
        for &(tx, ty) in tiles {
            region.mark(tx, ty);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::TILE_SIZE;

    /// Helper: read every pixel in a tile and return whether all are fully transparent.
    fn tile_is_blank(doc: &Document, layer_id: LayerId, tx: i32, ty: i32) -> bool {
        let r = match doc.layer(layer_id) {
            Some(Layer::Raster(r)) => r,
            _ => return true,
        };
        match r.tiles.get(tx, ty) {
            None => true,
            Some(t) => {
                let data = t.data();
                for y in 0..TILE_SIZE {
                    for x in 0..TILE_SIZE {
                        if data.pixel(x, y)[3] != 0 {
                            return false;
                        }
                    }
                }
                true
            }
        }
    }

    /// Helper: collect all non-transparent pixel values from a tile.
    fn non_transparent_pixels(doc: &Document, layer_id: LayerId, tx: i32, ty: i32) -> Vec<(usize, usize, [u8; 4])> {
        let r = match doc.layer(layer_id) {
            Some(Layer::Raster(r)) => r,
            _ => return vec![],
        };
        let tile = match r.tiles.get(tx, ty) {
            Some(t) => t,
            None => return vec![],
        };
        let data = tile.data();
        let mut pixels = Vec::new();
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let px = data.pixel(x, y);
                if px[3] != 0 {
                    pixels.push((x, y, *px));
                }
            }
        }
        pixels
    }

    #[test]
    fn undo_semitransparent_dab_on_empty_layer() {
        // This reproduces the actual user scenario: paint a semi-transparent
        // dab (alpha=200) on an empty layer, then undo.
        let mut doc = Document::new(128, 128);
        let id = doc.add_raster_layer();
        let mut undo = UndoStack::new(100);

        // Verify layer starts empty.
        assert!(tile_is_blank(&doc, id, 0, 0));

        // Paint a semi-transparent circle (matches App.svelte: a=200).
        doc.begin_transaction(id);
        doc.paint_circle(id, 32.0, 32.0, 5.0, [220, 180, 60, 200]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(step);
        }

        // Verify the dab is there with correct alpha.
        let painted_pixels = non_transparent_pixels(&doc, id, 0, 0);
        assert!(!painted_pixels.is_empty(), "dab should have painted pixels");
        // Center pixel should have alpha=200, not 255.
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            let px = r.tiles.get(0, 0).unwrap().data().pixel(32, 32);
            assert_eq!(px[3], 200, "center pixel alpha should be 200, got {}", px[3]);
        }

        // Undo. The tile should be completely blank again.
        let affected = undo.undo(&mut doc).unwrap();
        mark_affected_dirty(&mut doc.dirty, &affected);

        assert!(
            tile_is_blank(&doc, id, 0, 0),
            "after undo, tile (0,0) should be blank but has pixels: {:?}",
            non_transparent_pixels(&doc, id, 0, 0),
        );

        // Redo. The dab should be back with original alpha.
        let affected = undo.redo(&mut doc).unwrap();
        mark_affected_dirty(&mut doc.dirty, &affected);

        let redone_pixels = non_transparent_pixels(&doc, id, 0, 0);
        assert_eq!(
            painted_pixels, redone_pixels,
            "redo should restore exactly the same pixels"
        );
    }

    #[test]
    fn undo_two_overlapping_strokes() {
        // Paint two overlapping semi-transparent strokes. Undo should
        // restore to the state after stroke 1, not to blank.
        let mut doc = Document::new(128, 128);
        let id = doc.add_raster_layer();
        let mut undo = UndoStack::new(100);

        // Stroke 1: semi-transparent dab at center.
        doc.begin_transaction(id);
        doc.paint_circle(id, 32.0, 32.0, 5.0, [220, 180, 60, 200]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(step);
        }

        // Capture state after stroke 1.
        let after_stroke1 = non_transparent_pixels(&doc, id, 0, 0);

        // Stroke 2: another dab on top (overlapping).
        doc.begin_transaction(id);
        doc.paint_circle(id, 32.0, 32.0, 5.0, [220, 180, 60, 200]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(step);
        }

        // After two blended dabs, alpha should be higher than 200.
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            let px = r.tiles.get(0, 0).unwrap().data().pixel(32, 32);
            assert!(px[3] > 200, "two overlapping dabs should blend: alpha={}", px[3]);
        }

        // Undo stroke 2 — should restore to after-stroke-1 state exactly.
        let affected = undo.undo(&mut doc).unwrap();
        mark_affected_dirty(&mut doc.dirty, &affected);

        let after_undo = non_transparent_pixels(&doc, id, 0, 0);
        assert_eq!(
            after_stroke1, after_undo,
            "undoing stroke 2 should restore exact state after stroke 1"
        );
    }

    #[test]
    fn undo_clears_redo() {
        let mut doc = Document::new(128, 128);
        let id = doc.add_raster_layer();
        let mut undo = UndoStack::new(100);

        // Stroke 1.
        doc.begin_transaction(id);
        doc.paint_circle(id, 10.0, 10.0, 3.0, [255, 0, 0, 255]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(step);
        }

        // Stroke 2.
        doc.begin_transaction(id);
        doc.paint_circle(id, 50.0, 50.0, 3.0, [0, 255, 0, 255]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(step);
        }

        // Undo stroke 2.
        undo.undo(&mut doc);
        assert!(undo.can_redo());

        // New stroke should clear redo.
        doc.begin_transaction(id);
        doc.paint_circle(id, 70.0, 70.0, 3.0, [0, 0, 255, 255]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(step);
        }
        assert!(!undo.can_redo());
    }
}
