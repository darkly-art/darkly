//! Selection-mask rasterization and contour extraction over flat R8 buffers.
//!
//! Selections live on the GPU as single-channel R8 textures; these functions
//! are the CPU side of that. Rasterization writes a shape into a tight-bounds
//! buffer the caller uploads as a texture subregion via `queue.write_texture()`,
//! and contour extraction reads a mask's outline back out of a GPU readback as
//! polylines for the marching-ants overlay.
//!
//! Shape rasterization uses the shared SDF functions from `sdf.rs`: the
//! engine's selection filter hands an SDF closure to [`rasterize_sdf_r8`],
//! which evaluates it at each pixel center and writes coverage into the
//! buffer. Freehand and polygon shapes take the scanline path
//! ([`rasterize_polygon_r8`]) instead, which needs no per-pixel distance.

use crate::coord::WindowRect;

// ---------------------------------------------------------------------------
// SDF rasterization
// ---------------------------------------------------------------------------

/// Result of rasterizing an SDF shape to a flat R8 buffer.
/// Contains only the tight bounding region, not the full canvas.
pub struct RasterizedMask {
    /// R8 pixel data, `region_w * region_h` bytes.
    pub data: Vec<u8>,
    /// Origin of the region in canvas coordinates.
    pub x: u32,
    pub y: u32,
    /// Dimensions of the region.
    pub width: u32,
    pub height: u32,
}

/// Rasterize an SDF shape into a tight-bounds R8 buffer.
///
/// Returns only the region that the shape covers (plus margin for AA/feather),
/// clamped to canvas bounds. The caller uploads this as a subregion of the
/// GPU texture via `queue.write_texture()` with an origin offset.
///
/// - `canvas_width`, `canvas_height`: full canvas dimensions (for clamping)
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
) -> RasterizedMask {
    let (bx, by, bw, bh) = bounds;

    let margin = if feather > 0.0 {
        feather.ceil() as i32
    } else if antialias {
        1
    } else {
        0
    };

    // Tight output region: the margin-expanded shape box clamped to the window.
    // A shape entirely off-window — or a reversed-drag negative bw/bh — has no
    // overlap, so `intersect` yields `None` and the mask is empty.
    let window = WindowRect::from_xywh(0, 0, canvas_width, canvas_height);
    let region = match WindowRect::from_corners(
        bx - margin,
        by - margin,
        bx + bw + margin,
        by + bh + margin,
    )
    .intersect(window)
    {
        Some(r) => r,
        None => {
            return RasterizedMask {
                data: Vec::new(),
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }
        }
    };

    let (x0, y0) = (region.x0() as u32, region.y0() as u32);
    let (rw, rh) = (region.width, region.height);
    let (x1, y1) = (x0 + rw, y0 + rh);

    let mut pixels = vec![0u8; (rw * rh) as usize];

    for py in y0..y1 {
        for px in x0..x1 {
            let sdf = sdf_fn(px as f32 + 0.5, py as f32 + 0.5);
            let coverage = crate::sdf::sdf_coverage(sdf, antialias, feather);
            if coverage > 0.0 {
                pixels[((py - y0) * rw + (px - x0)) as usize] = (coverage * 255.0) as u8;
            }
        }
    }

    RasterizedMask {
        data: pixels,
        x: x0,
        y: y0,
        width: rw,
        height: rh,
    }
}

// ---------------------------------------------------------------------------
// Scanline polygon rasterization (no SDF)
// ---------------------------------------------------------------------------

