//! Shared signed distance functions for mask rasterization and overlay hit testing.
//!
//! These pure-math functions compute signed distances from a point to geometric
//! primitives. Negative = inside, positive = outside (except `sdf_segment` which
//! returns unsigned distance). The same formulas appear in `shaders/overlay.wgsl`
//! for GPU rendering — one is the source of truth for the other.
//!
//! The `sdf_coverage` function converts an SDF value to alpha coverage (0.0–1.0)
//! with three modes: hard edge, antialiased (1px smoothstep), or feathered.

// ---------------------------------------------------------------------------
// Signed distance functions
// ---------------------------------------------------------------------------

/// Unsigned distance from point to line segment.
pub fn sdf_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let pax = px - ax;
    let pay = py - ay;
    let bax = bx - ax;
    let bay = by - ay;
    let dot_ba = bax * bax + bay * bay;
    if dot_ba < 1e-12 {
        return pax.hypot(pay);
    }
    let t = ((pax * bax + pay * bay) / dot_ba).clamp(0.0, 1.0);
    let dx = pax - bax * t;
    let dy = pay - bay * t;
    dx.hypot(dy)
}

/// Signed distance to filled circle. Negative inside, positive outside.
pub fn sdf_circle(px: f32, py: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let dx = px - cx;
    let dy = py - cy;
    dx.hypot(dy) - r
}

/// Signed distance to filled axis-aligned rectangle. Negative inside, positive outside.
///
/// Parameters: center (cx, cy) and half-extents (half_w, half_h).
pub fn sdf_rect(px: f32, py: f32, cx: f32, cy: f32, half_w: f32, half_h: f32) -> f32 {
    let dx = (px - cx).abs() - half_w;
    let dy = (py - cy).abs() - half_h;
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    let inside = dx.max(dy).min(0.0);
    outside + inside
}

/// Signed distance to filled rounded rectangle. Negative inside, positive outside.
///
/// Parameters: center (cx, cy), half-extents (half_w, half_h), corner radius.
pub fn sdf_rounded_rect(
    px: f32,
    py: f32,
    cx: f32,
    cy: f32,
    half_w: f32,
    half_h: f32,
    corner_r: f32,
) -> f32 {
    let hw = half_w - corner_r;
    let hh = half_h - corner_r;
    let dx = (px - cx).abs() - hw;
    let dy = (py - cy).abs() - hh;
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    let inside = dx.max(dy).min(0.0);
    outside + inside - corner_r
}

/// Signed distance to filled ellipse. Negative inside, positive outside.
///
/// Uses implicit surface approximation: `f(x,y) / |∇f(x,y)|` where
/// `f = (x/rx)² + (y/ry)² - 1`. Exact on the boundary, accurate within
/// ±1px near it — sufficient for antialiased rasterization.
pub fn sdf_ellipse(px: f32, py: f32, cx: f32, cy: f32, rx: f32, ry: f32) -> f32 {
    let dx = px - cx;
    let dy = py - cy;
    // Implicit function: f = (dx/rx)² + (dy/ry)² - 1
    let f = (dx * dx) / (rx * rx) + (dy * dy) / (ry * ry) - 1.0;
    // Gradient magnitude: |∇f| = 2 * sqrt((dx/rx²)² + (dy/ry²)²)
    let gx = 2.0 * dx / (rx * rx);
    let gy = 2.0 * dy / (ry * ry);
    let grad_len = (gx * gx + gy * gy).sqrt();
    if grad_len < 1e-12 {
        // At the center of the ellipse — return negative min radius
        return -rx.min(ry);
    }
    f / grad_len
}

