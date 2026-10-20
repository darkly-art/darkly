//! Per-dab save points for O(1) stroke rewind.
//!
//! Each save point records the footprint of its dab, the cumulative
//! bounding box of all dabs placed up to that point, and the polyline
//! vector index the dab was placed on. The cumulative bbox is the undo
//! damage rect; the per-dab footprints give the region dirtied between
//! any two dab indices, which is what a checkpoint save or restore has to
//! copy. No GPU readback is involved in either.
//!
//! Save points also store a `RenderCheckpoint` (the stroke engine's render
//! state at that dab) so the checkpoint ring can restore engine state and
//! resume rendering from any save point.

use super::stroke_engine::RenderCheckpoint;
use crate::coord::CanvasRect;

/// A single save point recorded when a dab is placed.
#[derive(Clone)]
pub struct DabSavePoint {
    /// This dab's own write footprint, in canvas pixel coords; empty for a
    /// dab that wrote nothing. Stable across mid-stroke layer growth.
    pub canvas_bbox: CanvasRect,
    /// Union of all dab bounding boxes from dab 0..=this one, in canvas
    /// pixel coords. Stable across mid-stroke layer growth.
    pub cumulative_canvas_bbox: CanvasRect,
    /// Index into the stabilized polyline that this dab was placed on.
    pub vector_index: usize,
    /// Checkpoint: render state at this dab.
    pub render_state: RenderCheckpoint,
}

/// Accumulator of per-dab save points for the current stroke.
pub struct SavePointStore {
    points: Vec<DabSavePoint>,
}

impl Default for SavePointStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SavePointStore {
    pub fn new() -> Self {
        Self {
            points: Vec::with_capacity(512),
        }
    }

    /// Record a new dab. `dab_bbox` is in canvas pixels.
    pub fn push(
        &mut self,
        dab_bbox: CanvasRect,
        vector_index: usize,
        render_state: RenderCheckpoint,
    ) {
        let cumulative = if let Some(prev) = self.points.last() {
            prev.cumulative_canvas_bbox.union(dab_bbox)
        } else {
            dab_bbox
        };
        self.points.push(DabSavePoint {
            canvas_bbox: dab_bbox,
            cumulative_canvas_bbox: cumulative,
            vector_index,
            render_state,
        });
    }

    /// Cumulative bounding box of all dabs (= last save point's bbox), in
    /// canvas pixels.
    pub fn full_bbox(&self) -> Option<CanvasRect> {
        self.points.last().map(|sp| sp.cumulative_canvas_bbox)
    }

    /// Union of the footprints of the dabs with index in `(after, through]`,
    /// in canvas pixels: the region in which the scratch after dab `through`
    /// differs from the scratch after dab `after`. Empty when the range is.
    pub fn dirty_between(&self, after: usize, through: usize) -> CanvasRect {
        self.points
            .get(after + 1..=through)
            .unwrap_or(&[])
            .iter()
            .fold(CanvasRect::empty(), |union, sp| union.union(sp.canvas_bbox))
    }

    /// Union of the footprints of every dab after `after`, in canvas
    /// pixels: what a rewind to dab `after` has to undo.
    pub fn dirty_after(&self, after: usize) -> CanvasRect {
        self.dirty_between(after, self.points.len().saturating_sub(1))
    }

    /// Keep only the first `n` save points.
    pub fn truncate(&mut self, n: usize) {
        self.points.truncate(n);
    }

    pub fn clear(&mut self) {
        self.points.clear();
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Access the underlying save point slice.
    pub fn points(&self) -> &[DabSavePoint] {
        &self.points
    }

    /// Update the render state on ALL save points that share the given
    /// vector index.  Called at the end of each vector index iteration so
    /// every save point for that segment represents "everything through
    /// this vector index is fully processed": the checkpoint restore
    /// starts from the next vector index.
    pub fn finalize_render_state(&mut self, vector_index: usize, render_state: RenderCheckpoint) {
        for sp in self.points.iter_mut().rev() {
            if sp.vector_index == vector_index {
                sp.render_state = render_state.clone();
            } else if sp.vector_index < vector_index {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_checkpoint() -> RenderCheckpoint {
        RenderCheckpoint {
            last_point: None,
            accumulated_distance: 0.0,
            leftover_distance: 0.0,
            last_dab_size: [10.0, 10.0],
            last_dab_pos: None,
            dab_count: 0,
            stamp_angle: None,
        }
    }

    fn r(x: i32, y: i32, w: u32, h: u32) -> CanvasRect {
        CanvasRect::from_xywh(x, y, w, h)
    }

    #[test]
    fn single_dab() {
        let mut store = SavePointStore::new();
        store.push(r(10, 20, 30, 40), 0, dummy_checkpoint());
        assert_eq!(store.len(), 1);
        assert_eq!(store.full_bbox(), Some(r(10, 20, 30, 40)));
    }

    #[test]
    fn cumulative_bbox_grows() {
        let mut store = SavePointStore::new();
        store.push(r(10, 10, 5, 5), 0, dummy_checkpoint());
        store.push(r(20, 20, 5, 5), 1, dummy_checkpoint());
        assert_eq!(store.full_bbox(), Some(r(10, 10, 15, 15)));
    }

    #[test]
    fn truncate_removes_tail() {
        let mut store = SavePointStore::new();
        for i in 0..5 {
            store.push(r(i * 10, 0, 5, 5), i as usize, dummy_checkpoint());
        }
        assert_eq!(store.len(), 5);
        store.truncate(3);
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn clear_empties() {
        let mut store = SavePointStore::new();
        store.push(r(0, 0, 10, 10), 0, dummy_checkpoint());
        store.clear();
        assert!(store.is_empty());
        assert!(store.full_bbox().is_none());
    }

    #[test]
    fn dirty_ranges_union_only_the_dabs_in_range() {
        let mut store = SavePointStore::new();
        store.push(r(0, 0, 10, 10), 0, dummy_checkpoint());
        store.push(r(100, 0, 10, 10), 1, dummy_checkpoint());
        store.push(r(0, 100, 10, 10), 2, dummy_checkpoint());
        store.push(r(50, 50, 10, 10), 3, dummy_checkpoint());
        // The per-dab footprint is stored as given; the cumulative one unions.
        assert_eq!(store.points()[1].canvas_bbox, r(100, 0, 10, 10));
        assert_eq!(store.points()[1].cumulative_canvas_bbox, r(0, 0, 110, 10));
        assert_eq!(store.dirty_between(0, 2), r(0, 0, 110, 110));
        assert_eq!(store.dirty_between(2, 3), r(50, 50, 10, 10));
        assert_eq!(store.dirty_after(1), r(0, 50, 60, 60));
        assert!(store.dirty_between(1, 1).is_empty(), "an empty range");
        assert!(
            store.dirty_after(3).is_empty(),
            "nothing after the last dab"
        );
        assert!(store.dirty_between(3, 9).is_empty(), "a range past the end");
        assert!(SavePointStore::new().dirty_after(0).is_empty());
    }

    #[test]
    fn vector_index_preserved() {
        let mut store = SavePointStore::new();
        store.push(r(0, 0, 5, 5), 42, dummy_checkpoint());
        store.push(r(10, 10, 5, 5), 99, dummy_checkpoint());
        let pts = store.points();
        assert_eq!(pts[0].vector_index, 42);
        assert_eq!(pts[1].vector_index, 99);
    }
}
