//! Per-dab save points for O(1) stroke rewind.
//!
//! Each save point records the cumulative bounding box of all dabs placed
//! up to that point and the polyline vector index the dab was placed on.
//! This lets the stabilizer rewind to any dab index by looking up the
//! region that needs to be restored — no GPU readback required.

/// A single save point recorded when a dab is placed.
#[derive(Clone, Copy, Debug)]
pub struct DabSavePoint {
    /// Union of all dab bounding boxes from dab 0..=this one.
    /// `[x, y, width, height]` in canvas pixels.
    pub cumulative_bbox: [u32; 4],
    /// Index into the stabilized polyline that this dab was placed on.
    pub vector_index: usize,
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
    pub fn push(&mut self, dab_bbox: [u32; 4], vector_index: usize) {
        let cumulative = if let Some(prev) = self.points.last() {
            union_bbox(prev.cumulative_bbox, dab_bbox)
        } else {
            dab_bbox
        };
        self.points.push(DabSavePoint { cumulative_bbox: cumulative, vector_index });
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

    #[test]
    fn single_dab() {
        let mut store = SavePointStore::new();
        store.push([10, 20, 30, 40], 0);
        assert_eq!(store.len(), 1);
        assert_eq!(store.full_bbox(), Some([10, 20, 30, 40]));
        assert_eq!(store.rewind_bbox(0), Some([10, 20, 30, 40]));
    }

    #[test]
    fn cumulative_bbox_grows() {
        let mut store = SavePointStore::new();
        store.push([10, 10, 5, 5], 0);
        store.push([20, 20, 5, 5], 1);

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
            store.push([i * 10, 0, 5, 5], i as usize);
        }
        assert_eq!(store.len(), 5);
        store.truncate(3);
        assert_eq!(store.len(), 3);
        assert!(store.rewind_bbox(3).is_none());
    }

    #[test]
    fn clear_empties() {
        let mut store = SavePointStore::new();
        store.push([0, 0, 10, 10], 0);
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
        store.push([0, 0, 5, 5], 42);
        store.push([10, 10, 5, 5], 99);
        assert_eq!(store.get(0).unwrap().vector_index, 42);
        assert_eq!(store.get(1).unwrap().vector_index, 99);
    }
}
