use std::collections::HashMap;

use crate::document::Document;
use crate::layer::{Layer, LayerId};
use crate::tile::TileGrid;

/// A snapshot of all raster layer tile grids. Filter layers have no tile data.
/// Thanks to COW (Arc<TileData>), cloning a TileGrid only bumps refcounts.
pub struct UndoSnapshot {
    pub tiles: HashMap<LayerId, TileGrid>,
}

/// Undo/redo stack using COW tile snapshots.
///
/// Model: `snapshots` is a list of document states. `cursor` points one past
/// the "current" state (i.e., the current state is `snapshots[cursor - 1]`).
/// - `undo` decrements cursor and applies `snapshots[cursor - 1]`
/// - `redo` applies `snapshots[cursor]` and increments cursor
/// - `push` truncates any redo history, appends, and advances cursor
pub struct UndoStack {
    pub(crate) snapshots: Vec<UndoSnapshot>,
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

    /// Take a COW snapshot of all raster layers in the document.
    pub fn push(&mut self, doc: &Document) {
        // Discard any redo history beyond cursor
        self.snapshots.truncate(self.cursor);

        let mut tiles = HashMap::new();
        for layer in &doc.layers {
            if let Layer::Raster(raster) = layer {
                tiles.insert(raster.id, raster.tiles.snapshot());
            }
        }

        self.snapshots.push(UndoSnapshot { tiles });
        self.cursor = self.snapshots.len();

        // Evict oldest if over limit
        if self.snapshots.len() > self.max_snapshots {
            self.snapshots.remove(0);
            self.cursor = self.snapshots.len();
        }
    }

    /// Restore document to the previous snapshot.
    pub fn undo(&mut self, doc: &mut Document) {
        // cursor must be >= 2: we need a previous state to go back to
        if self.cursor < 2 {
            return;
        }
        self.cursor -= 1;
        self.apply_snapshot(doc, self.cursor - 1);
    }

    /// Move forward to a more recent snapshot.
    pub fn redo(&mut self, doc: &mut Document) {
        if self.cursor >= self.snapshots.len() {
            return;
        }
        self.apply_snapshot(doc, self.cursor);
        self.cursor += 1;
    }

    fn apply_snapshot(&self, doc: &mut Document, index: usize) {
        let snapshot = &self.snapshots[index];
        for layer in &mut doc.layers {
            if let Layer::Raster(raster) = layer {
                if let Some(saved_grid) = snapshot.tiles.get(&raster.id) {
                    // Mark current + restored tiles dirty for GPU re-upload
                    if let Some(dirty) = doc.dirty.get_mut(&raster.id) {
                        for &(tx, ty) in raster.tiles.tiles.keys() {
                            dirty.mark(tx, ty);
                        }
                        for &(tx, ty) in saved_grid.tiles.keys() {
                            dirty.mark(tx, ty);
                        }
                    }

                    raster.tiles = saved_grid.snapshot(); // COW clone
                }
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        self.cursor >= 2
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.snapshots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_cycle() {
        let mut doc = Document::new(256, 256);
        let id = doc.add_raster_layer();
        let mut undo = UndoStack::new(100);

        // Snapshot initial state (empty)
        undo.push(&doc);

        // Paint something
        doc.paint_circle(id, 32.0, 32.0, 5.0, [255, 0, 0, 255]);
        doc.clear_dirty();

        // Snapshot after paint
        undo.push(&doc);

        // Verify paint is there
        let pixel = doc.raster_layer(id).unwrap().tiles.get(0, 0).unwrap().data.pixel(32, 32);
        assert_eq!(pixel, &[255, 0, 0, 255]);

        // Undo — should restore to empty state (snapshot[0])
        undo.undo(&mut doc);
        let tile = doc.raster_layer(id).unwrap().tiles.get(0, 0);
        assert!(tile.is_none(), "tile should not exist after undo to empty state");

        // Redo — should restore paint (snapshot[1])
        undo.redo(&mut doc);
        let pixel = doc.raster_layer(id).unwrap().tiles.get(0, 0).unwrap().data.pixel(32, 32);
        assert_eq!(pixel, &[255, 0, 0, 255]);
    }

    #[test]
    fn undo_push_discards_redo() {
        let mut doc = Document::new(256, 256);
        let id = doc.add_raster_layer();
        let mut undo = UndoStack::new(100);

        undo.push(&doc); // state 0
        doc.paint_circle(id, 10.0, 10.0, 3.0, [255, 0, 0, 255]);
        undo.push(&doc); // state 1
        doc.paint_circle(id, 50.0, 50.0, 3.0, [0, 255, 0, 255]);
        undo.push(&doc); // state 2

        // Undo twice (back to state 0)
        undo.undo(&mut doc);
        undo.undo(&mut doc);

        // Now push a new state — should discard states 1 and 2
        doc.paint_circle(id, 30.0, 30.0, 3.0, [0, 0, 255, 255]);
        undo.push(&doc);

        assert!(!undo.can_redo());
    }

    #[test]
    fn cow_efficiency() {
        let mut doc = Document::new(256, 256);
        let id = doc.add_raster_layer();
        let mut undo = UndoStack::new(100);

        // Paint on tile (0,0)
        doc.paint_circle(id, 32.0, 32.0, 5.0, [255, 0, 0, 255]);

        // Snapshot — tile data should be shared (same Arc)
        undo.push(&doc);

        let raster = doc.raster_layer(id).unwrap();
        let snapshot_grid = &undo.snapshots[0].tiles[&id];
        let doc_tile = raster.tiles.get(0, 0).unwrap();
        let snap_tile = snapshot_grid.get(0, 0).unwrap();
        assert!(std::sync::Arc::ptr_eq(&doc_tile.data, &snap_tile.data));
    }
}
