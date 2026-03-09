//! AlphaMask operations — boolean ops and utilities.
//!
//! `AlphaMask` is a `TileStore<AlphaF32>` (single-channel f32 per pixel).
//! It's used for selections, layer masks, and future mask-like concepts.
//! The storage, COW, and transaction/memento infrastructure are inherited
//! from the generic `TileStore<F>` in `tile.rs`.
//!
//! Shape rasterization (rect, ellipse, polygon) does NOT live here.
//! Selection tools use shared rasterization infrastructure to write into
//! masks, just as paint tools write into layers. The mask is a transparent
//! paint target, not a shape-aware object.

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
// Test helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
impl AlphaMask {
    /// Fill a rectangular region with a constant value. Test-only helper.
    fn fill_rect_test(&mut self, x: i32, y: i32, w: i32, h: i32, value: f32) {
        let tile_size = TILE_SIZE as i32;
        for py in y..y + h {
            for px in x..x + w {
                let (tx, ty) = Self::tile_coords_for_pixel(px, py);
                let tile = self.get_or_create(tx, ty);
                let lx = (px - tx * tile_size) as usize;
                let ly = (py - ty * tile_size) as usize;
                tile.write().set(lx, ly, value);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::tile::AlphaMask;

    #[test]
    fn boolean_add() {
        let mut a = AlphaMask::new();
        let mut b = AlphaMask::new();

        a.fill_rect_test(0, 0, 10, 10, 0.5);
        b.fill_rect_test(5, 0, 10, 10, 0.5);

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

        a.fill_rect_test(0, 0, 20, 10, 1.0);
        b.fill_rect_test(10, 0, 20, 10, 1.0);

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

        a.fill_rect_test(0, 0, 20, 10, 1.0);
        b.fill_rect_test(10, 0, 20, 10, 0.5);

        a.boolean_intersect(&b);

        // a-only region should be 0 (b has no tile there)
        assert_eq!(a.sample(5, 5), 0.0);
        // Overlap: min(1.0, 0.5) = 0.5
        assert_eq!(a.sample(15, 5), 0.5);
    }

    #[test]
    fn invert() {
        let mut mask = AlphaMask::new();
        mask.fill_rect_test(0, 0, 10, 10, 0.75);
        mask.invert();

        assert!((mask.sample(5, 5) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn clear() {
        let mut mask = AlphaMask::new();
        mask.fill_rect_test(0, 0, 64, 64, 1.0);
        assert!(!mask.is_empty());

        mask.clear();
        assert!(mask.is_empty());
        assert_eq!(mask.sample(5, 5), 0.0);
    }

    #[test]
    fn bounding_rect() {
        let mut mask = AlphaMask::new();
        assert!(mask.bounding_rect().is_none());

        // Write a single pixel at (100, 200) to create a tile at (1, 3)
        let (tx, ty) = AlphaMask::tile_coords_for_pixel(100, 200);
        mask.get_or_create(tx, ty).write().set(0, 0, 1.0);

        let (tx_min, ty_min, tx_max, ty_max) = mask.bounding_rect().unwrap();
        assert_eq!(tx_min, 1); // 100 / 64 = 1
        assert_eq!(ty_min, 3); // 200 / 64 = 3
        assert_eq!(tx_max, 1);
        assert_eq!(ty_max, 3);
    }

    #[test]
    fn sample_empty() {
        let mask = AlphaMask::new();
        assert_eq!(mask.sample(0, 0), 0.0);
        assert_eq!(mask.sample(1000, 1000), 0.0);
    }
}
