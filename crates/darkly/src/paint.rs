//! PaintTarget — shared pixel write abstraction with selection masking.
//!
//! All paint operations (brush, eraser, fill, gradient) write through this
//! abstraction. Selection masking is applied internally — callers never see
//! the selection.

use crate::dirty::DirtyRegion;
use crate::tile::{AlphaMask, TileGrid, TILE_SIZE};

pub struct PaintTarget<'a> {
    pub tiles: &'a mut TileGrid,
    pub dirty: &'a mut DirtyRegion,
    selection: Option<&'a AlphaMask>,
}

impl<'a> PaintTarget<'a> {
    pub fn new(
        tiles: &'a mut TileGrid,
        dirty: &'a mut DirtyRegion,
        selection: Option<&'a AlphaMask>,
    ) -> Self {
        PaintTarget { tiles, dirty, selection }
    }

    /// Selection coverage at a pixel. Returns 1.0 if no active selection.
    fn coverage(&self, px: i32, py: i32) -> f32 {
        match self.selection {
            Some(sel) => sel.sample(px, py),
            None => 1.0,
        }
    }

    /// Alpha-composite `src` onto the pixel at (px, py) using normal (over) blending.
    /// Selection mask modulates source alpha automatically.
    pub fn composite(&mut self, px: i32, py: i32, src: [u8; 4]) {
        let cov = self.coverage(px, py);
        if cov <= 0.0 {
            return;
        }

        let tile_size = TILE_SIZE as i32;
        let (tx, ty) = TileGrid::tile_coords_for_pixel(px, py);
        let lx = (px - tx * tile_size) as usize;
        let ly = (py - ty * tile_size) as usize;

        let tile = self.tiles.get_or_create(tx, ty);
        let data = tile.write();
        let dst = data.pixel_mut(lx, ly);

        let src_a = src[3] as f32 / 255.0 * cov;
        let dst_a = dst[3] as f32 / 255.0;
        let out_a = src_a + dst_a * (1.0 - src_a);
        if out_a > 0.0 {
            for c in 0..3 {
                let src_c = src[c] as f32 / 255.0;
                let dst_c = dst[c] as f32 / 255.0;
                let out_c = (src_c * src_a + dst_c * dst_a * (1.0 - src_a)) / out_a;
                dst[c] = (out_c * 255.0).round() as u8;
            }
            dst[3] = (out_a * 255.0).round() as u8;
        }

        self.dirty.mark(tx, ty);
    }

    /// Erase (blend toward transparent) at (px, py).
    /// Selection mask modulates erase strength.
    pub fn erase(&mut self, px: i32, py: i32, strength: f32) {
        let cov = self.coverage(px, py);
        if cov <= 0.0 {
            return;
        }

        let tile_size = TILE_SIZE as i32;
        let (tx, ty) = TileGrid::tile_coords_for_pixel(px, py);
        let lx = (px - tx * tile_size) as usize;
        let ly = (py - ty * tile_size) as usize;

        let tile = self.tiles.get_or_create(tx, ty);
        let data = tile.write();
        let dst = data.pixel_mut(lx, ly);

        let factor = 1.0 - strength * cov;
        for c in 0..4 {
            dst[c] = (dst[c] as f32 * factor).round() as u8;
        }

        self.dirty.mark(tx, ty);
    }

    /// Replace pixel at (px, py) with color.
    /// Selection mask modulates via alpha blend (coverage=1 → full replace, 0 → no change).
    pub fn replace(&mut self, px: i32, py: i32, color: [u8; 4]) {
        let cov = self.coverage(px, py);
        if cov <= 0.0 {
            return;
        }

        let tile_size = TILE_SIZE as i32;
        let (tx, ty) = TileGrid::tile_coords_for_pixel(px, py);
        let lx = (px - tx * tile_size) as usize;
        let ly = (py - ty * tile_size) as usize;

        let tile = self.tiles.get_or_create(tx, ty);
        let data = tile.write();
        let dst = data.pixel_mut(lx, ly);

        if cov >= 1.0 {
            dst.copy_from_slice(&color);
        } else {
            for c in 0..4 {
                let d = dst[c] as f32;
                let s = color[c] as f32;
                dst[c] = (d + (s - d) * cov).round() as u8;
            }
        }

        self.dirty.mark(tx, ty);
    }
}

/// Paint target for layer masks. The mask is a single-channel f32 surface
/// where white (1.0) = reveal and black (0.0) = hide. Uses `get_or_create_full()`
/// so new tiles default to 1.0 (GIMP/Krita convention).
pub struct MaskPaintTarget<'a> {
    pub mask: &'a mut AlphaMask,
    pub dirty: &'a mut DirtyRegion,
    selection: Option<&'a AlphaMask>,
}

