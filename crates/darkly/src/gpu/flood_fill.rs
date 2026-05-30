//! Flat-pixel flood fill for GPU hybrid workflow.
//!
//! Operates on readback pixel data (flat RGBA or R8 arrays) instead of tiles.
//! Produces an R8 mask suitable for uploading as a GPU texture.
//!
//! Flow: readback layer → CPU scanline fill → upload mask → GPU stamp.
//!
//! ## Layer-aware orchestration ([`LayerFloodFillExtent`])
//!
//! Magic wand and the paint-bucket fill tool both flood-fill a layer from a
//! canvas-space seed and consume the result as a canvas-aligned mask. The
//! GPU layer texture, however, is **not** in general canvas-aligned: it can
//! sit at a non-zero canvas offset and be larger or smaller than the canvas
//! (paste-extent layers, leftward-grown layers from `ensure_layer_covers_dab`,
//! masks parented to off-canvas raster layers).
//!
//! [`request_layer_flood_fill_readback`] + [`LayerFloodFillExtent`] are the
//! single place that owns the canvas↔texture translation:
//! the readback samples the texture's full extent, the canvas-space seed is
//! translated to texture-local coords for the scanline fill, and the
//! resulting layer-local mask is projected back into a canvas-aligned R8
//! buffer. Both call sites consume the same `extent.flood_fill_to_canvas_mask`
//! helper so the math lives once. **Do not** call `request_readback` with a
//! canvas-rect from a flood-fill call site — go through this helper.

use std::collections::VecDeque;

use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::readback::{self, ReadbackRequest};

/// Scanline flood fill on flat RGBA pixel data.
///
/// Returns an R8 mask (width × height bytes): 255 where the fill should apply, 0 elsewhere.
/// The algorithm is the same scanline approach used by the tile-based fill, but
/// operates on contiguous pixel data from a GPU readback.
///
/// Algorithm notes (per CLAUDE.md "Performance Principle"): the implementation
/// is Smith/Heckbert scanline fill — `VecDeque<(y, start, end)>` holds whole
/// horizontal segments, not per-pixel work. Queue depth is bounded by the
/// number of distinct segments in the fill region (O(perimeter)), not the
/// pixel count. The `mask` is a flat `Vec<u8>` indexed directly; no HashMap.
/// The prior "burned by" lesson was per-pixel HashMap dispatch inside a
/// tile-based fill — this implementation specifically does not have that
/// shape, so the scanline+VecDeque pair is the right primitive here.
pub fn flood_fill_rgba(
    pixels: &[u8],
    width: u32,
    height: u32,
    seed_x: i32,
    seed_y: i32,
    tolerance: u8,
) -> Vec<u8> {
    let w = width as i32;
    let h = height as i32;
    let mut mask = vec![0u8; (width * height) as usize];

    if seed_x < 0 || seed_y < 0 || seed_x >= w || seed_y >= h {
        return mask;
    }

    let seed = read_rgba(pixels, width, seed_x, seed_y);
    let tol = tolerance as i16;

    // Find initial segment.
    let (seg_start, seg_end) = find_segment_rgba(pixels, width, w, &seed, tol, seed_x, seed_y);
    fill_span(&mut mask, width, seg_start, seg_end, seed_y);

    let mut queue = VecDeque::new();
    queue.push_back((seed_y, seg_start, seg_end));

    while let Some((y, start, end)) = queue.pop_front() {
        for dy in [-1i32, 1] {
            let ny = y + dy;
            if ny < 0 || ny >= h {
                continue;
            }
            scan_row_rgba(
                pixels, width, w, &seed, tol, &mut mask, &mut queue, ny, start, end,
            );
        }
    }

    mask
}

