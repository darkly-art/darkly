mod tile;
mod layer;
pub mod property;

pub use tile::TileAction;
pub use layer::{LayerAddAction, LayerRemoveAction, LayerMoveAction};
pub use property::PropertyAction;

use crate::dirty::DirtyRegion;
use crate::document::Document;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// A reversible action that can be undone and redone.
///
/// Each action stores enough state to flip-flop: calling `undo()` transforms
/// the internal state so that a subsequent `redo()` reverses it, and vice versa.
/// The action is moved between the undo and redo stacks as a single `Box<dyn UndoAction>`.
pub trait UndoAction {
    /// Reverse this action. Returns affected tile coordinates per layer for GPU dirty marking.
    /// For non-tile actions (layer structure, properties), returns an empty map.
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>>;

    /// Re-apply this action. Returns affected tile coordinates per layer for GPU dirty marking.
    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>>;
}

pub struct UndoStack {
    undo_steps: Vec<Box<dyn UndoAction>>,
    redo_steps: Vec<Box<dyn UndoAction>>,
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

    /// Push a completed action. Clears redo history.
    pub fn push(&mut self, action: Box<dyn UndoAction>) {
        self.redo_steps.clear();
        self.undo_steps.push(action);

        if self.undo_steps.len() > self.max_steps {
            let remove = self.undo_steps.len() - self.max_steps;
            self.undo_steps.drain(0..remove);
        }
    }

    /// Undo the most recent action. Returns affected tile coords per layer.
    pub fn undo(&mut self, doc: &mut Document) -> Option<HashMap<LayerId, HashSet<(i32, i32)>>> {
        let mut action = self.undo_steps.pop()?;
        let affected = action.undo(doc);
        self.redo_steps.push(action);
        Some(affected)
    }

    /// Redo the most recently undone action. Returns affected tile coords per layer.
    pub fn redo(&mut self, doc: &mut Document) -> Option<HashMap<LayerId, HashSet<(i32, i32)>>> {
        let mut action = self.redo_steps.pop()?;
        let affected = action.redo(doc);
        self.undo_steps.push(action);
        Some(affected)
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
    use crate::layer::Layer;

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
        let mut doc = Document::new(128, 128);
        let id = doc.add_raster_layer();
        let mut undo = UndoStack::new(100);

        assert!(tile_is_blank(&doc, id, 0, 0));

        doc.begin_transaction(id);
        doc.paint_circle(id, 32.0, 32.0, 5.0, [220, 180, 60, 200]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(Box::new(TileAction::new(step)));
        }

        let painted_pixels = non_transparent_pixels(&doc, id, 0, 0);
        assert!(!painted_pixels.is_empty(), "dab should have painted pixels");
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            let px = r.tiles.get(0, 0).unwrap().data().pixel(32, 32);
            assert_eq!(px[3], 200, "center pixel alpha should be 200, got {}", px[3]);
        }

        let affected = undo.undo(&mut doc).unwrap();
        mark_affected_dirty(&mut doc.dirty, &affected);

        assert!(
            tile_is_blank(&doc, id, 0, 0),
            "after undo, tile (0,0) should be blank but has pixels: {:?}",
            non_transparent_pixels(&doc, id, 0, 0),
        );

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
        let mut doc = Document::new(128, 128);
        let id = doc.add_raster_layer();
        let mut undo = UndoStack::new(100);

        doc.begin_transaction(id);
        doc.paint_circle(id, 32.0, 32.0, 5.0, [220, 180, 60, 200]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(Box::new(TileAction::new(step)));
        }

        let after_stroke1 = non_transparent_pixels(&doc, id, 0, 0);

        doc.begin_transaction(id);
        doc.paint_circle(id, 32.0, 32.0, 5.0, [220, 180, 60, 200]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(Box::new(TileAction::new(step)));
        }

        if let Some(Layer::Raster(r)) = doc.layer(id) {
            let px = r.tiles.get(0, 0).unwrap().data().pixel(32, 32);
            assert!(px[3] > 200, "two overlapping dabs should blend: alpha={}", px[3]);
        }

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

        doc.begin_transaction(id);
        doc.paint_circle(id, 10.0, 10.0, 3.0, [255, 0, 0, 255]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(Box::new(TileAction::new(step)));
        }

        doc.begin_transaction(id);
        doc.paint_circle(id, 50.0, 50.0, 3.0, [0, 255, 0, 255]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(Box::new(TileAction::new(step)));
        }

        undo.undo(&mut doc);
        assert!(undo.can_redo());

