//! PaintSurface — unified trait for painting on tiled pixel surfaces.
//!
//! Both RGBA layers and f32 alpha masks implement the same trait. Paint
//! algorithms (brush, eraser, fill, gradient) are generic free functions
//! that work on any `impl PaintSurface`. Selection masking is applied
//! internally — callers never see the selection.

use crate::dirty::DirtyRegion;
use crate::layer::LayerId;
use crate::tile::{AlphaF32, AlphaF32Data, AlphaMask, Memento, Rgba, RgbaData, TileGrid, TILE_SIZE};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// PaintSurface trait
// ---------------------------------------------------------------------------

/// A tiled pixel surface that can be read from and written to.
///
/// Write methods accept RGBA colors; implementations convert internally
/// (no-op for RGBA layers, luminance conversion for masks).
///
/// Read methods expose the native pixel format for algorithms like flood
/// fill that need tile-level batched access for performance.
pub trait PaintSurface {
    // --- Native pixel format (for reading / comparison) ---

    /// The seed value type used for flood fill comparison.
    type Seed: Copy;
    /// The tile data type for direct tile-level access.
    type TileData;

    /// Read the seed value at a pixel (used for the flood fill seed point).
    fn read_seed(&self, x: i32, y: i32) -> Self::Seed;
    /// Get a reference to the tile data at tile coords (tx, ty).
    fn get_tile_data(&self, tx: i32, ty: i32) -> Option<&Self::TileData>;
    /// Check if a pixel in tile data matches the seed within tolerance.
    fn pixel_matches(data: &Self::TileData, lx: usize, ly: usize, seed: &Self::Seed, tol: i16) -> bool;
    /// Check if an empty (missing) tile's pixel matches the seed within tolerance.
    fn empty_matches(seed: &Self::Seed, tol: i16) -> bool;

    // --- Write (RGBA input, conversion happens internally) ---

    /// Alpha-composite `src` onto the surface at (px, py).
    fn composite(&mut self, px: i32, py: i32, src: [u8; 4]);
    /// Blend toward transparent/zero at (px, py).
    fn erase(&mut self, px: i32, py: i32, strength: f32);
    /// Replace pixel at (px, py) with `color`, modulated by selection coverage.
    fn replace(&mut self, px: i32, py: i32, color: [u8; 4]);
}

// ---------------------------------------------------------------------------
// PaintTarget — RGBA layer surface
// ---------------------------------------------------------------------------

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
}

