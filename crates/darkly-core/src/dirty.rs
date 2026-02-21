use std::collections::HashSet;

use crate::tile::TileGrid;

/// Tracks which tiles have been modified and need GPU re-upload.
pub struct DirtyRegion {
    pub tiles: HashSet<(i32, i32)>,
}

impl DirtyRegion {
    pub fn new() -> Self {
        DirtyRegion {
            tiles: HashSet::new(),
        }
    }

    /// Mark a single tile coordinate as dirty.
    pub fn mark(&mut self, tx: i32, ty: i32) {
        self.tiles.insert((tx, ty));
    }

    /// Mark all tiles overlapping a pixel-coordinate rectangle.
    pub fn mark_rect(&mut self, x: i32, y: i32, w: u32, h: u32) {
        let (tx0, ty0) = TileGrid::tile_coords(x, y);
        let (tx1, ty1) = TileGrid::tile_coords(
            x + w as i32 - 1,
            y + h as i32 - 1,
        );
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                self.tiles.insert((tx, ty));
            }
        }
    }

    pub fn clear(&mut self) {
        self.tiles.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Returns the bounding box in tile coordinates: (min_tx, min_ty, max_tx, max_ty).
    pub fn bounding_rect(&self) -> Option<(i32, i32, i32, i32)> {
        let mut iter = self.tiles.iter();
        let &(first_x, first_y) = iter.next()?;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (first_x, first_y, first_x, first_y);
        for &(tx, ty) in iter {
            min_x = min_x.min(tx);
            min_y = min_y.min(ty);
            max_x = max_x.max(tx);
            max_y = max_y.max(ty);
        }
        Some((min_x, min_y, max_x, max_y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_and_bounding_rect() {
        let mut dirty = DirtyRegion::new();
        assert!(dirty.is_empty());
        assert_eq!(dirty.bounding_rect(), None);

        dirty.mark(0, 0);
        dirty.mark(2, 3);
        assert_eq!(dirty.bounding_rect(), Some((0, 0, 2, 3)));

        dirty.clear();
        assert!(dirty.is_empty());
    }

    #[test]
    fn mark_rect_covers_tiles() {
        let mut dirty = DirtyRegion::new();
        // A rect from pixel (0,0) size 128x128 should cover tiles (0,0) to (1,1)
        dirty.mark_rect(0, 0, 128, 128);
        assert!(dirty.tiles.contains(&(0, 0)));
        assert!(dirty.tiles.contains(&(1, 0)));
        assert!(dirty.tiles.contains(&(0, 1)));
        assert!(dirty.tiles.contains(&(1, 1)));
        assert_eq!(dirty.tiles.len(), 4);
    }

    #[test]
    fn mark_rect_negative_coords() {
        let mut dirty = DirtyRegion::new();
        dirty.mark_rect(-10, -10, 20, 20);
        // -10 to 9 in x and y
        // tile_coords(-10) = -1, tile_coords(9) = 0
        assert!(dirty.tiles.contains(&(-1, -1)));
        assert!(dirty.tiles.contains(&(0, 0)));
        assert!(dirty.tiles.contains(&(-1, 0)));
        assert!(dirty.tiles.contains(&(0, -1)));
    }
}