        doc.begin_transaction(id);
        doc.paint_circle(id, 70.0, 70.0, 3.0, [0, 0, 255, 255]);
        if let Some(step) = doc.commit_transaction(id) {
            undo.push(Box::new(TileAction::new(step)));
        }
        assert!(!undo.can_redo());
    }

    #[test]
    fn undo_layer_add_remove() {
        let mut doc = Document::new(128, 128);
        let mut undo = UndoStack::new(100);

        let id = doc.add_raster_layer();

        // Paint something on the layer so we can verify it survives undo/redo.
        doc.paint_circle(id, 32.0, 32.0, 5.0, [255, 0, 0, 255]);
        let painted = non_transparent_pixels(&doc, id, 0, 0);

        // Record the add as undoable.
        let parent = doc.parent_of(id);
        let pos = doc.position_in_parent(id).unwrap();
        undo.push(Box::new(LayerAddAction::new(id, parent, pos)));

        assert_eq!(doc.flat_layers().len(), 1);

        // Undo the add — layer should be removed.
        undo.undo(&mut doc);
        assert_eq!(doc.flat_layers().len(), 0);

        // Redo — layer comes back with its tile data.
        undo.redo(&mut doc);
        assert_eq!(doc.flat_layers().len(), 1);
        let restored = non_transparent_pixels(&doc, id, 0, 0);
        assert_eq!(painted, restored, "redo should restore layer with its tiles");
    }

    #[test]
    fn undo_layer_remove() {
        let mut doc = Document::new(128, 128);
        let mut undo = UndoStack::new(100);

        let id = doc.add_raster_layer();
        doc.paint_circle(id, 32.0, 32.0, 5.0, [255, 0, 0, 255]);
        let painted = non_transparent_pixels(&doc, id, 0, 0);

        // Remove the layer (undoable).
        let parent = doc.parent_of(id);
        let pos = doc.position_in_parent(id).unwrap();
        let node = doc.detach_for_undo(id).unwrap();
        undo.push(Box::new(LayerRemoveAction::new(node, parent, pos)));

        assert_eq!(doc.flat_layers().len(), 0);

        // Undo the remove — layer should come back.
        undo.undo(&mut doc);
        assert_eq!(doc.flat_layers().len(), 1);
        let restored = non_transparent_pixels(&doc, id, 0, 0);
        assert_eq!(painted, restored);

        // Redo the remove — layer gone again.
        undo.redo(&mut doc);
        assert_eq!(doc.flat_layers().len(), 0);
    }

    #[test]
    fn undo_layer_move() {
        let mut doc = Document::new(128, 128);
        let mut undo = UndoStack::new(100);

        let l1 = doc.add_raster_layer();
        let l2 = doc.add_raster_layer();
        let l3 = doc.add_raster_layer();

        // Order: l1, l2, l3 (bottom to top).
        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l1, l2, l3]);

        // Move l1 to the top (after l3).
        let old_parent = doc.parent_of(l1);
        let old_pos = doc.position_in_parent(l1).unwrap();
        doc.move_layer(l1, crate::document::MoveTarget::After(l3));
        let new_parent = doc.parent_of(l1);
        let new_pos = doc.position_in_parent(l1).unwrap();

        undo.push(Box::new(LayerMoveAction::new(l1, old_parent, old_pos, new_parent, new_pos)));

        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l2, l3, l1]);

        // Undo — back to original order.
        undo.undo(&mut doc);
        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l1, l2, l3]);

        // Redo — moved again.
        undo.redo(&mut doc);
        let flat: Vec<_> = doc.flat_layers().iter().map(|l| l.id()).collect();
        assert_eq!(flat, vec![l2, l3, l1]);
    }

    #[test]
    fn undo_property_change() {
        use super::property::Property;

        let mut doc = Document::new(128, 128);
        let mut undo = UndoStack::new(100);

        let id = doc.add_raster_layer();

        // Change opacity.
        undo.push(Box::new(PropertyAction::new(
            id,
            Property::Opacity(1.0),
            Property::Opacity(0.5),
        )));
        if let Some(Layer::Raster(r)) = doc.layer_mut(id) {
            r.opacity = 0.5;
        }

        // Undo — opacity back to 1.0.
        undo.undo(&mut doc);
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            assert!((r.opacity - 1.0).abs() < f32::EPSILON);
        }

        // Redo — opacity back to 0.5.
        undo.redo(&mut doc);
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            assert!((r.opacity - 0.5).abs() < f32::EPSILON);
        }
    }
}
