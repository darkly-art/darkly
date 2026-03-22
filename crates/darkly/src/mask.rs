//! AlphaMask operations — boolean ops, SDF rasterization, feathering.
//!
//! `AlphaMask` is a `TileStore<AlphaF32>` (single-channel f32 per pixel).
//! It's used for selections, layer masks, and future mask-like concepts.
//! The storage, COW, and transaction/memento infrastructure are inherited
//! from the generic `TileStore<F>` in `tile.rs`.
//!
//! Shape rasterization uses the shared SDF functions from `sdf.rs`. Selection
//! tools provide an SDF closure to `rasterize()`, which evaluates it at each
//! pixel center and writes coverage values into tiles.

use crate::tile::{AlphaF32, AlphaF32Data, AlphaMask, Tile, TILE_SIZE};

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
    /// Invert the mask within the given canvas bounds (pixels).
    /// Creates tiles for the full canvas extent so the inverted "outside"
    /// region is correctly filled with 1.0.
    pub fn invert(&mut self, canvas_w: u32, canvas_h: u32) {
        let ts = TILE_SIZE as i32;
        // Ensure tiles exist for the entire canvas.
        let tx_max = ((canvas_w as i32) - 1).div_euclid(ts);
        let ty_max = ((canvas_h as i32) - 1).div_euclid(ts);
        for ty in 0..=ty_max {
            for tx in 0..=tx_max {
                self.get_or_create(tx, ty);
            }
        }
        // Now invert all tiles (existing + newly created).
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

    /// Tight pixel-level bounding rect of non-zero coverage: `[x, y, w, h]`.
    ///
    /// Scans every tile's actual pixel data, so this is more expensive than
    /// `bounding_rect()` but gives exact bounds with no tile-alignment padding.
    pub fn pixel_bounding_rect(&self) -> Option<[u32; 4]> {
        let ts = TILE_SIZE as i32;
        let mut px_min_x = i32::MAX;
        let mut px_min_y = i32::MAX;
        let mut px_max_x = i32::MIN;
        let mut px_max_y = i32::MIN;

        for ((tx, ty), tile) in self.iter() {
            let data = tile.data();
            let origin_x = tx * ts;
            let origin_y = ty * ts;

            for ly in 0..TILE_SIZE {
                for lx in 0..TILE_SIZE {
                    if data.0[ly * TILE_SIZE + lx] > 0.0 {
                        let px = origin_x + lx as i32;
                        let py = origin_y + ly as i32;
                        px_min_x = px_min_x.min(px);
                        px_min_y = px_min_y.min(py);
                        px_max_x = px_max_x.max(px);
                        px_max_y = px_max_y.max(py);
                    }
                }
            }
        }

        if px_min_x <= px_max_x {
            let x = px_min_x.max(0) as u32;
            let y = px_min_y.max(0) as u32;
            let w = (px_max_x - px_min_x + 1) as u32;
            let h = (px_max_y - px_min_y + 1) as u32;
            Some([x, y, w, h])
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
// SDF rasterization
// ---------------------------------------------------------------------------

impl AlphaMask {
    /// Rasterize a shape defined by a signed distance function into the mask.
    ///
    /// The SDF is evaluated at each pixel center within `bounds` (plus margin for
    /// antialiasing/feathering). Positive = outside, negative = inside.
    ///
    /// - `bounds`: (x, y, width, height) in pixel coordinates — the shape's bounding rect
    /// - `sdf_fn`: returns signed distance at pixel center (negative inside, positive outside)
    /// - `antialias`: smooth 1px edge transition (ignored if feather > 0)
    /// - `feather`: if > 0, smooth transition over this many pixels
    pub fn rasterize(
        &mut self,
        bounds: (i32, i32, i32, i32),
        sdf_fn: impl Fn(f32, f32) -> f32,
        antialias: bool,
        feather: f32,
    ) {
        let (bx, by, bw, bh) = bounds;
        // Expand bounds for edge softening
        let margin = if feather > 0.0 {
            feather.ceil() as i32
        } else if antialias {
            1
        } else {
            0
        };
        let x0 = bx - margin;
        let y0 = by - margin;
        let x1 = bx + bw + margin;
        let y1 = by + bh + margin;

        let ts = TILE_SIZE as i32;
        let (tx0, ty0) = Self::tile_coords_for_pixel(x0, y0);
        let (tx1, ty1) = Self::tile_coords_for_pixel(x1 - 1, y1 - 1);

        let edge_band = if feather > 0.0 {
            feather * 0.5
        } else if antialias {
            0.5
        } else {
            0.0
        };

        for tty in ty0..=ty1 {
            for ttx in tx0..=tx1 {
                let base_px = ttx * ts;
                let base_py = tty * ts;

                // Sample SDF at tile corners for tile-level optimization.
                let corners = [
                    sdf_fn(base_px as f32 + 0.5, base_py as f32 + 0.5),
                    sdf_fn((base_px + ts - 1) as f32 + 0.5, base_py as f32 + 0.5),
                    sdf_fn(base_px as f32 + 0.5, (base_py + ts - 1) as f32 + 0.5),
                    sdf_fn(
                        (base_px + ts - 1) as f32 + 0.5,
                        (base_py + ts - 1) as f32 + 0.5,
                    ),
                ];
                let max_corner = corners.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let min_corner = corners.iter().copied().fold(f32::INFINITY, f32::min);

                // All corners deeply inside → fill entire tile with 1.0
                if max_corner < -edge_band {
                    let tile = self.get_or_create(ttx, tty);
                    tile.write().0.fill(1.0);
                    continue;
                }

                // All corners far outside → skip tile entirely.
                // Conservative: use half-diagonal as safety margin for non-convex shapes.
                let half_diag = (ts as f32) * std::f32::consts::FRAC_1_SQRT_2;
                if min_corner > edge_band + half_diag {
                    continue;
                }

                // Tile crosses the boundary — per-pixel evaluation.
                let tile = self.get_or_create(ttx, tty);
                let data = tile.write();
                for ly in 0..TILE_SIZE {
                    let py = base_py + ly as i32;
                    if py < y0 || py >= y1 {
                        continue;
                    }
                    for lx in 0..TILE_SIZE {
                        let px = base_px + lx as i32;
                        if px < x0 || px >= x1 {
                            continue;
                        }
                        let sdf = sdf_fn(px as f32 + 0.5, py as f32 + 0.5);
                        let coverage = crate::sdf::sdf_coverage(sdf, antialias, feather);
                        if coverage > 0.0 {
                            data.set(lx, ly, coverage);
                        }
                    }
                }
            }
        }
    }
}


// ---------------------------------------------------------------------------
// Flat-buffer SDF rasterization (no tile indirection)
// ---------------------------------------------------------------------------

/// Rasterize an SDF shape directly into a flat R8 buffer (canvas-sized).
///
/// This is the tile-free equivalent of `AlphaMask::rasterize()`, producing a
/// `Vec<u8>` ready for `queue.write_texture()` to a GPU R8 texture.
///
/// - `canvas_width`, `canvas_height`: full canvas dimensions
/// - `bounds`: (x, y, w, h) pixel bounding box of the shape
/// - `sdf_fn`: signed distance at pixel center (negative = inside)
/// - `antialias`: smooth 1px edge transition
/// - `feather`: if > 0, smooth transition over this many pixels
pub fn rasterize_sdf_r8(
    canvas_width: u32,
    canvas_height: u32,
    bounds: (i32, i32, i32, i32),
    sdf_fn: impl Fn(f32, f32) -> f32,
    antialias: bool,
    feather: f32,
) -> Vec<u8> {
    let mut pixels = vec![0u8; (canvas_width * canvas_height) as usize];
    let (bx, by, bw, bh) = bounds;

    let margin = if feather > 0.0 {
        feather.ceil() as i32
    } else if antialias {
        1
    } else {
        0
    };

    let x0 = (bx - margin).max(0) as u32;
    let y0 = (by - margin).max(0) as u32;
    let x1 = ((bx + bw + margin) as u32).min(canvas_width);
    let y1 = ((by + bh + margin) as u32).min(canvas_height);

    for py in y0..y1 {
        for px in x0..x1 {
            let sdf = sdf_fn(px as f32 + 0.5, py as f32 + 0.5);
            let coverage = crate::sdf::sdf_coverage(sdf, antialias, feather);
            if coverage > 0.0 {
                pixels[(py * canvas_width + px) as usize] = (coverage * 255.0) as u8;
            }
        }
    }

    pixels
}

// ---------------------------------------------------------------------------
// Flat-buffer contour extraction (no tile indirection)
// ---------------------------------------------------------------------------

/// Extract contour segments from a flat R8 buffer using marching squares.
///
/// Equivalent to `AlphaMask::contour_segments()` but operates on a flat `&[u8]`
/// from GPU readback instead of tile-based storage.
pub fn contour_segments_r8(
    pixels: &[u8],
    width: u32,
    height: u32,
    threshold: u8,
) -> Vec<([f32; 2], [f32; 2])> {
    let [bx, by, bw, bh] = match pixel_bounds_r8(pixels, width, height) {
        Some(b) => b,
        None => return Vec::new(),
    };

    // Extend by 1 pixel for marching squares boundary blocks.
    let px_min = (bx as i32 - 1).max(0);
    let py_min = (by as i32 - 1).max(0);
    let px_max = ((bx + bw) as i32).min(width as i32 - 1);
    let py_max = ((by + bh) as i32).min(height as i32 - 1);

    let sample = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
            return 0.0;
        }
        pixels[(y as u32 * width + x as u32) as usize] as f32 / 255.0
    };

    let threshold_f = threshold as f32 / 255.0;
    let mut segments = Vec::new();

    for py in py_min..py_max {
        for px in px_min..px_max {
            let tl = sample(px, py) > threshold_f;
            let tr = sample(px + 1, py) > threshold_f;
            let bl = sample(px, py + 1) > threshold_f;
            let br = sample(px + 1, py + 1) > threshold_f;

            let index = (tl as u8) | ((tr as u8) << 1) | ((bl as u8) << 2) | ((br as u8) << 3);
            if index == 0 || index == 15 {
                continue;
            }

            let x = px as f32;
            let y = py as f32;

            let top = lerp_edge(sample(px, py), sample(px + 1, py), threshold_f);
            let bottom = lerp_edge(sample(px, py + 1), sample(px + 1, py + 1), threshold_f);
            let left = lerp_edge(sample(px, py), sample(px, py + 1), threshold_f);
            let right = lerp_edge(sample(px + 1, py), sample(px + 1, py + 1), threshold_f);

            let t = [x + top, y];
            let b = [x + bottom, y + 1.0];
            let l = [x, y + left];
            let r = [x + 1.0, y + right];

            match index {
                1  => segments.push((l, t)),
                2  => segments.push((t, r)),
                3  => segments.push((l, r)),
                4  => segments.push((b, l)),
                5  => segments.push((b, t)),
                6  => { segments.push((t, r)); segments.push((b, l)); }
                7  => segments.push((b, r)),
                8  => segments.push((r, b)),
                9  => { segments.push((l, t)); segments.push((r, b)); }
                10 => segments.push((t, b)),
                11 => segments.push((l, b)),
                12 => segments.push((r, l)),
                13 => segments.push((r, t)),
                14 => segments.push((t, l)),
                _  => unreachable!(),
            }
        }
    }

    merge_collinear(segments)
}

/// Compute tight pixel bounding box from a flat R8 buffer.
/// Returns `[x, y, w, h]` or None if all pixels are zero.
pub fn pixel_bounds_r8(pixels: &[u8], width: u32, height: u32) -> Option<[u32; 4]> {
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..height {
        for x in 0..width {
            if pixels[(y * width + x) as usize] > 0 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    if max_x < min_x {
        None
    } else {
        Some([min_x, min_y, max_x - min_x + 1, max_y - min_y + 1])
    }
}

// ---------------------------------------------------------------------------
// Feathering (separable Gaussian blur)
// ---------------------------------------------------------------------------

/// Compute a normalized 1D Gaussian kernel with the given radius.
/// Kernel extends ±ceil(radius) pixels. σ = radius / 2.
fn gaussian_kernel(radius: f32) -> Vec<f32> {
    let sigma = radius * 0.5;
    let half = radius.ceil() as usize;
    let size = 2 * half + 1;
    let mut kernel = Vec::with_capacity(size);
    let two_sigma_sq = 2.0 * sigma * sigma;
    let mut sum = 0.0;

    for i in 0..size {
        let x = i as f32 - half as f32;
        let val = (-x * x / two_sigma_sq).exp();
        kernel.push(val);
        sum += val;
    }

    for v in &mut kernel {
        *v /= sum;
    }
    kernel
}

impl AlphaMask {
    /// Apply Gaussian feathering (blur) to the mask.
    ///
    /// Uses separable 2D Gaussian convolution: horizontal pass then vertical pass.
    /// σ = radius / 2, kernel extends ±ceil(radius) pixels. Expands the mask
    /// by the blur radius in all directions.
    pub fn feather(&mut self, radius: f32) {
        if radius < 0.5 {
            return;
        }

        let kernel = gaussian_kernel(radius);
        let half = (kernel.len() / 2) as i32;

        let Some((tx_min, ty_min, tx_max, ty_max)) = self.bounding_rect() else {
            return;
        };

        let ts = TILE_SIZE as i32;
        let tile_expand = ((half as usize) + TILE_SIZE - 1) / TILE_SIZE;
        let te = tile_expand as i32;

        // Horizontal blur: self → intermediate
        let mut intermediate = AlphaMask::new();
        for tty in ty_min..=ty_max {
            for ttx in (tx_min - te)..=(tx_max + te) {
                let base_px = ttx * ts;
                let base_py = tty * ts;
                let mut tile_data = AlphaF32Data::default();
                let mut any = false;

                for ly in 0..TILE_SIZE {
                    let py = base_py + ly as i32;
                    for lx in 0..TILE_SIZE {
                        let px = base_px + lx as i32;
                        let mut sum = 0.0;
                        for (ki, &weight) in kernel.iter().enumerate() {
                            let sx = px + ki as i32 - half;
                            sum += self.sample(sx, py) * weight;
                        }
                        if sum > 1e-6 {
                            tile_data.set(lx, ly, sum);
                            any = true;
                        }
                    }
                }

                if any {
                    let tile = intermediate.get_or_create(ttx, tty);
                    *tile.write() = tile_data;
                }
            }
        }

        // Vertical blur: intermediate → result
        let Some((ix_min, iy_min, ix_max, iy_max)) = intermediate.bounding_rect() else {
            self.clear();
            return;
        };

        let mut result = AlphaMask::new();
        for tty in (iy_min - te)..=(iy_max + te) {
            for ttx in ix_min..=ix_max {
                let base_px = ttx * ts;
                let base_py = tty * ts;
                let mut tile_data = AlphaF32Data::default();
                let mut any = false;

                for ly in 0..TILE_SIZE {
                    let py = base_py + ly as i32;
                    for lx in 0..TILE_SIZE {
                        let px = base_px + lx as i32;
                        let mut sum = 0.0;
                        for (ki, &weight) in kernel.iter().enumerate() {
                            let sy = py + ki as i32 - half;
                            sum += intermediate.sample(px, sy) * weight;
                        }
                        if sum > 1e-6 {
                            tile_data.set(lx, ly, sum.min(1.0));
                            any = true;
                        }
                    }
                }

                if any {
                    let tile = result.get_or_create(ttx, tty);
                    *tile.write() = tile_data;
                }
            }
        }

        *self = result;
    }
}

// ---------------------------------------------------------------------------
// Contour extraction (marching squares)
// ---------------------------------------------------------------------------

impl AlphaMask {
    /// Extract contour line segments from the mask at the given threshold.
    ///
    /// Uses marching squares on the pixel grid: for each 2×2 block, classify
    /// corners as inside (> threshold) or outside (≤ threshold) and emit edge
    /// segments based on the 16 possible configurations. Returns segments in
    /// canvas pixel coordinates.
    ///
    /// The contour is recomputed only when the selection changes (infrequent).
    pub fn contour_segments(&self, threshold: f32) -> Vec<([f32; 2], [f32; 2])> {
        let Some((tx_min, ty_min, tx_max, ty_max)) = self.bounding_rect() else {
            return Vec::new();
        };

        let ts = TILE_SIZE as i32;
        // Pixel range: one extra pixel on each side for the 2×2 block boundary
        let px_min = tx_min * ts - 1;
        let py_min = ty_min * ts - 1;
        let px_max = (tx_max + 1) * ts;
        let py_max = (ty_max + 1) * ts;

        let mut segments = Vec::new();

        for py in py_min..py_max {
            for px in px_min..px_max {
                // 2×2 block corners: TL=(px,py), TR=(px+1,py), BL=(px,py+1), BR=(px+1,py+1)
                let tl = self.sample(px, py) > threshold;
                let tr = self.sample(px + 1, py) > threshold;
                let bl = self.sample(px, py + 1) > threshold;
                let br = self.sample(px + 1, py + 1) > threshold;

                let index = (tl as u8) | ((tr as u8) << 1) | ((bl as u8) << 2) | ((br as u8) << 3);

                // Skip empty (0) and full (15)
                if index == 0 || index == 15 {
                    continue;
                }

                let x = px as f32;
                let y = py as f32;

                // Interpolation along edges for smoother contours
                let top = lerp_edge(self.sample(px, py), self.sample(px + 1, py), threshold);
                let bottom = lerp_edge(self.sample(px, py + 1), self.sample(px + 1, py + 1), threshold);
                let left = lerp_edge(self.sample(px, py), self.sample(px, py + 1), threshold);
                let right = lerp_edge(self.sample(px + 1, py), self.sample(px + 1, py + 1), threshold);

                let t = [x + top, y];       // top edge
                let b = [x + bottom, y + 1.0]; // bottom edge
                let l = [x, y + left];      // left edge
                let r = [x + 1.0, y + right]; // right edge

                // Marching squares lookup — emit 1 or 2 segments per cell.
                match index {
                    1  => segments.push((l, t)),       // TL inside
                    2  => segments.push((t, r)),       // TR inside
                    3  => segments.push((l, r)),       // TL+TR inside
                    4  => segments.push((b, l)),       // BL inside
                    5  => segments.push((b, t)),       // TL+BL inside
                    6  => { segments.push((t, r)); segments.push((b, l)); } // TR+BL (saddle)
                    7  => segments.push((b, r)),       // TL+TR+BL inside
                    8  => segments.push((r, b)),       // BR inside
                    9  => { segments.push((l, t)); segments.push((r, b)); } // TL+BR (saddle)
                    10 => segments.push((t, b)),       // TR+BR inside
                    11 => segments.push((l, b)),       // TL+TR+BR inside
                    12 => segments.push((r, l)),       // BL+BR inside
                    13 => segments.push((r, t)),       // TL+BL+BR inside
                    14 => segments.push((t, l)),       // TR+BL+BR inside
                    _  => unreachable!(),
                }
            }
        }

        merge_collinear(segments)
    }
}

/// Merge collinear adjacent segments to reduce primitive count.
///
/// Separates segments into horizontal (same Y), vertical (same X), and diagonal.
/// Horizontal/vertical groups are sorted and merged when endpoints touch.
/// A 200×200 rectangle goes from ~800 segments to ~4.
fn merge_collinear(segments: Vec<([f32; 2], [f32; 2])>) -> Vec<([f32; 2], [f32; 2])> {
    use std::collections::BTreeMap;

    // Quantize coordinate to integer key for grouping (f32 bits as i32).
    fn key(v: f32) -> i32 { v.to_bits() as i32 }

    // Group by (coordinate, reversed) to preserve winding direction from
    // marching squares. This ensures the dash animation marches consistently
    // around the contour (clockwise).
    // Horizontal segments: a[1] == b[1]; reversed = a[0] > b[0] (right-to-left)
    // Vertical segments: a[0] == b[0]; reversed = a[1] > b[1] (bottom-to-top)
    let mut horiz: BTreeMap<(i32, bool), Vec<(f32, f32)>> = BTreeMap::new();
    let mut vert: BTreeMap<(i32, bool), Vec<(f32, f32)>> = BTreeMap::new();
    let mut other: Vec<([f32; 2], [f32; 2])> = Vec::new();

    for (a, b) in segments {
        if a[1] == b[1] {
            let reversed = a[0] > b[0];
            let (lo, hi) = if reversed { (b[0], a[0]) } else { (a[0], b[0]) };
            horiz.entry((key(a[1]), reversed)).or_default().push((lo, hi));
        } else if a[0] == b[0] {
            let reversed = a[1] > b[1];
            let (lo, hi) = if reversed { (b[1], a[1]) } else { (a[1], b[1]) };
            vert.entry((key(a[0]), reversed)).or_default().push((lo, hi));
        } else {
            other.push((a, b));
        }
    }

    let mut result = Vec::new();

    // Merge horizontal spans, preserving direction.
    for ((y_bits, reversed), mut spans) in horiz {
        let y = f32::from_bits(y_bits as u32);
        spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let (mut lo, mut hi) = spans[0];
        for &(s_lo, s_hi) in &spans[1..] {
            if s_lo == hi {
                hi = s_hi;
            } else {
                if reversed {
                    result.push(([hi, y], [lo, y]));
                } else {
                    result.push(([lo, y], [hi, y]));
                }
                lo = s_lo;
                hi = s_hi;
            }
        }
        if reversed {
            result.push(([hi, y], [lo, y]));
        } else {
            result.push(([lo, y], [hi, y]));
        }
    }

    // Merge vertical spans, preserving direction.
    for ((x_bits, reversed), mut spans) in vert {
        let x = f32::from_bits(x_bits as u32);
        spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let (mut lo, mut hi) = spans[0];
        for &(s_lo, s_hi) in &spans[1..] {
            if s_lo == hi {
                hi = s_hi;
            } else {
                if reversed {
                    result.push(([x, hi], [x, lo]));
                } else {
                    result.push(([x, lo], [x, hi]));
                }
                lo = s_lo;
                hi = s_hi;
            }
        }
        if reversed {
            result.push(([x, hi], [x, lo]));
        } else {
            result.push(([x, lo], [x, hi]));
        }
    }

    result.extend(other);
    simplify_segments(result)
}

/// Chain independent segments into polylines, then simplify with
/// Ramer-Douglas-Peucker. Reduces curved contours (ellipses, polygons)
/// from hundreds of segments to tens while preserving shape within ±1px.
fn simplify_segments(segments: Vec<([f32; 2], [f32; 2])>) -> Vec<([f32; 2], [f32; 2])> {
    if segments.len() <= 32 {
        return segments;
    }

    // Build adjacency: endpoint → list of segment indices.
    // Quantize coordinates to avoid f32 precision issues.
    use std::collections::HashMap;

    fn qkey(p: [f32; 2]) -> (i64, i64) {
        ((p[0] * 1024.0) as i64, (p[1] * 1024.0) as i64)
    }

    let mut adj: HashMap<(i64, i64), Vec<(usize, bool)>> = HashMap::new();
    for (i, (a, b)) in segments.iter().enumerate() {
        adj.entry(qkey(*a)).or_default().push((i, false)); // false = start
        adj.entry(qkey(*b)).or_default().push((i, true));  // true = end
    }

    // Chain segments into polylines via greedy traversal.
    let mut used = vec![false; segments.len()];
    let mut chains: Vec<Vec<[f32; 2]>> = Vec::new();

    for start_idx in 0..segments.len() {
        if used[start_idx] {
            continue;
        }
        used[start_idx] = true;
        let (a, b) = segments[start_idx];
        let mut chain = vec![a, b];

        // Extend forward from the last point.
        loop {
            let tail = *chain.last().unwrap();
            let key = qkey(tail);
            let next = adj.get(&key).and_then(|neighbors| {
                neighbors.iter().find(|&&(idx, _)| !used[idx])
            });
            match next {
                Some(&(idx, is_end)) => {
                    used[idx] = true;
                    let (sa, sb) = segments[idx];
                    if is_end {
                        // tail matches segment end → traverse backward
                        chain.push(sa);
                    } else {
                        // tail matches segment start → traverse forward
                        chain.push(sb);
                    }
                }
                None => break,
            }
        }

        // Extend backward from the first point.
        loop {
            let head = chain[0];
            let key = qkey(head);
            let next = adj.get(&key).and_then(|neighbors| {
                neighbors.iter().find(|&&(idx, _)| !used[idx])
            });
            match next {
                Some(&(idx, is_end)) => {
                    used[idx] = true;
                    let (sa, sb) = segments[idx];
                    if is_end {
                        chain.insert(0, sa);
                    } else {
                        chain.insert(0, sb);
                    }
                }
                None => break,
            }
        }

        chains.push(chain);
    }

    // Simplify each chain with Ramer-Douglas-Peucker (epsilon = 1.0 px).
    let mut result = Vec::new();
    for chain in &chains {
        let simplified = rdp_simplify(chain, 1.0);
        for w in simplified.windows(2) {
            result.push((w[0], w[1]));
        }
    }

    result
}

/// Ramer-Douglas-Peucker polyline simplification.
/// Removes points that deviate less than `epsilon` from the line between
/// their neighbors. Preserves endpoints and sharp corners.
fn rdp_simplify(points: &[[f32; 2]], epsilon: f32) -> Vec<[f32; 2]> {
    if points.len() <= 2 {
        return points.to_vec();
    }

    // Find the point farthest from the line between first and last.
    let first = points[0];
    let last = points[points.len() - 1];
    let mut max_dist = 0.0f32;
    let mut max_idx = 0;

    for (i, p) in points.iter().enumerate().skip(1).take(points.len() - 2) {
        let d = point_to_line_dist(*p, first, last);
        if d > max_dist {
            max_dist = d;
            max_idx = i;
        }
    }

    if max_dist > epsilon {
        // Recurse on both halves.
        let mut left = rdp_simplify(&points[..=max_idx], epsilon);
        let right = rdp_simplify(&points[max_idx..], epsilon);
        left.pop(); // Remove duplicate at split point.
        left.extend(right);
        left
    } else {
        // All intermediate points are within epsilon — keep only endpoints.
        vec![first, last]
    }
}

/// Perpendicular distance from point `p` to line segment `a`–`b`.
fn point_to_line_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        let ex = p[0] - a[0];
        let ey = p[1] - a[1];
        return (ex * ex + ey * ey).sqrt();
    }
    // Signed area of triangle / base length = perpendicular distance.
    ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / len_sq.sqrt()
}

/// Linear interpolation for contour edge crossing.
/// Returns position [0,1] along the edge where the threshold is crossed.
fn lerp_edge(v0: f32, v1: f32, threshold: f32) -> f32 {
    let dv = v1 - v0;
    if dv.abs() < 1e-6 {
        0.5
    } else {
        ((threshold - v0) / dv).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// R8 pixel buffer conversions
// ---------------------------------------------------------------------------

impl AlphaMask {
    /// Rasterize the mask into a flat R8 (`Vec<u8>`) pixel buffer.
    ///
    /// `origin` is the top-left corner in canvas pixel coordinates.
    /// Pixels outside allocated tiles default to `default_value`.
    pub fn rasterize_r8(
        &self,
        origin: (i32, i32),
        width: u32,
        height: u32,
        default_value: u8,
    ) -> Vec<u8> {
        let mut pixels = vec![default_value; (width * height) as usize];
        let (ox, oy) = origin;
        let ts = TILE_SIZE;

        for ((tx, ty), tile) in self.iter() {
            let base_x = tx * ts as i32;
            let base_y = ty * ts as i32;
            let data = tile.data();
            for ly in 0..ts {
                for lx in 0..ts {
                    let px = base_x + lx as i32 - ox;
                    let py = base_y + ly as i32 - oy;
                    if px >= 0 && py >= 0 && (px as u32) < width && (py as u32) < height {
                        let v = (data.get(lx, ly).clamp(0.0, 1.0) * 255.0) as u8;
                        pixels[(py as u32 * width + px as u32) as usize] = v;
                    }
                }
            }
        }

        pixels
    }

    /// Construct an AlphaMask from a flat R8 pixel buffer.
    ///
    /// Pixels with value 0 are skipped (treated as empty). The buffer covers
    /// canvas coordinates starting at (0, 0).
    pub fn from_r8(pixels: &[u8], width: u32, height: u32) -> Self {
        let ts = TILE_SIZE;
        let mut mask = AlphaMask::new();

        for py in 0..height {
            for px in 0..width {
                let v = pixels[(py * width + px) as usize];
                if v > 0 {
                    let tx = (px / ts as u32) as i32;
                    let ty = (py / ts as u32) as i32;
                    let lx = (px % ts as u32) as usize;
                    let ly = (py % ts as u32) as usize;
                    mask.get_or_create(tx, ty).write().set(lx, ly, v as f32 / 255.0);
                }
            }
        }

        mask
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
        // Canvas is 64×64 — invert should fill the full canvas extent.
        mask.invert(64, 64);

        // Inside the original rect: 1.0 - 0.75 = 0.25.
        assert!((mask.sample(5, 5) - 0.25).abs() < 1e-6);
        // Outside the original rect but inside canvas: 1.0 - 0.0 = 1.0.
        assert!((mask.sample(32, 32) - 1.0).abs() < 1e-6);
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

    // --- rasterize ---

    #[test]
    fn rasterize_rect_hard_edge() {
        let mut mask = AlphaMask::new();
        // 20x10 rect at (5, 5)
        mask.rasterize(
            (5, 5, 20, 10),
            |px, py| crate::sdf::sdf_rect(px, py, 15.0, 10.0, 10.0, 5.0),
            false,
            0.0,
        );
        // Inside
        assert_eq!(mask.sample(10, 8), 1.0);
        assert_eq!(mask.sample(15, 10), 1.0);
        // Outside
        assert_eq!(mask.sample(3, 8), 0.0);
        assert_eq!(mask.sample(10, 20), 0.0);
    }

    #[test]
    fn rasterize_rect_antialiased() {
        let mut mask = AlphaMask::new();
        // Use non-integer boundary so pixel centers fall in the AA transition zone.
        // cx=30.25, hw=20 → right edge at x=50.25. Pixel center 50.5 has sdf=0.25 → partial.
        mask.rasterize(
            (10, 10, 41, 30),
            |px, py| crate::sdf::sdf_rect(px, py, 30.25, 25.0, 20.0, 15.0),
            true,
            0.0,
        );
        // Deep inside = 1.0
        assert_eq!(mask.sample(25, 20), 1.0);
        // Deep outside = 0.0
        assert_eq!(mask.sample(0, 0), 0.0);
        // On the boundary: pixel at x=50, center 50.5, sdf = 50.5 - 50.25 = 0.25
        // smoothstep(0.5, -0.5, 0.25) → t = (0.25-0.5)/(-1) = 0.25 → ~0.156
        let edge = mask.sample(50, 20);
        assert!(
            edge > 0.0 && edge < 1.0,
            "edge pixel should be partially covered, got {edge}"
        );
    }

    #[test]
    fn rasterize_circle_hard_edge() {
        let mut mask = AlphaMask::new();
        mask.rasterize(
            (0, 0, 100, 100),
            |px, py| crate::sdf::sdf_circle(px, py, 50.0, 50.0, 30.0),
            false,
            0.0,
        );
        // Center
        assert_eq!(mask.sample(50, 50), 1.0);
        // Inside near edge
        assert_eq!(mask.sample(50, 22), 1.0);
        // Outside
        assert_eq!(mask.sample(50, 15), 0.0);
    }

    #[test]
    fn rasterize_feathered() {
        let mut mask = AlphaMask::new();
        mask.rasterize(
            (10, 10, 40, 30),
            |px, py| crate::sdf::sdf_rect(px, py, 30.0, 25.0, 20.0, 15.0),
            false,
            4.0,
        );
        // Deep inside = 1.0
        assert_eq!(mask.sample(25, 20), 1.0);
        // Near boundary: coverage should be between 0 and 1 in the transition zone.
        // Right edge at x=50. Pixel at x=49 (center 49.5) has sdf=-0.5.
        // feather=4 → smoothstep(2,-2,-0.5) → partial coverage.
        let near_edge = mask.sample(49, 20);
        assert!(
            near_edge > 0.0 && near_edge < 1.0,
            "near-boundary pixel should be partially covered, got {near_edge}"
        );
        // 1px outside boundary: pixel at x=50 (center 50.5), sdf=0.5 → also partial
        let just_outside = mask.sample(50, 20);
        assert!(
            just_outside > 0.0 && just_outside < 1.0,
            "just-outside pixel should be partially covered, got {just_outside}"
        );
        // Well outside: pixel at x=52 (center 52.5), sdf=2.5 → coverage ≈ 0
        let far_outside = mask.sample(52, 20);
        assert!(far_outside < 0.05, "far outside should be ~0, got {far_outside}");
    }

    #[test]
    fn rasterize_polygon() {
        let mut mask = AlphaMask::new();
        let verts = [[10.0, 10.0], [50.0, 10.0], [50.0, 50.0], [10.0, 50.0]];
        mask.rasterize(
            (10, 10, 40, 40),
            |px, py| crate::sdf::sdf_polygon(px, py, &verts),
            false,
            0.0,
        );
        // Inside
        assert_eq!(mask.sample(30, 30), 1.0);
        // Outside
        assert_eq!(mask.sample(5, 5), 0.0);
    }

    #[test]
    fn rasterize_ellipse() {
        let mut mask = AlphaMask::new();
        mask.rasterize(
            (0, 0, 100, 60),
            |px, py| crate::sdf::sdf_ellipse(px, py, 50.0, 30.0, 40.0, 20.0),
            false,
            0.0,
        );
        // Center
        assert_eq!(mask.sample(50, 30), 1.0);
        // Well outside
        assert_eq!(mask.sample(95, 30), 0.0);
        assert_eq!(mask.sample(50, 55), 0.0);
    }

    // --- feather ---

    #[test]
    fn feather_expands_mask() {
        let mut mask = AlphaMask::new();
        mask.fill_rect_test(20, 20, 20, 20, 1.0); // 20x20 solid block

        // Before feathering: outside boundary is 0
        assert_eq!(mask.sample(18, 30), 0.0);

        mask.feather(4.0);

        // After feathering: pixels just outside should have non-zero values
        let outside = mask.sample(18, 30);
        assert!(
            outside > 0.01,
            "feather should expand mask beyond original boundary, got {outside}"
        );

        // Center should still be close to 1.0 (may be slightly less due to normalization)
        let center = mask.sample(30, 30);
        assert!(center > 0.9, "center should remain near 1.0 after feather, got {center}");
    }

    #[test]
    fn feather_zero_radius_noop() {
        let mut mask = AlphaMask::new();
        mask.fill_rect_test(10, 10, 10, 10, 0.75);
        let before = mask.sample(15, 15);

        mask.feather(0.0);

        assert_eq!(mask.sample(15, 15), before);
    }

    #[test]
    fn feather_empty_mask() {
        let mut mask = AlphaMask::new();
        mask.feather(5.0); // should not panic
        assert!(mask.is_empty());
    }

    // --- contour_segments ---

    #[test]
    fn contour_empty_mask() {
        let mask = AlphaMask::new();
        assert!(mask.contour_segments(0.5).is_empty());
    }

    #[test]
    fn contour_rect_produces_segments() {
        let mut mask = AlphaMask::new();
        mask.fill_rect_test(10, 10, 20, 20, 1.0);
        let segs = mask.contour_segments(0.5);
        // A 20×20 rectangle should produce boundary segments
        assert!(!segs.is_empty(), "contour should produce segments for a filled rect");
        // Segments should be near the boundary (x=10, x=30, y=10, y=30)
        for (a, b) in &segs {
            let near_boundary =
                (a[0] >= 9.0 && a[0] <= 31.0) && (a[1] >= 9.0 && a[1] <= 31.0)
                && (b[0] >= 9.0 && b[0] <= 31.0) && (b[1] >= 9.0 && b[1] <= 31.0);
            assert!(near_boundary, "segment [{},{}]-[{},{}] should be near boundary",
                a[0], a[1], b[0], b[1]);
        }
    }
}