impl PaintSurface for PaintTarget<'_> {
    type Seed = [u8; 4];
    type TileData = RgbaData;

    fn read_seed(&self, x: i32, y: i32) -> [u8; 4] {
        let ts = TILE_SIZE as i32;
        let (tx, ty) = TileGrid::tile_coords_for_pixel(x, y);
        match self.tiles.get(tx, ty) {
            Some(t) => *t.data().pixel((x - tx * ts) as usize, (y - ty * ts) as usize),
            None => [0, 0, 0, 0],
        }
    }

    fn get_tile_data(&self, tx: i32, ty: i32) -> Option<&RgbaData> {
        self.tiles.get(tx, ty).map(|t| t.data())
    }

    fn pixel_matches(data: &RgbaData, lx: usize, ly: usize, seed: &[u8; 4], tol: i16) -> bool {
        let px = data.pixel(lx, ly);
        (px[0] as i16 - seed[0] as i16).abs() <= tol
            && (px[1] as i16 - seed[1] as i16).abs() <= tol
            && (px[2] as i16 - seed[2] as i16).abs() <= tol
            && (px[3] as i16 - seed[3] as i16).abs() <= tol
    }

    fn empty_matches(seed: &[u8; 4], tol: i16) -> bool {
        Self::pixel_matches(
            // Zero-filled tile data — comparing [0,0,0,0] against seed
            &RgbaData::default(), 0, 0, seed, tol,
        )
    }

    fn composite(&mut self, px: i32, py: i32, src: [u8; 4]) {
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

    fn erase(&mut self, px: i32, py: i32, strength: f32) {
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

    fn replace(&mut self, px: i32, py: i32, color: [u8; 4]) {
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

// ---------------------------------------------------------------------------
// MaskPaintTarget — f32 alpha mask surface
// ---------------------------------------------------------------------------

/// Convert RGBA to mask (value, strength) pair.
/// RGB luminance → target mask value, alpha → paint strength.
#[inline(always)]
fn rgba_to_mask(color: [u8; 4]) -> (f32, f32) {
    let value = (0.2126 * color[0] as f32
        + 0.7152 * color[1] as f32
        + 0.0722 * color[2] as f32) / 255.0;
    let strength = color[3] as f32 / 255.0;
    (value, strength)
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
}

impl PaintSurface for MaskPaintTarget<'_> {
    type Seed = f32;
    type TileData = AlphaF32Data;

    fn read_seed(&self, x: i32, y: i32) -> f32 {
        self.mask.sample(x, y)
    }

    fn get_tile_data(&self, tx: i32, ty: i32) -> Option<&AlphaF32Data> {
        self.mask.get(tx, ty).map(|t| t.data())
    }

    fn pixel_matches(data: &AlphaF32Data, lx: usize, ly: usize, seed: &f32, tol: i16) -> bool {
        let tol_f32 = tol as f32 / 255.0;
        (data.get(lx, ly) - seed).abs() <= tol_f32
    }

    fn empty_matches(seed: &f32, tol: i16) -> bool {
        // Missing mask tiles default to 1.0 (reveal all) via get_or_create_full
        let tol_f32 = tol as f32 / 255.0;
        (1.0 - seed).abs() <= tol_f32
    }

    fn composite(&mut self, px: i32, py: i32, src: [u8; 4]) {
        let (value, strength) = rgba_to_mask(src);
        self.paint(px, py, value, strength);
    }

    fn erase(&mut self, px: i32, py: i32, strength: f32) {
        self.paint(px, py, 0.0, strength);
    }

    fn replace(&mut self, px: i32, py: i32, color: [u8; 4]) {
        let cov = self.coverage(px, py);
        if cov <= 0.0 {
            return;
        }

        let (value, _) = rgba_to_mask(color);

        let tile_size = TILE_SIZE as i32;
        let (tx, ty) = AlphaMask::tile_coords_for_pixel(px, py);
        let lx = (px - tx * tile_size) as usize;
        let ly = (py - ty * tile_size) as usize;

        let tile = self.mask.get_or_create_full(tx, ty);
        let data = tile.write();

        if cov >= 1.0 {
            data.set(lx, ly, value);
        } else {
            let current = data.get(lx, ly);
            data.set(lx, ly, current + (value - current) * cov);
        }

        self.dirty.mark(tx, ty);
    }
}

// ---------------------------------------------------------------------------
// Surface — enum wrapper that erases the layer/mask distinction
// ---------------------------------------------------------------------------

/// A paint surface that is either an RGBA layer or an f32 mask.
///
/// This is the only type callers interact with. The layer/mask distinction
/// is invisible — write methods delegate to whichever variant, and
/// `flood_fill_on` dispatches to the concrete impl for tile-level access.
pub enum Surface<'a> {
    Layer(PaintTarget<'a>),
    Mask(MaskPaintTarget<'a>),
}

/// What kind of tile data was captured during a transaction.
/// Used by the undo system to handle both layer tiles and mask tiles.
pub enum TransactionMemento {
    Tiles(HashMap<LayerId, Memento<Rgba>>),
    Mask(LayerId, Memento<AlphaF32>),
}

impl Surface<'_> {
    pub fn composite(&mut self, px: i32, py: i32, src: [u8; 4]) {
        match self { Surface::Layer(t) => t.composite(px, py, src), Surface::Mask(t) => t.composite(px, py, src) }
    }

    pub fn erase(&mut self, px: i32, py: i32, strength: f32) {
        match self { Surface::Layer(t) => t.erase(px, py, strength), Surface::Mask(t) => t.erase(px, py, strength) }
    }

    pub fn replace(&mut self, px: i32, py: i32, color: [u8; 4]) {
        match self { Surface::Layer(t) => t.replace(px, py, color), Surface::Mask(t) => t.replace(px, py, color) }
    }

    pub fn begin_transaction(&mut self) {
        match self {
            Surface::Layer(t) => t.tiles.begin_transaction(),
            Surface::Mask(t) => t.mask.begin_transaction(),
        }
    }

    pub fn commit_transaction(&mut self, layer_id: LayerId) -> Option<TransactionMemento> {
        match self {
            Surface::Layer(t) => {
                t.tiles.commit_transaction().map(|m| {
                    let mut mementos = HashMap::new();
                    mementos.insert(layer_id, m);
                    TransactionMemento::Tiles(mementos)
                })
            }
            Surface::Mask(t) => {
                t.mask.commit_transaction()
                    .map(|m| TransactionMemento::Mask(layer_id, m))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Generic paint algorithms — work on any PaintSurface
// ---------------------------------------------------------------------------

/// Paint a filled circle onto a surface.
pub fn paint_circle(target: &mut Surface, cx: f32, cy: f32, radius: f32, color: [u8; 4]) {
    let r2 = radius * radius;
    let x_min = (cx - radius).floor() as i32;
    let x_max = (cx + radius).ceil() as i32;
    let y_min = (cy - radius).floor() as i32;
    let y_max = (cy + radius).ceil() as i32;

    for py in y_min..y_max {
        for px in x_min..x_max {
            let fpx = px as f32 + 0.5;
            let fpy = py as f32 + 0.5;
            let dx = fpx - cx;
            let dy = fpy - cy;
            if dx * dx + dy * dy <= r2 {
                target.composite(px, py, color);
            }
        }
    }
}

/// Erase a filled circle on a surface.
pub fn erase_circle(target: &mut Surface, cx: f32, cy: f32, radius: f32) {
    let r2 = radius * radius;
    let x_min = (cx - radius).floor() as i32;
    let x_max = (cx + radius).ceil() as i32;
    let y_min = (cy - radius).floor() as i32;
    let y_max = (cy + radius).ceil() as i32;

    for py in y_min..y_max {
        for px in x_min..x_max {
            let fpx = px as f32 + 0.5;
            let fpy = py as f32 + 0.5;
            let dx = fpx - cx;
            let dy = fpy - cy;
            if dx * dx + dy * dy <= r2 {
                target.erase(px, py, 1.0);
            }
        }
    }
}

/// Apply a fill mask to a surface — write `color` wherever the mask is > 0.
pub fn apply_fill(target: &mut Surface, fill_mask: &AlphaMask, color: [u8; 4]) {
    let ts = TILE_SIZE as i32;
    for ((tx, ty), mask_tile) in fill_mask.iter() {
        let base_x = tx * ts;
        let base_y = ty * ts;
        let data = mask_tile.data();
        for ly in 0..TILE_SIZE {
            for lx in 0..TILE_SIZE {
                if data.get(lx, ly) > 0.0 {
                    target.replace(base_x + lx as i32, base_y + ly as i32, color);
                }
            }
        }
    }
}

/// Draw a linear gradient between two colors on a surface.
pub fn linear_gradient(
    target: &mut Surface,
    x0: f32, y0: f32,
    x1: f32, y1: f32,
    color0: [u8; 4],
    color1: [u8; 4],
    width: u32,
    height: u32,
) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len2 = dx * dx + dy * dy;
    if len2 < 0.001 {
        return;
    }

    for py in 0..height as i32 {
        for px in 0..width as i32 {
            let fpx = px as f32 + 0.5;
            let fpy = py as f32 + 0.5;

            let t = ((fpx - x0) * dx + (fpy - y0) * dy) / len2;
            let t = t.clamp(0.0, 1.0);

            let r = (color0[0] as f32 * (1.0 - t) + color1[0] as f32 * t) as u8;
            let g = (color0[1] as f32 * (1.0 - t) + color1[1] as f32 * t) as u8;
            let b = (color0[2] as f32 * (1.0 - t) + color1[2] as f32 * t) as u8;
            let a = (color0[3] as f32 * (1.0 - t) + color1[3] as f32 * t) as u8;

            target.replace(px, py, [r, g, b, a]);
        }
    }
}

/// Flood fill from a seed point on a surface, then apply color.
pub fn flood_fill(target: &mut Surface, seed_x: i32, seed_y: i32, canvas_w: i32, canvas_h: i32, color: [u8; 4], tolerance: u8) {
    let fill_mask = match target {
        Surface::Layer(t) => AlphaMask::flood_fill_on(t, seed_x, seed_y, canvas_w, canvas_h, tolerance),
        Surface::Mask(t) => AlphaMask::flood_fill_on(t, seed_x, seed_y, canvas_w, canvas_h, tolerance),
    };
    apply_fill(target, &fill_mask, color);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

    #[test]
    fn mask_composite_converts_rgba_to_grayscale() {
        let mut mask = AlphaMask::new();
        let mut dirty = DirtyRegion::new();
        {
            let mut target = MaskPaintTarget::new(&mut mask, &mut dirty, None);
            // Paint pure white at full alpha → mask value should go to 1.0
            target.composite(5, 5, [255, 255, 255, 255]);
        }
        let tile = mask.get(0, 0).unwrap();
        assert!((tile.data().get(5, 5) - 1.0).abs() < 0.01);
    }

    #[test]
    fn mask_composite_black_hides() {
        let mut mask = AlphaMask::new();
        let mut dirty = DirtyRegion::new();
        {
            let mut target = MaskPaintTarget::new(&mut mask, &mut dirty, None);
            // Paint black at full alpha → mask value should go to 0.0
            // Start from default 1.0 (get_or_create_full)
            target.composite(5, 5, [0, 0, 0, 255]);
        }
        let tile = mask.get(0, 0).unwrap();
        assert!((tile.data().get(5, 5) - 0.0).abs() < 0.01);
    }

    #[test]
    fn mask_replace_sets_luminance() {
        let mut mask = AlphaMask::new();
        let mut dirty = DirtyRegion::new();
        {
            let mut target = MaskPaintTarget::new(&mut mask, &mut dirty, None);
            // Replace with 50% gray → value ≈ 0.5
            target.replace(5, 5, [128, 128, 128, 255]);
        }
        let tile = mask.get(0, 0).unwrap();
        assert!((tile.data().get(5, 5) - 0.502).abs() < 0.01);
    }

    #[test]
    fn paint_circle_works_on_mask_surface() {
        let mut mask = AlphaMask::new();
        let mut dirty = DirtyRegion::new();
        {
            let mut surface = Surface::Mask(MaskPaintTarget::new(&mut mask, &mut dirty, None));
            paint_circle(&mut surface, 5.0, 5.0, 1.0, [0, 0, 0, 255]);
        }
        // Center pixel should be painted toward black (0.0)
        let tile = mask.get(0, 0).unwrap();
        assert!(tile.data().get(5, 5) < 0.01);
    }

    #[test]
    fn paint_circle_works_on_layer_surface() {
        let mut tiles = TileGrid::new();
        let mut dirty = DirtyRegion::new();
        {
            let mut surface = Surface::Layer(PaintTarget::new(&mut tiles, &mut dirty, None));
            paint_circle(&mut surface, 5.0, 5.0, 1.0, [255, 0, 0, 255]);
        }
        let tile = tiles.get(0, 0).unwrap();
        assert_eq!(tile.data().pixel(5, 5), &[255, 0, 0, 255]);
    }
}