/// Scanline flood fill on flat R8 (single-channel) pixel data.
///
/// Used when flood-filling on a mask texture. Returns an R8 mask.
pub fn flood_fill_r8(
    pixels: &[u8],
    width: u32,
    height: u32,
    seed_x: i32,
    seed_y: i32,
    tolerance: u8,
) -> Vec<u8> {
    let w = width as i32;
    let h = height as i32;
    let mut mask = vec![0u8; (width * height) as usize];

    if seed_x < 0 || seed_y < 0 || seed_x >= w || seed_y >= h {
        return mask;
    }

    let seed = pixels[(seed_y as u32 * width + seed_x as u32) as usize];
    let tol = tolerance as i16;

    let (seg_start, seg_end) = find_segment_r8(pixels, width, w, seed, tol, seed_x, seed_y);
    fill_span(&mut mask, width, seg_start, seg_end, seed_y);

    let mut queue = VecDeque::new();
    queue.push_back((seed_y, seg_start, seg_end));

    while let Some((y, start, end)) = queue.pop_front() {
        for dy in [-1i32, 1] {
            let ny = y + dy;
            if ny < 0 || ny >= h {
                continue;
            }
            scan_row_r8(
                pixels, width, w, seed, tol, &mut mask, &mut queue, ny, start, end,
            );
        }
    }

    mask
}

// ---------------------------------------------------------------------------
// RGBA helpers
// ---------------------------------------------------------------------------

fn read_rgba(pixels: &[u8], width: u32, x: i32, y: i32) -> [u8; 4] {
    let offset = ((y as u32 * width + x as u32) * 4) as usize;
    [
        pixels[offset],
        pixels[offset + 1],
        pixels[offset + 2],
        pixels[offset + 3],
    ]
}

fn matches_rgba(pixels: &[u8], width: u32, x: i32, y: i32, seed: &[u8; 4], tol: i16) -> bool {
    let px = read_rgba(pixels, width, x, y);
    (px[0] as i16 - seed[0] as i16).abs() <= tol
        && (px[1] as i16 - seed[1] as i16).abs() <= tol
        && (px[2] as i16 - seed[2] as i16).abs() <= tol
        && (px[3] as i16 - seed[3] as i16).abs() <= tol
}

fn find_segment_rgba(
    pixels: &[u8],
    width: u32,
    canvas_w: i32,
    seed: &[u8; 4],
    tol: i16,
    x: i32,
    y: i32,
) -> (i32, i32) {
    let mut end = x;
    while end < canvas_w && matches_rgba(pixels, width, end, y, seed, tol) {
        end += 1;
    }
    let mut start = x;
    while start > 0 && matches_rgba(pixels, width, start - 1, y, seed, tol) {
        start -= 1;
    }
    (start, end)
}

fn scan_row_rgba(
    pixels: &[u8],
    width: u32,
    canvas_w: i32,
    seed: &[u8; 4],
    tol: i16,
    mask: &mut [u8],
    queue: &mut VecDeque<(i32, i32, i32)>,
    y: i32,
    start: i32,
    end: i32,
) {
    let mut x = start;
    while x < end {
        let idx = (y as u32 * width + x as u32) as usize;
        if mask[idx] != 0 || !matches_rgba(pixels, width, x, y, seed, tol) {
            x += 1;
            continue;
        }
        let (seg_start, seg_end) = find_segment_rgba(pixels, width, canvas_w, seed, tol, x, y);
        fill_span(mask, width, seg_start, seg_end, y);
        queue.push_back((y, seg_start, seg_end));
        x = seg_end;
    }
}

// ---------------------------------------------------------------------------
// R8 helpers
// ---------------------------------------------------------------------------

fn matches_r8(pixels: &[u8], width: u32, x: i32, y: i32, seed: u8, tol: i16) -> bool {
    let px = pixels[(y as u32 * width + x as u32) as usize];
    (px as i16 - seed as i16).abs() <= tol
}

fn find_segment_r8(
    pixels: &[u8],
    width: u32,
    canvas_w: i32,
    seed: u8,
    tol: i16,
    x: i32,
    y: i32,
) -> (i32, i32) {
    let mut end = x;
    while end < canvas_w && matches_r8(pixels, width, end, y, seed, tol) {
        end += 1;
    }
    let mut start = x;
    while start > 0 && matches_r8(pixels, width, start - 1, y, seed, tol) {
        start -= 1;
    }
    (start, end)
}