/// Rasterize a polygon into a tight-bounds R8 buffer using scanline fill.
///
/// O(height × edges + pixels) — no per-pixel distance computation.
/// Antialiasing uses 4× vertical supersampling.
pub fn rasterize_polygon_r8(
    canvas_width: u32,
    canvas_height: u32,
    vertices: &[[f32; 2]],
    antialias: bool,
) -> RasterizedMask {
    if vertices.len() < 3 {
        return RasterizedMask {
            data: Vec::new(),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
    }

    // Bounding box.
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for v in vertices {
        min_x = min_x.min(v[0]);
        min_y = min_y.min(v[1]);
        max_x = max_x.max(v[0]);
        max_y = max_y.max(v[1]);
    }

    // Tight output region: the margin-expanded vertex bbox clamped to the
    // window. A polygon entirely off-window has no overlap, so `clamp_f32`
    // yields `None` and the mask is empty.
    let margin = if antialias { 1.0 } else { 0.0 };
    let window = WindowRect::from_xywh(0, 0, canvas_width, canvas_height);
    let region = match window.clamp_f32(
        min_x - margin,
        min_y - margin,
        max_x + margin + 1.0,
        max_y + margin + 1.0,
    ) {
        Some(r) => r,
        None => {
            return RasterizedMask {
                data: Vec::new(),
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }
        }
    };
    let (x0, y0) = (region.x0() as u32, region.y0() as u32);
    let (rw, rh) = (region.width, region.height);
    let (x1, y1) = (x0 + rw, y0 + rh);

    let n = vertices.len();
    let sub_samples: &[f32] = if antialias {
        &[0.125, 0.375, 0.625, 0.875]
    } else {
        &[0.5]
    };
    let scale = if antialias {
        255.0 / sub_samples.len() as f32
    } else {
        255.0
    };

    // Accumulator: one u8 per pixel for non-AA, one u16 per pixel for AA.
    let mut accum = vec![0u16; (rw * rh) as usize];
    let mut intersections = Vec::with_capacity(n / 2 + 4);

    for py in y0..y1 {
        let local_y = (py - y0) as usize;

        for &sub_offset in sub_samples {
            let scan_y = py as f32 + sub_offset;

            // Compute edge intersections with this scanline.
            intersections.clear();
            let mut j = n - 1;
            for i in 0..n {
                let yi = vertices[i][1];
                let yj = vertices[j][1];

                // Edge crosses scanline? (one endpoint strictly above, one at or below)
                if (yi <= scan_y && yj > scan_y) || (yj <= scan_y && yi > scan_y) {
                    let t = (scan_y - yi) / (yj - yi);
                    let x = vertices[i][0] + t * (vertices[j][0] - vertices[i][0]);
                    intersections.push(x);
                }
                j = i;
            }

            // Sort intersections.
            intersections.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());

            // Fill between pairs (even-odd rule).
            for pair in intersections.as_chunks::<2>().0 {
                let xl = pair[0];
                let xr = pair[1];

                // Integer pixel range fully inside the span.
                // Clamp BOTH endpoints into `[x0, x1]` before the `as u32` cast.
                // A span entirely off the left edge has a negative `xr`; without
                // the lower clamp `(xr.floor() + 1) as u32` wraps to ~4 billion and
                // the inner loop walks off `accum`. A fully-off span now yields a
                // reversed (empty) range instead.
                let px_start = (xl.ceil() as i32).clamp(x0 as i32, x1 as i32) as u32;
                let px_end = (xr.floor() as i32 + 1).clamp(x0 as i32, x1 as i32) as u32;

                for px in px_start..px_end {
                    accum[local_y * rw as usize + (px - x0) as usize] += 1;
                }

                // Sub-pixel coverage at left edge.
                if antialias {
                    let left_px = (xl.floor() as i32).max(x0 as i32) as u32;
                    if left_px < px_start && left_px >= x0 && left_px < x1 {
                        // Fraction of pixel that's inside: right edge of pixel minus intersection x
                        let coverage = (left_px as f32 + 1.0 - xl).clamp(0.0, 1.0);
                        accum[local_y * rw as usize + (left_px - x0) as usize] +=
                            (coverage * 1.0) as u16; // each sub-sample contributes fractionally
                    }
                    // Sub-pixel coverage at right edge.
                    let right_px = (xr.floor() as i32).max(x0 as i32) as u32;
                    if right_px >= px_end && right_px >= x0 && right_px < x1 {
                        let coverage = (xr - right_px as f32).clamp(0.0, 1.0);
                        accum[local_y * rw as usize + (right_px - x0) as usize] +=
                            (coverage * 1.0) as u16;
                    }
                }
            }
        }
    }

    // Convert accumulator to R8.
    let data: Vec<u8> = accum
        .iter()
        .map(|&v| (v as f32 * scale).round().min(255.0) as u8)
        .collect();

    RasterizedMask {
        data,
        x: x0,
        y: y0,
        width: rw,
        height: rh,
    }
}

