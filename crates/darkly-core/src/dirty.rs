use crate::tile::TileGrid;
use std::collections::HashSet;

/// Tracks which tiles have been modified and need GPU upload.
#[derive(Clone, Default)]
pub struct DirtyRegion {
    tiles: HashSet<(i32, i32)>,
}

impl DirtyRegion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a single tile as dirty.
    pub fn mark(&mut self, tx: i32, ty: i32) {
        self.tiles.insert((tx, ty));
    }

    /// Mark all tiles that overlap a pixel rectangle.
    pub fn mark_rect(&mut self, x: u32, y: u32, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let (tx0, ty0) = TileGrid::tile_coords_for_pixel(x as i32, y as i32);
        let (tx1, ty1) =
            TileGrid::tile_coords_for_pixel((x + w - 1) as i32, (y + h - 1) as i32);
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                self.tiles.insert((tx, ty));
            }
        }
    }

    /// Clear all dirty marks.
    pub fn clear(&mut self) {
        self.tiles.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Iterate over dirty tile coordinates.
    pub fn iter(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.tiles.iter().copied()
    }

    /// Bounding rectangle in tile coordinates, or None if empty.
    pub fn bounding_rect(&self) -> Option<(i32, i32, i32, i32)> {
        if self.tiles.is_empty() {
            return None;
        }
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for &(tx, ty) in &self.tiles {
            min_x = min_x.min(tx);
            min_y = min_y.min(ty);
            max_x = max_x.max(tx);
            max_y = max_y.max(ty);
        }
        Some((min_x, min_y, max_x, max_y))
    }

    /// Number of dirty tiles.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::TILE_SIZE;

    #[test]
    fn mark_rect_single_tile() {
        let mut d = DirtyRegion::new();
        d.mark_rect(0, 0, 1, 1);
        assert_eq!(d.len(), 1);
        assert!(d.iter().any(|(tx, ty)| tx == 0 && ty == 0));
    }

    #[test]
    fn mark_rect_spanning_tiles() {
        let mut d = DirtyRegion::new();
        // Spans from tile (0,0) to tile (1,1)
        d.mark_rect(32, 32, TILE_SIZE as u32, TILE_SIZE as u32);
        assert_eq!(d.len(), 4); // (0,0), (1,0), (0,1), (1,1)
    }

    #[test]
    fn bounding_rect() {
        let mut d = DirtyRegion::new();
        d.mark(2, 3);
        d.mark(-1, 5);
        assert_eq!(d.bounding_rect(), Some((-1, 3, 2, 5)));
    }
}