fn scan_row_r8(
    pixels: &[u8],
    width: u32,
    canvas_w: i32,
    seed: u8,
    tol: i16,
    mask: &mut [u8],
    queue: &mut VecDeque<(i32, i32, i32)>,
    y: i32,
    start: i32,
    end: i32,
) {
    let mut x = start;
    while x < end {
        let idx = (y as u32 * width + x as u32) as usize;
        if mask[idx] != 0 || !matches_r8(pixels, width, x, y, seed, tol) {
            x += 1;
            continue;
        }
        let (seg_start, seg_end) = find_segment_r8(pixels, width, canvas_w, seed, tol, x, y);
        fill_span(mask, width, seg_start, seg_end, y);
        queue.push_back((y, seg_start, seg_end));
        x = seg_end;
    }
}

// ---------------------------------------------------------------------------
// Common
// ---------------------------------------------------------------------------

fn fill_span(mask: &mut [u8], width: u32, start: i32, end: i32, y: i32) {
    let row_offset = (y as u32 * width) as usize;
    for x in start..end {
        mask[row_offset + x as usize] = 255;
    }
}

// ---------------------------------------------------------------------------
// Layer-aware flood-fill orchestration
// ---------------------------------------------------------------------------

/// Snapshot of a paint target's coordinate frame, captured at flood-fill
/// request time and carried through the async readback round-trip.
///
/// Owns no GPU resources — pure metadata. Pairs with the readback request
/// returned by [`request_layer_flood_fill_readback`]: the request reads the
/// texture's full extent (`width × height` pixels starting at texture-local
/// (0,0)), and this struct provides the canvas↔texture translation on the
/// other side so callers receive a canvas-aligned R8 mask without re-deriving
/// the layer offset.
#[derive(Copy, Clone)]
pub struct LayerFloodFillExtent {
    /// Canvas-space offset of the texture's (0, 0) pixel.
    pub offset_x: i32,
    pub offset_y: i32,
    /// Texture pixel dimensions — the size of the readback buffer.
    pub width: u32,
    pub height: u32,
    /// Document canvas dimensions — the size of the produced mask.
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub format: wgpu::TextureFormat,
}

impl LayerFloodFillExtent {
    pub fn from_target(target: &GpuPaintTarget<'_>) -> Self {
        let canvas_extent = target.canvas_extent();
        let layer_extent = target.layer_extent();
        let (canvas_w, canvas_h) = target.canvas_size();
        Self {
            offset_x: canvas_extent.x0(),
            offset_y: canvas_extent.y0(),
            width: layer_extent.width,
            height: layer_extent.height,
            canvas_width: canvas_w,
            canvas_height: canvas_h,
            format: target.format(),
        }
    }

    /// Run the CPU scanline fill on the texture-extent buffer and project the
    /// resulting layer-local mask into a canvas-aligned R8 mask sized
    /// `canvas_width × canvas_height`.
    ///
    /// `seed_canvas` is the click point in canvas coordinates. The seed is
    /// translated to texture-local coords before the scanline fill runs;
    /// pixels outside the layer's canvas footprint stay 0 in the output (the
    /// layer has no data there).
    ///
    /// Format dispatch matches the texture's own format — RGBA reads four
    /// bytes per pixel, R8 reads one.
    pub fn flood_fill_to_canvas_mask(
        &self,
        pixels: &[u8],
        seed_canvas: crate::coord::CanvasPoint,
        tolerance: u8,
    ) -> Vec<u8> {
        let layer_seed_x = seed_canvas.x - self.offset_x;
        let layer_seed_y = seed_canvas.y - self.offset_y;

        let layer_mask = match self.format {
            wgpu::TextureFormat::R8Unorm => flood_fill_r8(
                pixels,
                self.width,
                self.height,
                layer_seed_x,
                layer_seed_y,
                tolerance,
            ),
            _ => flood_fill_rgba(
                pixels,
                self.width,
                self.height,
                layer_seed_x,
                layer_seed_y,
                tolerance,
            ),
        };

        let cw = self.canvas_width as usize;
        let ch = self.canvas_height as usize;
        let mut canvas_mask = vec![0u8; cw * ch];

        let x0 = self.offset_x.max(0);
        let y0 = self.offset_y.max(0);
        let x1 = (self.offset_x + self.width as i32).min(self.canvas_width as i32);
        let y1 = (self.offset_y + self.height as i32).min(self.canvas_height as i32);
        if x0 >= x1 || y0 >= y1 {
            return canvas_mask;
        }

        let stride = self.width as usize;
        for cy in y0..y1 {
            let ty = (cy - self.offset_y) as usize;
            let src_row = ty * stride;
            let dst_row = (cy as usize) * cw;
            for cx in x0..x1 {
                let tx = (cx - self.offset_x) as usize;
                canvas_mask[dst_row + cx as usize] = layer_mask[src_row + tx];
            }
        }

        canvas_mask
    }
}