impl<'a> MaskPaintTarget<'a> {
    pub fn new(
        mask: &'a mut AlphaMask,
        dirty: &'a mut DirtyRegion,
        selection: Option<&'a AlphaMask>,
    ) -> Self {
        MaskPaintTarget { mask, dirty, selection }
    }

    fn coverage(&self, px: i32, py: i32) -> f32 {
        match self.selection {
            Some(sel) => sel.sample(px, py),
            None => 1.0,
        }
    }

    /// Paint toward `value` at (px, py) with the given strength.
    /// strength=1.0 fully replaces with `value`.
    pub fn paint(&mut self, px: i32, py: i32, value: f32, strength: f32) {
        let cov = self.coverage(px, py);
        if cov <= 0.0 {
            return;
        }

        let tile_size = TILE_SIZE as i32;
        let (tx, ty) = AlphaMask::tile_coords_for_pixel(px, py);
        let lx = (px - tx * tile_size) as usize;
        let ly = (py - ty * tile_size) as usize;

        let tile = self.mask.get_or_create_full(tx, ty);
        let data = tile.write();
        let current = data.get(lx, ly);
        let factor = strength * cov;
        data.set(lx, ly, current + (value - current) * factor);

        self.dirty.mark(tx, ty);
    }

    /// Erase (blend toward 0.0 = hide) at (px, py).
    pub fn erase(&mut self, px: i32, py: i32, strength: f32) {
        self.paint(px, py, 0.0, strength);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirty::DirtyRegion;
    use crate::tile::{AlphaMask, TileGrid};

    #[test]
    fn composite_without_selection() {
        let mut tiles = TileGrid::new();
        let mut dirty = DirtyRegion::new();
        let mut target = PaintTarget::new(&mut tiles, &mut dirty, None);

        target.composite(5, 5, [255, 0, 0, 255]);

        let tile = tiles.get(0, 0).unwrap();
        assert_eq!(tile.data().pixel(5, 5), &[255, 0, 0, 255]);
    }

    #[test]
    fn composite_masked_by_selection() {
        let mut tiles = TileGrid::new();
        let mut dirty = DirtyRegion::new();
        let mut sel = AlphaMask::new();
        // Only allow pixel (5,5), block pixel (10,10)
        sel.get_or_create(0, 0).write().set(5, 5, 1.0);

        {
            let mut target = PaintTarget::new(&mut tiles, &mut dirty, Some(&sel));
            target.composite(5, 5, [255, 0, 0, 255]);
            target.composite(10, 10, [0, 255, 0, 255]);
        }

        let tile = tiles.get(0, 0).unwrap();
        assert_eq!(tile.data().pixel(5, 5), &[255, 0, 0, 255]);
        assert_eq!(tile.data().pixel(10, 10), &[0, 0, 0, 0]);
    }

    #[test]
    fn erase_masked_by_selection() {
        let mut tiles = TileGrid::new();
        // Pre-fill a pixel
        tiles.get_or_create(0, 0).write().pixel_mut(5, 5).copy_from_slice(&[255, 0, 0, 255]);
        tiles.get_or_create(0, 0).write().pixel_mut(10, 10).copy_from_slice(&[0, 255, 0, 255]);

        let mut dirty = DirtyRegion::new();
        let mut sel = AlphaMask::new();
        sel.get_or_create(0, 0).write().set(5, 5, 1.0);
        // (10,10) has 0.0 coverage → erase should be blocked

        {
            let mut target = PaintTarget::new(&mut tiles, &mut dirty, Some(&sel));
            target.erase(5, 5, 1.0);
            target.erase(10, 10, 1.0);
        }

        let tile = tiles.get(0, 0).unwrap();
        assert_eq!(tile.data().pixel(5, 5), &[0, 0, 0, 0]);
        assert_eq!(tile.data().pixel(10, 10), &[0, 255, 0, 255]);
    }

    #[test]
    fn replace_with_partial_coverage() {
        let mut tiles = TileGrid::new();
        tiles.get_or_create(0, 0).write().pixel_mut(5, 5).copy_from_slice(&[0, 0, 0, 255]);

        let mut dirty = DirtyRegion::new();
        let mut sel = AlphaMask::new();
        sel.get_or_create(0, 0).write().set(5, 5, 0.5);

        {
            let mut target = PaintTarget::new(&mut tiles, &mut dirty, Some(&sel));
            target.replace(5, 5, [255, 255, 255, 255]);
        }

        let tile = tiles.get(0, 0).unwrap();
        let px = tile.data().pixel(5, 5);
        // 50% blend: each channel ≈ 128
        assert!((px[0] as i32 - 128).abs() <= 1);
        assert!((px[3] as i32 - 255).abs() <= 0);
    }
}
