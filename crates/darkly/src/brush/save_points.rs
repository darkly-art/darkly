//! Per-dab save points for O(1) stroke rewind.
//!
//! Each save point records the cumulative bounding box of all dabs placed
//! up to that point and the polyline vector index the dab was placed on.
//! This lets the stabilizer rewind to any dab index by looking up the
//! region that needs to be restored — no GPU readback required.
//!
//! Save points also serve as **checkpoints** for partial re-render: each
//! stores a `RenderCheckpoint` (the stroke engine's render state at that dab)
//! and a flag indicating whether the checkpoint texture contains a snapshot
//! taken at this save point. When divergence occurs, the engine can restore
//! the stroke buffer from the checkpoint texture and re-render only from
//! there to the tip.

use super::stroke_engine::RenderCheckpoint;

/// A single save point recorded when a dab is placed.
#[derive(Clone)]
pub struct DabSavePoint {
    /// Union of all dab bounding boxes from dab 0..=this one.
    /// `[x, y, width, height]` in canvas pixels.
    pub cumulative_bbox: [u32; 4],
    /// Index into the stabilized polyline that this dab was placed on.
    pub vector_index: usize,
    /// True if the checkpoint texture contains a snapshot taken at this save point.
    pub has_checkpoint: bool,
    /// Checkpoint: render state at this dab.
    pub render_state: RenderCheckpoint,
}

/// Accumulator of per-dab save points for the current stroke.
pub struct SavePointStore {
    points: Vec<DabSavePoint>,
}

impl SavePointStore {
    pub fn new() -> Self {
        Self { points: Vec::with_capacity(512) }
    }

    /// Record a new dab.  `dab_bbox` is `[x, y, w, h]` in canvas pixels.
    pub fn push(&mut self, dab_bbox: [u32; 4], vector_index: usize, render_state: RenderCheckpoint) {
        let cumulative = if let Some(prev) = self.points.last() {
            union_bbox(prev.cumulative_bbox, dab_bbox)
        } else {
            dab_bbox
        };
        self.points.push(DabSavePoint {
            cumulative_bbox: cumulative,
            vector_index,
            has_checkpoint: false,
            render_state,
        });
    }

    /// Find the nearest checkpoint strictly before the given vector index
    /// that has pixel data. Returns the save point index (not vector index).
    ///
    /// Strict less-than is critical: the divergence_index marks the first
    /// vector index whose stabilized position changed.  A checkpoint AT that
    /// index would contain dabs at the old (stale) positions.  Only
    /// checkpoints BEFORE the divergence are guaranteed to have unchanged
    /// pixel content.
    pub fn checkpoint_before(&self, vector_index: usize) -> Option<usize> {
        for (i, sp) in self.points.iter().enumerate().rev() {
            if sp.vector_index < vector_index && sp.has_checkpoint {
                return Some(i);
            }
        }
        None
    }

    /// Mark the save point at `index` as having a checkpoint texture snapshot.
    pub fn mark_checkpoint(&mut self, index: usize) {
        if let Some(sp) = self.points.get_mut(index) {
            sp.has_checkpoint = true;
        }
    }

    /// Clear checkpoint flags on all save points after `index`.
    /// Used after truncation, since those checkpoints are invalidated.
    pub fn clear_checkpoints_after(&mut self, index: usize) {
        for sp in self.points.iter_mut().skip(index + 1) {
            sp.has_checkpoint = false;
        }
    }

    /// Cumulative bounding box up to (and including) the given dab index.
    /// Returns `None` if the index is out of range.
    pub fn rewind_bbox(&self, dab_index: usize) -> Option<[u32; 4]> {
        self.points.get(dab_index).map(|sp| sp.cumulative_bbox)
    }

    /// Cumulative bounding box of all dabs (= last save point's bbox).
    pub fn full_bbox(&self) -> Option<[u32; 4]> {
        self.points.last().map(|sp| sp.cumulative_bbox)
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

    /// Access individual save points (for divergence-based rewind).
    pub fn get(&self, index: usize) -> Option<&DabSavePoint> {
        self.points.get(index)
    }

    /// Access the underlying save point slice.
    pub fn points(&self) -> &[DabSavePoint] {
        &self.points
    }

    /// Update the render state on ALL save points that share the given
    /// vector index.  Called at the end of each vector index iteration so
    /// every save point for that segment represents "everything through
    /// this vector index is fully processed" — regardless of which one an
    /// async readback delivers pixels to.
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

/// Compute the union of two `[x, y, w, h]` bounding boxes.
fn union_bbox(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    let ax2 = a[0] + a[2];
    let ay2 = a[1] + a[3];
    let bx2 = b[0] + b[2];
    let by2 = b[1] + b[3];

    let x = a[0].min(b[0]);
    let y = a[1].min(b[1]);
    let x2 = ax2.max(bx2);
    let y2 = ay2.max(by2);
    [x, y, x2 - x, y2 - y]
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
            dab_count: 0,
        }
    }

