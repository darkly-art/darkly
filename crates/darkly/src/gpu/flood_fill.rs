//! Flat-pixel flood fill for GPU hybrid workflow.
//!
//! Operates on readback pixel data (flat RGBA or R8 arrays) instead of tiles.
//! Produces an R8 mask suitable for uploading as a GPU texture.
//!
//! Flow: readback layer → CPU scanline fill → upload mask → GPU stamp.

use std::collections::VecDeque;

/// Scanline flood fill on flat RGBA pixel data.
///
/// Returns an R8 mask (width × height bytes): 255 where the fill should apply, 0 elsewhere.
/// The algorithm is the same scanline approach used by the tile-based fill, but
/// operates on contiguous pixel data from a GPU readback.
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
            scan_row_rgba(pixels, width, w, &seed, tol, &mut mask, &mut queue, ny, start, end);
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
            scan_row_r8(pixels, width, w, seed, tol, &mut mask, &mut queue, ny, start, end);
        }
    }

    mask
}

// ---------------------------------------------------------------------------
// RGBA helpers
// ---------------------------------------------------------------------------

fn read_rgba(pixels: &[u8], width: u32, x: i32, y: i32) -> [u8; 4] {
    let offset = ((y as u32 * width + x as u32) * 4) as usize;
    [pixels[offset], pixels[offset + 1], pixels[offset + 2], pixels[offset + 3]]
}

fn matches_rgba(pixels: &[u8], width: u32, x: i32, y: i32, seed: &[u8; 4], tol: i16) -> bool {
    let px = read_rgba(pixels, width, x, y);
    (px[0] as i16 - seed[0] as i16).abs() <= tol
        && (px[1] as i16 - seed[1] as i16).abs() <= tol
        && (px[2] as i16 - seed[2] as i16).abs() <= tol
        && (px[3] as i16 - seed[3] as i16).abs() <= tol
}

fn find_segment_rgba(
    pixels: &[u8], width: u32, canvas_w: i32,
    seed: &[u8; 4], tol: i16, x: i32, y: i32,
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
    pixels: &[u8], width: u32, canvas_w: i32,
    seed: &[u8; 4], tol: i16,
    mask: &mut [u8],
    queue: &mut VecDeque<(i32, i32, i32)>,
    y: i32, start: i32, end: i32,
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
    pixels: &[u8], width: u32, canvas_w: i32,
    seed: u8, tol: i16, x: i32, y: i32,
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
    pixels: &[u8], width: u32, canvas_w: i32,
    seed: u8, tol: i16,
    mask: &mut [u8],
    queue: &mut VecDeque<(i32, i32, i32)>,
    y: i32, start: i32, end: i32,
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
                pixels[offset] = 255;     // R
                pixels[offset + 3] = 255; // A
            }
        }

        // Fill from (0,0) — should fill the 2×2 red area.
        let mask = flood_fill_rgba(&pixels, 4, 4, 0, 0, 0);
        assert_eq!(mask[0], 255); // (0,0)
        assert_eq!(mask[1], 255); // (1,0)
        assert_eq!(mask[4], 255); // (0,1)
        assert_eq!(mask[5], 255); // (1,1)
        assert_eq!(mask[2], 0);   // (2,0) — transparent, not matching
        assert_eq!(mask[8], 0);   // (0,2) — transparent

        // Fill from (3,3) — should fill all transparent pixels.
        let mask = flood_fill_rgba(&pixels, 4, 4, 3, 3, 0);
        assert_eq!(mask[0], 0);   // (0,0) — red, not matching
        assert_eq!(mask[2], 255); // (2,0) — transparent
        assert_eq!(mask[15], 255); // (3,3) — transparent
    }

    #[test]
    fn flood_fill_r8_basic() {
        // 4×4 R8 image: top-left 2×2 is 255, rest is 0.
        let mut pixels = vec![0u8; 4 * 4];
        pixels[0] = 255; pixels[1] = 255;
        pixels[4] = 255; pixels[5] = 255;

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
