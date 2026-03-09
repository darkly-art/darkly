//! AlphaMask operations — boolean ops, shape filling, and utilities.
//!
//! `AlphaMask` is a `TileStore<AlphaF32>` (single-channel f32 per pixel).
//! It's used for selections, layer masks, and future mask-like concepts.
//! The storage, COW, and transaction/memento infrastructure are inherited
//! from the generic `TileStore<F>` in `tile.rs`.

use crate::tile::{AlphaMask, AlphaF32, Tile, TILE_SIZE};

// ---------------------------------------------------------------------------
// Boolean operations
// ---------------------------------------------------------------------------

impl AlphaMask {
    /// Add another mask: `self = min(1.0, self + other)` per pixel.
    pub fn boolean_add(&mut self, other: &AlphaMask) {
        for ((tx, ty), other_tile) in other.iter() {
            let tile = self.get_or_create(tx, ty);
            let dst = tile.write();
            let src = other_tile.data();
            for i in 0..dst.0.len() {
                dst.0[i] = (dst.0[i] + src.0[i]).min(1.0);
            }
        }
    }

    /// Subtract another mask: `self = max(0.0, self - other)` per pixel.
    pub fn boolean_subtract(&mut self, other: &AlphaMask) {
        for ((tx, ty), other_tile) in other.iter() {
            if let Some(tile) = self.get_mut(tx, ty) {
                let dst = tile.write();
                let src = other_tile.data();
                for i in 0..dst.0.len() {
                    dst.0[i] = (dst.0[i] - src.0[i]).max(0.0);
                }
            }
            // If self has no tile at this position, subtracting from 0 stays 0.
        }
    }

    /// Intersect with another mask: `self = min(self, other)` per pixel.
    /// Tiles in self that don't exist in other become zero (fully deselected).
    pub fn boolean_intersect(&mut self, other: &AlphaMask) {
        // For each tile in self: if other has it, min(); if not, zero it out.
        let self_keys: Vec<(i32, i32)> = self.iter().map(|(k, _)| k).collect();
        for (tx, ty) in self_keys {
            match other.get(tx, ty) {
                Some(other_tile) => {
                    let tile = self.get_or_create(tx, ty);
                    let dst = tile.write();
                    let src = other_tile.data();
                    for i in 0..dst.0.len() {
                        dst.0[i] = dst.0[i].min(src.0[i]);
                    }
                }
                None => {
                    // Other has no tile here → intersection is zero.
                    let tile = self.get_or_create(tx, ty);
                    let dst = tile.write();
                    dst.0.fill(0.0);
                }
            }
        }
    }

    /// Get a mutable reference to an existing tile (without creating).
    fn get_mut(&mut self, tx: i32, ty: i32) -> Option<&mut Tile<AlphaF32>> {
        // Use get_or_create's recording path only if tile exists.
        if self.get(tx, ty).is_some() {
            Some(self.get_or_create(tx, ty))
        } else {
            None
        }
    }

    /// Fill a rectangle with a constant value.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, value: f32) {
        if w <= 0 || h <= 0 {
            return;
        }
        let x_max = x + w - 1;
        let y_max = y + h - 1;

        let tile_size = TILE_SIZE as i32;
        let (tx_min, ty_min) = Self::tile_coords_for_pixel(x, y);
        let (tx_max, ty_max) = Self::tile_coords_for_pixel(x_max, y_max);