    #[test]
    fn single_dab() {
        let mut store = SavePointStore::new();
        store.push([10, 20, 30, 40], 0, dummy_checkpoint());
        assert_eq!(store.len(), 1);
        assert_eq!(store.full_bbox(), Some([10, 20, 30, 40]));
        assert_eq!(store.rewind_bbox(0), Some([10, 20, 30, 40]));
    }

    #[test]
    fn cumulative_bbox_grows() {
        let mut store = SavePointStore::new();
        store.push([10, 10, 5, 5], 0, dummy_checkpoint());
        store.push([20, 20, 5, 5], 1, dummy_checkpoint());

        // First dab: just [10, 10, 5, 5].
        assert_eq!(store.rewind_bbox(0), Some([10, 10, 5, 5]));
        // Second dab: union = [10, 10, 15, 15].
        assert_eq!(store.rewind_bbox(1), Some([10, 10, 15, 15]));
        assert_eq!(store.full_bbox(), Some([10, 10, 15, 15]));
    }

    #[test]
    fn truncate_removes_tail() {
        let mut store = SavePointStore::new();
        for i in 0..5 {
            store.push([i * 10, 0, 5, 5], i as usize, dummy_checkpoint());
        }
        assert_eq!(store.len(), 5);
        store.truncate(3);
        assert_eq!(store.len(), 3);
        assert!(store.rewind_bbox(3).is_none());
    }

    #[test]
    fn clear_empties() {
        let mut store = SavePointStore::new();
        store.push([0, 0, 10, 10], 0, dummy_checkpoint());
        store.clear();
        assert!(store.is_empty());
        assert!(store.full_bbox().is_none());
    }

    #[test]
    fn union_bbox_correctness() {
        assert_eq!(union_bbox([0, 0, 10, 10], [5, 5, 10, 10]), [0, 0, 15, 15]);
        assert_eq!(union_bbox([10, 10, 5, 5], [0, 0, 5, 5]), [0, 0, 15, 15]);
        assert_eq!(union_bbox([0, 0, 10, 10], [0, 0, 10, 10]), [0, 0, 10, 10]);
    }

    #[test]
    fn vector_index_preserved() {
        let mut store = SavePointStore::new();
        store.push([0, 0, 5, 5], 42, dummy_checkpoint());
        store.push([10, 10, 5, 5], 99, dummy_checkpoint());
        assert_eq!(store.get(0).unwrap().vector_index, 42);
        assert_eq!(store.get(1).unwrap().vector_index, 99);
    }

    #[test]
    fn checkpoint_before_finds_marked() {
        let mut store = SavePointStore::new();
        for i in 0..5 {
            store.push([i * 10, 0, 5, 5], i as usize, dummy_checkpoint());
        }
        // No checkpoints — should return None.
        assert!(store.checkpoint_before(4).is_none());

        // Mark save point 2 (vector_index=2) as having a checkpoint.
        store.mark_checkpoint(2);
        // Looking for checkpoint before vector_index 4 → should find index 2.
        assert_eq!(store.checkpoint_before(4), Some(2));
        // Looking for checkpoint before vector_index 2 → None (strict less-than).
        assert!(store.checkpoint_before(2).is_none());
        // Looking for checkpoint before vector_index 3 → index 2.
        assert_eq!(store.checkpoint_before(3), Some(2));
        // Looking for checkpoint before vector_index 1 → None.
        assert!(store.checkpoint_before(1).is_none());
    }

    #[test]
    fn mark_checkpoint_sets_flag() {
        let mut store = SavePointStore::new();
        store.push([0, 0, 5, 5], 0, dummy_checkpoint());
        assert!(!store.get(0).unwrap().has_checkpoint);
        store.mark_checkpoint(0);
        assert!(store.get(0).unwrap().has_checkpoint);
    }

    #[test]
    fn clear_checkpoints_after() {
        let mut store = SavePointStore::new();
        for i in 0..5 {
            store.push([i * 10, 0, 5, 5], i as usize, dummy_checkpoint());
            store.mark_checkpoint(i as usize);
        }
        store.clear_checkpoints_after(2);
        assert!(store.get(0).unwrap().has_checkpoint);
        assert!(store.get(1).unwrap().has_checkpoint);
        assert!(store.get(2).unwrap().has_checkpoint);
        assert!(!store.get(3).unwrap().has_checkpoint);
        assert!(!store.get(4).unwrap().has_checkpoint);
    }
}