// ---------------------------------------------------------------------------
// Contour extraction
// ---------------------------------------------------------------------------

/// Extract contour segments from a flat R8 buffer using marching squares.
///
/// Returns independent segments in canvas pixel coordinates, collinear runs
/// merged. Use [`contour_polylines_r8`] instead when the consumer needs
/// continuity along the contour.
pub fn contour_segments_r8(
    pixels: &[u8],
    width: u32,
    height: u32,
    threshold: u8,
) -> Vec<([f32; 2], [f32; 2])> {
    let segments = contour_raw_segments_r8(pixels, width, height, threshold);
    simplify_segments(merge_collinear(segments))
}

/// Same as [`contour_segments_r8`] but returns chained polylines (one per connected
/// contour) instead of a flat list of independent segments. Each inner `Vec` is a
/// sequence of points forming a polyline: points `[p0, p1, ..., pN]` describe `N`
/// connected segments. Closed contours have `p0 == pN` (within float tolerance).
///
/// Use this when the consumer needs continuity along the contour (e.g. cumulative
/// arc length for dash-pattern phase). Otherwise [`contour_segments_r8`] is simpler.
pub fn contour_polylines_r8(
    pixels: &[u8],
    width: u32,
    height: u32,
    threshold: u8,
) -> Vec<Vec<[f32; 2]>> {
    let segments = contour_raw_segments_r8(pixels, width, height, threshold);
    build_polylines(merge_collinear(segments))
}

/// Marching-squares pass shared by [`contour_segments_r8`] and
/// [`contour_polylines_r8`]. Returns the raw per-cell segments before merging.
fn contour_raw_segments_r8(
    pixels: &[u8],
    width: u32,
    height: u32,
    threshold: u8,
) -> Vec<([f32; 2], [f32; 2])> {
    let [bx, by, bw, bh] = match pixel_bounds_r8(pixels, width, height) {
        Some(b) => b,
        None => return Vec::new(),
    };

    // Range over the cells straddling the bounds, including the virtual
    // "outside = 0" row/column at index -1 and at the buffer's far edge. A mask
    // that fills to the canvas border (e.g. an inverted selection) has its
    // boundary transition only against those out-of-range samples, so clamping
    // the loop to [0, width-1] would drop the entire border contour. The
    // `sample` closure returns 0 for out-of-range coords, so iterating past the
    // edge is safe.
    let px_min = bx as i32 - 1;
    let py_min = by as i32 - 1;
    let px_max = (bx + bw) as i32;
    let py_max = (by + bh) as i32;

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
                1 => segments.push((l, t)),
                2 => segments.push((t, r)),
                3 => segments.push((l, r)),
                4 => segments.push((b, l)),
                5 => segments.push((b, t)),
                6 => {
                    segments.push((t, r));
                    segments.push((b, l));
                }
                7 => segments.push((b, r)),
                8 => segments.push((r, b)),
                9 => {
                    segments.push((l, t));
                    segments.push((r, b));
                }
                10 => segments.push((t, b)),
                11 => segments.push((l, b)),
                12 => segments.push((r, l)),
                13 => segments.push((r, t)),
                14 => segments.push((t, l)),
                _ => unreachable!(),
            }
        }
    }

    segments
}

