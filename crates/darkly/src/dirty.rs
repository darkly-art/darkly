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

/// Compute the union of all dirty regions as a pixel-coordinate rect (x, y, w, h),
/// clamped to canvas bounds and aligned to tile boundaries.
/// Returns None if all regions are empty.
pub fn dirty_pixel_rect<'a>(
    regions: impl Iterator<Item = &'a DirtyRegion>,
    canvas_width: u32,
    canvas_height: u32,
) -> Option<(u32, u32, u32, u32)> {
    use crate::tile::TILE_SIZE;

    let mut min_tx = i32::MAX;
    let mut min_ty = i32::MAX;
    let mut max_tx = i32::MIN;
    let mut max_ty = i32::MIN;
    let mut found = false;

    for d in regions {
        if let Some((x0, y0, x1, y1)) = d.bounding_rect() {
            min_tx = min_tx.min(x0);
            min_ty = min_ty.min(y0);
            max_tx = max_tx.max(x1);
            max_ty = max_ty.max(y1);
            found = true;
        }
    }

    if !found {
        return None;
    }

    let ts = TILE_SIZE as u32;
    let px = (min_tx.max(0) as u32) * ts;
    let py = (min_ty.max(0) as u32) * ts;
    let px2 = ((max_tx + 1).max(0) as u32 * ts).min(canvas_width);
    let py2 = ((max_ty + 1).max(0) as u32 * ts).min(canvas_height);

    if px < px2 && py < py2 {
        Some((px, py, px2 - px, py2 - py))
    } else {
        None
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

    #[test]
    fn dirty_pixel_rect_single_tile() {
        let ts = TILE_SIZE as u32;
        let mut d = DirtyRegion::new();
        d.mark(1, 2);
        let rect = super::dirty_pixel_rect(std::iter::once(&d), 1920, 1080);
        assert_eq!(rect, Some((ts, 2 * ts, ts, ts.min(1080 - 2 * ts))));
    }

    #[test]
    fn dirty_pixel_rect_multiple_tiles() {
        let ts = TILE_SIZE as u32;
        let mut d = DirtyRegion::new();
        d.mark(0, 0);
        d.mark(2, 1);
        let rect = super::dirty_pixel_rect(std::iter::once(&d), 1920, 1080);
        assert_eq!(rect, Some((0, 0, (3 * ts).min(1920), (2 * ts).min(1080))));
    }

    #[test]
    fn dirty_pixel_rect_union_across_regions() {
        let ts = TILE_SIZE as u32;
        let mut d1 = DirtyRegion::new();
        d1.mark(0, 0);
        let mut d2 = DirtyRegion::new();
        d2.mark(5, 3);
        let regions = [d1, d2];
        let rect = super::dirty_pixel_rect(regions.iter(), 1920, 1080);
        assert_eq!(rect, Some((0, 0, (6 * ts).min(1920), (4 * ts).min(1080))));
    }

    #[test]
    fn dirty_pixel_rect_clamps_to_canvas() {
        let ts = TILE_SIZE as u32;
        // Pick a tile whose pixel range exceeds canvas height
        let tx = (1920 / ts).saturating_sub(1);
        let ty = 1080 / ts; // first tile row that extends past 1080
        let mut d = DirtyRegion::new();
        d.mark(tx as i32, ty as i32);
        let rect = super::dirty_pixel_rect(std::iter::once(&d), 1920, 1080);
        let px = tx * ts;
        let py = ty * ts;
        let expected_w = ((tx + 1) * ts).min(1920) - px;
        let expected_h = ((ty + 1) * ts).min(1080) - py;
        if py < 1080 && px < 1920 {
            assert_eq!(rect, Some((px, py, expected_w, expected_h)));
        } else {
            // Tile is entirely outside the canvas
            assert_eq!(rect, None);
        }
    }

    #[test]
    fn dirty_pixel_rect_empty_regions() {
        let d1 = DirtyRegion::new();
        let d2 = DirtyRegion::new();
        let regions = [d1, d2];
        let rect = super::dirty_pixel_rect(regions.iter(), 1920, 1080);
        assert_eq!(rect, None);
    }

    #[test]
    fn dirty_pixel_rect_negative_tiles_clamped() {
        let ts = TILE_SIZE as u32;
        let mut d = DirtyRegion::new();
        d.mark(-1, -1);
        d.mark(1, 1);
        let rect = super::dirty_pixel_rect(std::iter::once(&d), 1920, 1080);
        assert_eq!(rect, Some((0, 0, (2 * ts).min(1920), (2 * ts).min(1080))));
    }
}
