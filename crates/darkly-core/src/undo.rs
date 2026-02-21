use crate::dirty::DirtyRegion;
use crate::document::Document;
use crate::layer::{Layer, LayerId};
use crate::tile::TileGrid;
use std::collections::HashMap;

/// A snapshot of all raster layer tile grids. Cheap thanks to Arc COW tiles.
pub struct UndoSnapshot {
    tiles: HashMap<LayerId, TileGrid>,
}

pub struct UndoStack {
    snapshots: Vec<UndoSnapshot>,
    cursor: usize,
    max_snapshots: usize,
}

impl UndoStack {
    pub fn new(max_snapshots: usize) -> Self {
        UndoStack {
            snapshots: Vec::new(),
            cursor: 0,
            max_snapshots,
        }
    }

    /// Push a snapshot of the current document state.
    /// Truncates any redo history beyond the cursor.
    pub fn push(&mut self, doc: &Document) {
        // Truncate redo history
        self.snapshots.truncate(self.cursor);

        let mut tiles = HashMap::new();
        for layer in &doc.layers {
            if let Layer::Raster(r) = layer {
                tiles.insert(r.id, r.tiles.snapshot());
            }
        }

        self.snapshots.push(UndoSnapshot { tiles });
        self.cursor = self.snapshots.len();

        // Enforce max snapshots
        if self.snapshots.len() > self.max_snapshots {
            let remove = self.snapshots.len() - self.max_snapshots;
            self.snapshots.drain(0..remove);
            self.cursor = self.snapshots.len();
        }
    }

    /// Undo: restore the previous snapshot.
    /// Returns true if undo was performed.
    pub fn undo(&mut self, doc: &mut Document) -> bool {
        if self.cursor == 0 {
            return false;
        }

        // Save current state if we're at the top (so redo works)
        if self.cursor == self.snapshots.len() {
            let mut tiles = HashMap::new();
            for layer in &doc.layers {
                if let Layer::Raster(r) = layer {
                    tiles.insert(r.id, r.tiles.snapshot());
                }
            }
            self.snapshots.push(UndoSnapshot { tiles });
        }

        self.cursor -= 1;
        self.apply_snapshot(doc, self.cursor);
        true
    }

    /// Redo: restore the next snapshot.
    /// Returns true if redo was performed.
    pub fn redo(&mut self, doc: &mut Document) -> bool {
        if self.cursor + 1 >= self.snapshots.len() {
            return false;
        }

        self.cursor += 1;
        self.apply_snapshot(doc, self.cursor);
        true
    }

    fn apply_snapshot(&self, doc: &mut Document, index: usize) {
        let snap = &self.snapshots[index];
        for layer in &mut doc.layers {
            if let Layer::Raster(r) = layer {
                if let Some(grid) = snap.tiles.get(&r.id) {
                    r.tiles = grid.snapshot();
                    // Mark all tiles dirty so GPU re-uploads
                    let dirty = doc.dirty.entry(r.id).or_insert_with(DirtyRegion::new);
                    for ((tx, ty), _) in r.tiles.iter() {
                        dirty.mark(tx, ty);
                    }
                }
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.snapshots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_basic() {
        let mut doc = Document::new(128, 128);
        let id = doc.add_raster_layer();
        let mut undo = UndoStack::new(100);

        // Snapshot before painting (captures empty state)
        undo.push(&doc);

        // Paint
        doc.paint_circle(id, 32.0, 32.0, 5.0, [255, 0, 0, 255]);

        // Verify paint is there
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            assert_eq!(r.tiles.get(0, 0).unwrap().data().pixel(32, 32), &[255, 0, 0, 255]);
        }

        // Undo — should restore to snapshot[0] (empty state)
        assert!(undo.undo(&mut doc));
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            // Grid should be empty (no tiles) or tile should be zeroed
            match r.tiles.get(0, 0) {
                None => {} // Expected: tile doesn't exist
                Some(t) => assert_eq!(t.data().pixel(32, 32), &[0, 0, 0, 0]),
            }
        }

        // Redo — should restore to the painted state
        assert!(undo.redo(&mut doc));
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            assert_eq!(r.tiles.get(0, 0).unwrap().data().pixel(32, 32), &[255, 0, 0, 255]);
        }
    }
}