/// Compute tight pixel bounding box from a flat R8 buffer.
/// Returns `[x, y, w, h]` or None if all pixels are zero.
pub fn pixel_bounds_r8(pixels: &[u8], width: u32, height: u32) -> Option<[u32; 4]> {
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut found = false;

    for y in 0..height {
        for x in 0..width {
            if pixels[(y * width + x) as usize] > 0 {
                found = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    // Only subtract once we know a set pixel exists — guarantees max >= min on
    // both axes. A zero-dimension or all-zero buffer simply finds nothing.
    if found {
        Some([min_x, min_y, max_x - min_x + 1, max_y - min_y + 1])
    } else {
        None
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
    fn key(v: f32) -> i32 {
        v.to_bits() as i32
    }

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
            horiz
                .entry((key(a[1]), reversed))
                .or_default()
                .push((lo, hi));
        } else if a[0] == b[0] {
            let reversed = a[1] > b[1];
            let (lo, hi) = if reversed { (b[1], a[1]) } else { (a[1], b[1]) };
            vert.entry((key(a[0]), reversed))
                .or_default()
                .push((lo, hi));
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
    result
}

/// Chain independent segments into polylines, then optionally simplify with
/// Ramer-Douglas-Peucker. Reduces curved contours (ellipses, polygons) from
/// hundreds of segments to tens while preserving shape within ±1px.
///
/// Returns one polyline per connected component. Closed loops have first ≈ last.
/// RDP is skipped entirely below a segment-count threshold — small selections
/// don't benefit from it.
fn build_polylines(segments: Vec<([f32; 2], [f32; 2])>) -> Vec<Vec<[f32; 2]>> {
    if segments.is_empty() {
        return Vec::new();
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
        adj.entry(qkey(*b)).or_default().push((i, true)); // true = end
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
            let next = adj
                .get(&key)
                .and_then(|neighbors| neighbors.iter().find(|&&(idx, _)| !used[idx]));
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
            let next = adj
                .get(&key)
                .and_then(|neighbors| neighbors.iter().find(|&&(idx, _)| !used[idx]));
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

    // Below this segment count, RDP isn't worth the cost.
    if segments.len() <= 32 {
        return chains;
    }

    // Simplify each chain with Ramer-Douglas-Peucker (epsilon = 1.0 px).
    chains.into_iter().map(|c| rdp_simplify(&c, 1.0)).collect()
}

/// Flat-segment view of [`build_polylines`]. Kept for callers that don't need
/// polyline structure (e.g. older overlay code paths, tests).
fn simplify_segments(segments: Vec<([f32; 2], [f32; 2])>) -> Vec<([f32; 2], [f32; 2])> {
    let polylines = build_polylines(segments);
    let mut result = Vec::new();
    for poly in &polylines {
        for w in poly.windows(2) {
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // --- off-canvas bounds (regression: subtract-with-overflow in mask.rs) ---

    #[test]
    fn rasterize_sdf_r8_fully_below_right_is_empty() {
        // Shape entirely below-right of the canvas: y0 > y1 used to underflow.
        let mask = crate::mask::rasterize_sdf_r8(
            100,
            100,
            (5000, 5000, 50, 50),
            |px, py| crate::sdf::sdf_rect(px, py, 5025.0, 5025.0, 25.0, 25.0),
            true,
            0.0,
        );
        assert_eq!(mask.width, 0);
        assert_eq!(mask.height, 0);
    }

    #[test]
    fn rasterize_sdf_r8_fully_above_left_is_empty() {
        // Negative bounds: (bx + bw + margin) used to wrap to a huge u32 and
        // produce a bogus full-canvas region. Must be empty instead.
        let mask = crate::mask::rasterize_sdf_r8(
            100,
            100,
            (-5000, -5000, 50, 50),
            |px, py| crate::sdf::sdf_rect(px, py, -4975.0, -4975.0, 25.0, 25.0),
            true,
            0.0,
        );
        assert_eq!(mask.width, 0);
        assert_eq!(mask.height, 0);
    }

    #[test]
    fn rasterize_sdf_r8_partial_clamps_to_canvas() {
        // Shape straddling the right/bottom edge: region is non-empty but
        // clamped to the canvas so x+width / y+height never exceed it.
        let mask = crate::mask::rasterize_sdf_r8(
            100,
            100,
            (80, 80, 50, 50),
            |px, py| crate::sdf::sdf_rect(px, py, 105.0, 105.0, 25.0, 25.0),
            false,
            0.0,
        );
        assert!(mask.width > 0 && mask.height > 0);
        assert!(mask.x + mask.width <= 100);
        assert!(mask.y + mask.height <= 100);
    }

    #[test]
    fn pixel_bounds_r8_zero_dimension_is_none() {
        // A zero-width (or zero-height) buffer has no pixels; the old guard
        // only checked the x axis, so `max_y - min_y` underflowed. Must be None.
        assert!(crate::mask::pixel_bounds_r8(&[], 0, 5).is_none());
        assert!(crate::mask::pixel_bounds_r8(&[], 5, 0).is_none());
        assert!(crate::mask::pixel_bounds_r8(&[], 0, 0).is_none());
    }

    #[test]
    fn pixel_bounds_r8_all_zero_is_none() {
        let data = vec![0u8; 4 * 4];
        assert!(crate::mask::pixel_bounds_r8(&data, 4, 4).is_none());
    }

    #[test]
    fn rasterize_polygon_r8_fully_off_canvas_is_empty() {
        // All vertices off-canvas: rw = x1 - x0 used to underflow before the
        // zero-guard could fire.
        let verts = [[5000.0, 5000.0], [5050.0, 5000.0], [5025.0, 5050.0]];
        let mask = crate::mask::rasterize_polygon_r8(100, 100, &verts, true);
        assert_eq!(mask.width, 0);
        assert_eq!(mask.height, 0);
    }

    #[test]
    fn rasterize_polygon_r8_negative_span_does_not_panic() {
        // A thin triangle leaning off the left edge: on lower scanlines the fill
        // span is entirely negative (xr < 0). `(xr.floor() as i32 + 1) as u32` used
        // to wrap to ~4 billion and run the scanline loop off the end of `accum`.
        let verts = [[0.0_f32, 0.0], [10.0, 0.0], [-100.0, 100.0]];
        let mask = crate::mask::rasterize_polygon_r8(100, 100, &verts, true);
        assert!(mask.x + mask.width <= 100);
        assert!(mask.y + mask.height <= 100);
    }

    // --- contour_polylines_r8 ---
    //
    // Regression for the marching-ants flicker at low zoom: dashed-line phase
    // continuity across a polyline requires the contour to come back chained
    // (one connected sequence of points), with each segment's endpoint matching
    // the next segment's startpoint exactly. If chains break or segments come
    // back out of order, cumulative arc length is wrong and dashes flicker.

    fn rect_buffer_r8(stride: u32, rect_x: u32, rect_y: u32, rect_w: u32, rect_h: u32) -> Vec<u8> {
        let mut buf = vec![0u8; (stride * stride) as usize];
        for y in rect_y..rect_y + rect_h {
            for x in rect_x..rect_x + rect_w {
                buf[(y * stride + x) as usize] = 255;
            }
        }
        buf
    }

    #[test]
    fn polylines_empty_mask() {
        let buf = vec![0u8; 16 * 16];
        assert!(crate::mask::contour_polylines_r8(&buf, 16, 16, 127).is_empty());
    }

    #[test]
    fn polylines_rect_forms_closed_loop() {
        let buf = rect_buffer_r8(20, 5, 5, 8, 8);
        let polylines = crate::mask::contour_polylines_r8(&buf, 20, 20, 127);
        assert_eq!(
            polylines.len(),
            1,
            "a single filled rectangle should produce one polyline, got {}",
            polylines.len()
        );
        let poly = &polylines[0];
        assert!(
            poly.len() >= 4,
            "expected at least 4 points, got {}",
            poly.len()
        );

        // Closed loop: first ≈ last (within float tolerance).
        let first = poly[0];
        let last = *poly.last().unwrap();
        let dx = first[0] - last[0];
        let dy = first[1] - last[1];
        assert!(
            dx * dx + dy * dy < 1e-3,
            "polyline should be a closed loop: first={:?} last={:?}",
            first,
            last
        );
    }

    #[test]
    fn polylines_segments_are_chained() {
        // Adjacent segments within a polyline must share endpoints — that's the
        // invariant the dash-phase math relies on.
        let buf = rect_buffer_r8(20, 5, 5, 8, 8);
        let polylines = crate::mask::contour_polylines_r8(&buf, 20, 20, 127);
        for poly in &polylines {
            for i in 1..poly.len() {
                // The point at index i is shared between segment (i-1) and segment i,
                // so the chain trivially links — what we're really asserting is that
                // every consecutive pair forms a non-degenerate segment.
                let a = poly[i - 1];
                let b = poly[i];
                let len_sq = (b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2);
                assert!(len_sq > 0.0, "degenerate segment in polyline at index {i}");
            }
        }
    }

    #[test]
    fn polylines_full_canvas_traces_border() {
        // Regression: a mask filled to the canvas edge (e.g. the result of
        // inverting a selection) must produce a contour along the canvas border.
        // The marching-squares pass has to evaluate the cells straddling the
        // virtual "outside = 0" row/column at index -1 and width-1; clamping the
        // loop to [0, width-1] dropped them and left the border ants missing.
        let w = 12u32;
        let h = 12u32;
        let buf = vec![255u8; (w * h) as usize];
        let polylines = crate::mask::contour_polylines_r8(&buf, w, h, 127);
        assert_eq!(
            polylines.len(),
            1,
            "a fully-filled mask should trace one border loop, got {}",
            polylines.len()
        );
        let mut arc = 0.0_f32;
        for win in polylines[0].windows(2) {
            let dx = win[1][0] - win[0][0];
            let dy = win[1][1] - win[0][1];
            arc += (dx * dx + dy * dy).sqrt();
        }
        let expected = 2.0 * (w as f32 + h as f32);
        assert!(
            (arc - expected).abs() < 4.0,
            "border perimeter ≈ {expected}, got {arc}"
        );
    }

    #[test]
    fn polylines_inverted_rect_has_border_and_hole() {
        // The exact marching-ants invert case: a full-canvas mask with a
        // rectangular hole punched out (selection inverted). Expect two loops —
        // the canvas border and the inner hole — not just the hole.
        let stride = 20u32;
        let mut buf = vec![255u8; (stride * stride) as usize];
        for y in 6..12 {
            for x in 6..12 {
                buf[(y * stride + x) as usize] = 0;
            }
        }
        let polylines = crate::mask::contour_polylines_r8(&buf, stride, stride, 127);
        assert_eq!(
            polylines.len(),
            2,
            "inverted-rect mask should yield a border loop and a hole loop, got {}",
            polylines.len()
        );
    }

    #[test]
    fn polylines_perimeter_matches_rect_size() {
        // Walking the polyline should accumulate to roughly the perimeter of
        // the filled rectangle. Marching squares places contour points at cell
        // crossings (half-pixel offsets from filled pixels), so we expect
        // perimeter ≈ 2 * (w + h) ± a couple of pixels of corner rounding.
        let w = 8.0;
        let h = 8.0;
        let buf = rect_buffer_r8(20, 5, 5, w as u32, h as u32);
        let polylines = crate::mask::contour_polylines_r8(&buf, 20, 20, 127);
        assert_eq!(polylines.len(), 1);

        let mut arc = 0.0_f32;
        for win in polylines[0].windows(2) {
            let dx = win[1][0] - win[0][0];
            let dy = win[1][1] - win[0][1];
            arc += (dx * dx + dy * dy).sqrt();
        }
        let expected = 2.0 * (w + h);
        assert!(
            (arc - expected).abs() < 4.0,
            "expected perimeter ≈ {expected}, got {arc}"
        );
    }
}