/// Encode a readback of a layer's full texture extent and return the request
/// paired with the extent snapshot the completion handler needs.
///
/// Single source of truth for the readback rect used by magic wand and the
/// paint-bucket flood fill. The rect is the texture's own dimensions, NOT
/// the canvas — see the module docs for why.
pub fn request_layer_flood_fill_readback(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    target: &GpuPaintTarget<'_>,
) -> (ReadbackRequest, LayerFloodFillExtent) {
    let extent = LayerFloodFillExtent::from_target(target);
    // Texture-local rect spanning the entire layer — the canvas↔texture
    // translation happens later, in `flood_fill_to_canvas_mask`.
    let request = readback::request_readback(
        device,
        encoder,
        target.texture(),
        target.format(),
        target.layer_extent(),
    );
    (request, extent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flood_fill_rgba_basic() {
        // 4×4 image: top-left 2×2 is red, rest is transparent.
        let mut pixels = vec![0u8; 4 * 4 * 4];
        for y in 0..2 {
            for x in 0..2 {
                let offset = (y * 4 + x) * 4;
                pixels[offset] = 255; // R
                pixels[offset + 3] = 255; // A
            }
        }

        // Fill from (0,0) — should fill the 2×2 red area.
        let mask = flood_fill_rgba(&pixels, 4, 4, 0, 0, 0);
        assert_eq!(mask[0], 255); // (0,0)
        assert_eq!(mask[1], 255); // (1,0)
        assert_eq!(mask[4], 255); // (0,1)
        assert_eq!(mask[5], 255); // (1,1)
        assert_eq!(mask[2], 0); // (2,0) — transparent, not matching
        assert_eq!(mask[8], 0); // (0,2) — transparent

        // Fill from (3,3) — should fill all transparent pixels.
        let mask = flood_fill_rgba(&pixels, 4, 4, 3, 3, 0);
        assert_eq!(mask[0], 0); // (0,0) — red, not matching
        assert_eq!(mask[2], 255); // (2,0) — transparent
        assert_eq!(mask[15], 255); // (3,3) — transparent
    }

    #[test]
    fn flood_fill_r8_basic() {
        // 4×4 R8 image: top-left 2×2 is 255, rest is 0.
        let mut pixels = vec![0u8; 4 * 4];
        pixels[0] = 255;
        pixels[1] = 255;
        pixels[4] = 255;
        pixels[5] = 255;

        let mask = flood_fill_r8(&pixels, 4, 4, 0, 0, 0);
        assert_eq!(mask[0], 255);
        assert_eq!(mask[5], 255);
        assert_eq!(mask[2], 0);
    }

    #[test]
    fn flood_fill_with_tolerance() {
        // 4×4 image: gradient-ish red values.
        let mut pixels = vec![0u8; 4 * 4 * 4];
        for i in 0..16 {
            let offset = i * 4;
            pixels[offset] = (i * 10) as u8; // R = 0, 10, 20, ...
            pixels[offset + 3] = 255;
        }

        // With tolerance 30, seed at (0,0) which has R=0.
        // Should match pixels with R <= 30: indices 0..=3 (R = 0, 10, 20, 30).
        let mask = flood_fill_rgba(&pixels, 4, 4, 0, 0, 30);
        assert_eq!(mask[0], 255);
        assert_eq!(mask[3], 255);
        // Index 4 has R=40, which is > 30 from seed R=0.
        assert_eq!(mask[4], 0);
    }
}