/// Signed distance to filled polygon. Negative inside, positive outside.
///
/// Uses the Inigo Quilez winding-number algorithm: computes minimum edge
/// distance and determines inside/outside via ray-crossing parity.
pub fn sdf_polygon(px: f32, py: f32, vertices: &[[f32; 2]]) -> f32 {
    let n = vertices.len();
    if n < 3 {
        return f32::MAX;
    }

    // Initialize with squared distance to first vertex
    let d0x = px - vertices[0][0];
    let d0y = py - vertices[0][1];
    let mut d_sq = d0x * d0x + d0y * d0y;
    let mut s = 1.0_f32;

    let mut j = n - 1;
    for i in 0..n {
        // Edge vector: v[j] - v[i]
        let ex = vertices[j][0] - vertices[i][0];
        let ey = vertices[j][1] - vertices[i][1];
        // Vector from v[i] to point
        let wx = px - vertices[i][0];
        let wy = py - vertices[i][1];

        // Closest point on edge segment
        let dot_we = wx * ex + wy * ey;
        let dot_ee = ex * ex + ey * ey;
        let t = if dot_ee > 1e-12 {
            (dot_we / dot_ee).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let bx = wx - ex * t;
        let by = wy - ey * t;
        d_sq = d_sq.min(bx * bx + by * by);

        // Winding number: does edge cross the horizontal ray going right?
        let c1 = py >= vertices[i][1];
        let c2 = py < vertices[j][1];
        let cross = ex * wy - ey * wx;
        if (c1 && c2 && cross > 0.0) || (!c1 && !c2 && cross < 0.0) {
            s = -s;
        }

        j = i;
    }

    s * d_sq.sqrt()
}

// ---------------------------------------------------------------------------
// Coverage conversion
// ---------------------------------------------------------------------------

/// Attempt to match the standard library `smoothstep` function.
/// Returns 0 for x <= edge0, 1 for x >= edge1, smooth Hermite interpolation between.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Convert a signed distance value to alpha coverage (0.0 = outside, 1.0 = inside).
///
/// Three modes:
/// - `antialias=false, feather=0`: hard edge — binary 0/1 at boundary
/// - `antialias=true, feather=0`: smooth 1px transition via smoothstep (matches overlay shader)
/// - `feather > 0`: smooth transition over `feather` pixels (antialias flag ignored)
pub fn sdf_coverage(sdf: f32, antialias: bool, feather: f32) -> f32 {
    if feather > 0.0 {
        // Feathered: smooth transition centered on boundary, width = feather
        let half = feather * 0.5;
        smoothstep(half, -half, sdf)
    } else if antialias {
        // Antialiased: 1px smoothstep transition, same as overlay.wgsl
        smoothstep(0.5, -0.5, sdf)
    } else {
        // Hard edge: binary
        if sdf <= 0.0 {
            1.0
        } else {
            0.0
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    // --- sdf_segment ---

    #[test]
    fn segment_perpendicular_distance() {
        // Point (5, 3) to horizontal segment (0,0)→(10,0): distance = 3
        let d = sdf_segment(5.0, 3.0, 0.0, 0.0, 10.0, 0.0);
        assert!((d - 3.0).abs() < EPS);
    }

    #[test]
    fn segment_endpoint_distance() {
        // Point (12, 0) to segment (0,0)→(10,0): distance = 2
        let d = sdf_segment(12.0, 0.0, 0.0, 0.0, 10.0, 0.0);
        assert!((d - 2.0).abs() < EPS);
    }

    #[test]
    fn segment_on_segment() {
        let d = sdf_segment(5.0, 0.0, 0.0, 0.0, 10.0, 0.0);
        assert!(d < EPS);
    }

    // --- sdf_circle ---

    #[test]
    fn circle_inside() {
        let d = sdf_circle(5.0, 5.0, 5.0, 5.0, 10.0);
        assert!((d - (-10.0)).abs() < EPS); // at center, distance = -radius
    }

    #[test]
    fn circle_boundary() {
        let d = sdf_circle(15.0, 5.0, 5.0, 5.0, 10.0);
        assert!(d.abs() < EPS);
    }

    #[test]
    fn circle_outside() {
        let d = sdf_circle(20.0, 5.0, 5.0, 5.0, 10.0);
        assert!((d - 5.0).abs() < EPS);
    }

    // --- sdf_rect ---

    #[test]
    fn rect_inside() {
        let d = sdf_rect(5.0, 5.0, 5.0, 5.0, 10.0, 8.0);
        assert!(d < 0.0); // inside
        assert!((d - (-8.0)).abs() < EPS); // at center, min distance to edge
    }

    #[test]
    fn rect_edge() {
        // Right edge: cx=5, half_w=10, so right edge at x=15
        let d = sdf_rect(15.0, 5.0, 5.0, 5.0, 10.0, 8.0);
        assert!(d.abs() < EPS);
    }

    #[test]
    fn rect_outside() {
        let d = sdf_rect(20.0, 5.0, 5.0, 5.0, 10.0, 8.0);
        assert!((d - 5.0).abs() < EPS);
    }

    #[test]
    fn rect_corner_outside() {
        // Corner at (15, 13). Point at (16, 14): distance = sqrt(2)
        let d = sdf_rect(16.0, 14.0, 5.0, 5.0, 10.0, 8.0);
        assert!((d - std::f32::consts::SQRT_2).abs() < EPS);
    }

    // --- sdf_ellipse ---

    #[test]
    fn ellipse_center() {
        let d = sdf_ellipse(5.0, 5.0, 5.0, 5.0, 10.0, 6.0);
        assert!(d < 0.0); // inside
    }

    #[test]
    fn ellipse_on_major_axis() {
        // Semi-major axis endpoint: (15, 5) for cx=5, rx=10
        let d = sdf_ellipse(15.0, 5.0, 5.0, 5.0, 10.0, 6.0);
        assert!(d.abs() < 0.1); // approximately on boundary
    }

    #[test]
    fn ellipse_on_minor_axis() {
        // Semi-minor axis endpoint: (5, 11) for cy=5, ry=6
        let d = sdf_ellipse(5.0, 11.0, 5.0, 5.0, 10.0, 6.0);
        assert!(d.abs() < 0.1); // approximately on boundary
    }

    #[test]
    fn ellipse_outside() {
        let d = sdf_ellipse(20.0, 5.0, 5.0, 5.0, 10.0, 6.0);
        assert!(d > 0.0);
    }

    // --- sdf_polygon ---

    #[test]
    fn polygon_triangle_inside() {
        let verts = [[0.0, 0.0], [10.0, 0.0], [5.0, 10.0]];
        let d = sdf_polygon(5.0, 3.0, &verts);
        assert!(
            d < 0.0,
            "point inside triangle should have negative SDF, got {d}"
        );
    }

    #[test]
    fn polygon_triangle_outside() {
        let verts = [[0.0, 0.0], [10.0, 0.0], [5.0, 10.0]];
        let d = sdf_polygon(0.0, 10.0, &verts);
        assert!(
            d > 0.0,
            "point outside triangle should have positive SDF, got {d}"
        );
    }

    #[test]
    fn polygon_triangle_vertex() {
        let verts = [[0.0, 0.0], [10.0, 0.0], [5.0, 10.0]];
        let d = sdf_polygon(0.0, 0.0, &verts);
        assert!(d.abs() < EPS, "distance at vertex should be ~0, got {d}");
    }

    #[test]
    fn polygon_square_inside() {
        let verts = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let d = sdf_polygon(5.0, 5.0, &verts);
        assert!(d < 0.0);
        assert!(
            (d - (-5.0)).abs() < EPS,
            "center of 10x10 square: expected -5, got {d}"
        );
    }

    #[test]
    fn polygon_square_edge() {
        let verts = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let d = sdf_polygon(10.0, 5.0, &verts);
        assert!(d.abs() < EPS, "on edge should be ~0, got {d}");
    }

    // --- sdf_coverage ---

    #[test]
    fn coverage_hard_edge() {
        assert_eq!(sdf_coverage(-1.0, false, 0.0), 1.0);
        assert_eq!(sdf_coverage(0.0, false, 0.0), 1.0); // on boundary = inside
        assert_eq!(sdf_coverage(0.01, false, 0.0), 0.0);
    }

    #[test]
    fn coverage_antialiased() {
        assert_eq!(sdf_coverage(-1.0, true, 0.0), 1.0); // deep inside
        assert_eq!(sdf_coverage(1.0, true, 0.0), 0.0); // deep outside
        let c = sdf_coverage(0.0, true, 0.0);
        assert!((c - 0.5).abs() < EPS, "on boundary should be ~0.5, got {c}");
    }

    #[test]
    fn coverage_feathered() {
        assert_eq!(sdf_coverage(-10.0, false, 4.0), 1.0); // deep inside
        assert_eq!(sdf_coverage(10.0, false, 4.0), 0.0); // deep outside
        let c = sdf_coverage(0.0, false, 4.0);
        assert!((c - 0.5).abs() < EPS, "on boundary should be ~0.5, got {c}");
        // At sdf = feather/2 = 2.0, should be ~0
        let c2 = sdf_coverage(2.0, false, 4.0);
        assert!(c2 < EPS);
        // At sdf = -feather/2 = -2.0, should be ~1
        let c3 = sdf_coverage(-2.0, false, 4.0);
        assert!((c3 - 1.0).abs() < EPS);
    }

    // --- sdf_rounded_rect ---

    #[test]
    fn rounded_rect_zero_radius_matches_rect() {
        for &(px, py) in &[(5.0, 5.0), (15.0, 5.0), (20.0, 5.0), (16.0, 14.0)] {
            let d_rect = sdf_rect(px, py, 5.0, 5.0, 10.0, 8.0);
            let d_rr = sdf_rounded_rect(px, py, 5.0, 5.0, 10.0, 8.0, 0.0);
            assert!(
                (d_rect - d_rr).abs() < EPS,
                "mismatch at ({px}, {py}): rect={d_rect}, rounded={d_rr}"
            );
        }
    }
}