        for ty in ty_min..=ty_max {
            for tx in tx_min..=tx_max {
                let tile_px_x = tx * tile_size;
                let tile_px_y = ty * tile_size;

                let lx_start = (x - tile_px_x).max(0) as usize;
                let lx_end = ((x_max + 1 - tile_px_x).min(tile_size) as usize).min(TILE_SIZE);
                let ly_start = (y - tile_px_y).max(0) as usize;
                let ly_end = ((y_max + 1 - tile_px_y).min(tile_size) as usize).min(TILE_SIZE);

                let tile = self.get_or_create(tx, ty);
                let data = tile.write();
                for ly in ly_start..ly_end {
                    for lx in lx_start..lx_end {
                        data.set(lx, ly, value);
                    }
                }
            }
        }
    }

    /// Fill an axis-aligned ellipse with a constant value.
    pub fn fill_ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, value: f32) {
        if rx <= 0.0 || ry <= 0.0 {
            return;
        }
        let x_min = (cx - rx).floor() as i32;
        let x_max = (cx + rx).ceil() as i32;
        let y_min = (cy - ry).floor() as i32;
        let y_max = (cy + ry).ceil() as i32;

        let tile_size = TILE_SIZE as i32;
        let (tx_min, ty_min) = Self::tile_coords_for_pixel(x_min, y_min);
        let (tx_max, ty_max) = Self::tile_coords_for_pixel(x_max, y_max);

        let inv_rx2 = 1.0 / (rx * rx);
        let inv_ry2 = 1.0 / (ry * ry);

        for ty in ty_min..=ty_max {
            for tx in tx_min..=tx_max {
                let tile_px_x = tx * tile_size;
                let tile_px_y = ty * tile_size;

                let lx_start = (x_min - tile_px_x).max(0) as usize;
                let lx_end = ((x_max - tile_px_x).min(tile_size) as usize).min(TILE_SIZE);
                let ly_start = (y_min - tile_px_y).max(0) as usize;
                let ly_end = ((y_max - tile_px_y).min(tile_size) as usize).min(TILE_SIZE);

                let tile = self.get_or_create(tx, ty);
                let data = tile.write();
                for ly in ly_start..ly_end {
                    for lx in lx_start..lx_end {
                        let px = (tile_px_x + lx as i32) as f32 + 0.5;
                        let py = (tile_px_y + ly as i32) as f32 + 0.5;
                        let dx = px - cx;
                        let dy = py - cy;
                        if dx * dx * inv_rx2 + dy * dy * inv_ry2 <= 1.0 {
                            data.set(lx, ly, value);
                        }
                    }
                }
            }
        }
    }

    /// Fill a closed polygon with a constant value using scanline rasterization.
    /// Points define vertices in pixel coordinates. The polygon is implicitly closed.
    pub fn fill_polygon(&mut self, points: &[(f32, f32)], value: f32) {
        if points.len() < 3 {
            return;
        }

        // Compute bounding box.
        let mut x_min = f32::MAX;
        let mut x_max = f32::MIN;
        let mut y_min = f32::MAX;
        let mut y_max = f32::MIN;
        for &(px, py) in points {
            x_min = x_min.min(px);
            x_max = x_max.max(px);
            y_min = y_min.min(py);
            y_max = y_max.max(py);
        }

        let iy_min = y_min.floor() as i32;
        let iy_max = y_max.ceil() as i32;

        let tile_size = TILE_SIZE as i32;

        // Scanline fill: for each row, find intersections with polygon edges.
        for iy in iy_min..iy_max {
            let scan_y = iy as f32 + 0.5;
            let mut intersections = Vec::new();

            let n = points.len();
            for i in 0..n {
                let (x0, y0) = points[i];
                let (x1, y1) = points[(i + 1) % n];

                // Skip horizontal edges.
                if (y0 - y1).abs() < 1e-6 {
                    continue;
                }

                let (y_lo, y_hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                if scan_y < y_lo || scan_y >= y_hi {
                    continue;
                }

                let t = (scan_y - y0) / (y1 - y0);
                let ix = x0 + t * (x1 - x0);
                intersections.push(ix);
            }

            intersections.sort_by(|a, b| a.partial_cmp(b).unwrap());

            // Fill between pairs of intersections.
            for pair in intersections.chunks(2) {
                if pair.len() < 2 {
                    break;
                }
                let fill_x_min = pair[0].floor() as i32;
                let fill_x_max = pair[1].ceil() as i32;

                let (tx_min, _) = Self::tile_coords_for_pixel(fill_x_min, iy);
                let (tx_max, _) = Self::tile_coords_for_pixel(fill_x_max, iy);
                let (_, ty) = Self::tile_coords_for_pixel(0, iy);

                for tx in tx_min..=tx_max {
                    let tile_px_x = tx * tile_size;
                    let tile_px_y = ty * tile_size;

                    let lx_start = (fill_x_min - tile_px_x).max(0) as usize;
                    let lx_end = ((fill_x_max - tile_px_x).min(tile_size) as usize).min(TILE_SIZE);
                    let ly = (iy - tile_px_y) as usize;
                    if ly >= TILE_SIZE {
                        continue;
                    }

                    let tile = self.get_or_create(tx, ty);
                    let data = tile.write();
                    for lx in lx_start..lx_end {
                        let px = (tile_px_x + lx as i32) as f32 + 0.5;
                        if px >= pair[0] && px <= pair[1] {
                            data.set(lx, ly, value);
                        }
                    }
                }
            }
        }
    }

    /// Clear the entire mask (remove all tiles).
    pub fn clear(&mut self) {
        *self = AlphaMask::new();
    }

    /// Invert all existing tiles: `value = 1.0 - value`.
    pub fn invert(&mut self) {
        let keys: Vec<(i32, i32)> = self.iter().map(|(k, _)| k).collect();
        for (tx, ty) in keys {
            let tile = self.get_or_create(tx, ty);
            let data = tile.write();
            for v in data.0.iter_mut() {
                *v = 1.0 - *v;
            }
        }
    }

    /// Bounding rect of non-empty tiles in tile coordinates: (tx_min, ty_min, tx_max, ty_max).
    pub fn bounding_rect(&self) -> Option<(i32, i32, i32, i32)> {
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        let mut any = false;

        for ((tx, ty), _) in self.iter() {
            min_x = min_x.min(tx);
            min_y = min_y.min(ty);
            max_x = max_x.max(tx);
            max_y = max_y.max(ty);
            any = true;
        }

        if any {
            Some((min_x, min_y, max_x, max_y))
        } else {
            None
        }
    }

    /// Sample the mask value at a pixel coordinate. Returns 0.0 if no tile exists.
    pub fn sample(&self, px: i32, py: i32) -> f32 {
        let tile_size = TILE_SIZE as i32;
        let (tx, ty) = Self::tile_coords_for_pixel(px, py);
        match self.get(tx, ty) {
            Some(tile) => {
                let lx = (px - tx * tile_size) as usize;
                let ly = (py - ty * tile_size) as usize;
                tile.data().get(lx, ly)
            }
            None => 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::AlphaMask;

    #[test]
    fn fill_rect_basic() {
        let mut mask = AlphaMask::new();
        mask.fill_rect(10, 10, 20, 20, 1.0);

        assert_eq!(mask.sample(15, 15), 1.0);
        assert_eq!(mask.sample(9, 15), 0.0);
        assert_eq!(mask.sample(30, 15), 0.0);
        assert_eq!(mask.sample(15, 9), 0.0);
        assert_eq!(mask.sample(15, 30), 0.0);
        // Edge pixels: 10..29 inclusive
        assert_eq!(mask.sample(10, 10), 1.0);
        assert_eq!(mask.sample(29, 29), 1.0);
    }

    #[test]
    fn fill_rect_spanning_tiles() {
        let mut mask = AlphaMask::new();
        // Span across tile boundary (TILE_SIZE=64)
        mask.fill_rect(60, 60, 10, 10, 0.5);

        assert_eq!(mask.sample(63, 63), 0.5); // tile (0,0)
        assert_eq!(mask.sample(64, 64), 0.5); // tile (1,1)
        assert_eq!(mask.sample(69, 69), 0.5);
        assert_eq!(mask.sample(59, 63), 0.0);
    }

    #[test]
    fn fill_ellipse_basic() {
        let mut mask = AlphaMask::new();
        mask.fill_ellipse(32.0, 32.0, 10.0, 10.0, 1.0);

        assert_eq!(mask.sample(32, 32), 1.0); // center
        assert_eq!(mask.sample(32, 22), 1.0); // near top edge
        assert_eq!(mask.sample(32, 20), 0.0); // outside top
    }

    #[test]
    fn fill_polygon_triangle() {
        let mut mask = AlphaMask::new();
        let points = vec![(10.0, 10.0), (30.0, 10.0), (20.0, 30.0)];
        mask.fill_polygon(&points, 1.0);

        // Center of triangle should be filled
        assert_eq!(mask.sample(20, 15), 1.0);
        // Outside should be empty
        assert_eq!(mask.sample(5, 15), 0.0);
        assert_eq!(mask.sample(35, 15), 0.0);
    }

    #[test]
    fn boolean_add() {
        let mut a = AlphaMask::new();
        let mut b = AlphaMask::new();

        a.fill_rect(0, 0, 10, 10, 0.5);
        b.fill_rect(5, 0, 10, 10, 0.5);

        a.boolean_add(&b);

        // Overlap region should be 1.0 (clamped)
        assert_eq!(a.sample(7, 5), 1.0);
        // a-only region
        assert_eq!(a.sample(2, 5), 0.5);
        // b-only region
        assert_eq!(a.sample(12, 5), 0.5);
    }

    #[test]
    fn boolean_subtract() {
        let mut a = AlphaMask::new();
        let mut b = AlphaMask::new();

        a.fill_rect(0, 0, 20, 10, 1.0);
        b.fill_rect(10, 0, 20, 10, 1.0);

        a.boolean_subtract(&b);

        // Left half should remain
        assert_eq!(a.sample(5, 5), 1.0);
        // Overlap region should be 0
        assert_eq!(a.sample(15, 5), 0.0);
    }

    #[test]
    fn boolean_intersect() {
        let mut a = AlphaMask::new();
        let mut b = AlphaMask::new();

        a.fill_rect(0, 0, 20, 10, 1.0);
        b.fill_rect(10, 0, 20, 10, 0.5);

        a.boolean_intersect(&b);

        // a-only region should be 0 (b has no tile there)
        assert_eq!(a.sample(5, 5), 0.0);
        // Overlap: min(1.0, 0.5) = 0.5
        assert_eq!(a.sample(15, 5), 0.5);
    }

    #[test]
    fn invert() {
        let mut mask = AlphaMask::new();
        mask.fill_rect(0, 0, 10, 10, 0.75);
        mask.invert();

        assert!((mask.sample(5, 5) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn clear() {
        let mut mask = AlphaMask::new();
        mask.fill_rect(0, 0, 100, 100, 1.0);
        assert!(!mask.is_empty());

        mask.clear();
        assert!(mask.is_empty());
        assert_eq!(mask.sample(50, 50), 0.0);
    }

    #[test]
    fn bounding_rect() {
        let mut mask = AlphaMask::new();
        assert!(mask.bounding_rect().is_none());

        mask.fill_rect(100, 200, 10, 10, 1.0);
        let (tx_min, ty_min, tx_max, ty_max) = mask.bounding_rect().unwrap();
        assert_eq!(tx_min, 1); // 100 / 64 = 1
        assert_eq!(ty_min, 3); // 200 / 64 = 3
        assert_eq!(tx_max, 1);
        assert_eq!(ty_max, 3);
    }
}
