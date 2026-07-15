//! Projective (3×3 homography) math — the perspective generalization of the
//! affine helpers in [`super`]. Like its sibling, this module is
//! **consumer-agnostic**: pure `f32` math, no `gpu`/`layer`/`document` deps.
//!
//! Affine is the special case of a [`Mat3`] whose bottom row is `[0, 0, 1]`
//! (so `w ≡ 1` and the perspective divide is a no-op). The GPU commit path
//! consumes [`Mat3`] uniformly — perspective falls out of the same shader.
//!
//! The rect→quad homography ([`homography_from_corners`]) is a port of GIMP's
//! `gimp_transform_matrix_perspective` (`app/core/gimp-transform-utils.c`),
//! itself the closed-form unit-square→quad mapping from Paul Heckbert,
//! "Projective Mappings for Image Warping" (1989). No linear solve is needed
//! because our source is always an axis-aligned rect.

/// 3×3 projective matrix stored row-major as `[m00, m01, m02, m10, m11, m12,
/// m20, m21, m22]`. Transforms point `(x, y)` →
/// `((m00·x + m01·y + m02) / w, (m10·x + m11·y + m12) / w)` where
/// `w = m20·x + m21·y + m22`.
///
/// **CONTRACT:** this layout is mirrored by the TS gizmo
/// (`frontend/src/tools/transform_projective.ts`) and consumed by the WGSL
/// commit shader through the inverse. The three are pinned by
/// [`tests::homography_contract`] here and a mirrored vitest.
pub type Mat3 = [f32; 9];

/// Identity projective matrix.
pub const MAT3_IDENTITY: Mat3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

/// Widen a 2D affine `[a, b, tx, c, d, ty]` to a [`Mat3`] with bottom row
/// `[0, 0, 1]`.
pub fn affine_to_mat3(m: &super::Affine2D) -> Mat3 {
    let [a, b, tx, c, d, ty] = *m;
    [a, b, tx, c, d, ty, 0.0, 0.0, 1.0]
}

/// Multiply two projective matrices: `result = a · b` (apply `b` first).
pub fn mat3_multiply(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [0.0f32; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    out
}

/// Transform a point by a projective matrix, applying the perspective divide.
/// Returns `(x/w, y/w)`; `w` near zero yields a point at infinity (large
/// values) — callers that build bounds must clamp (see
/// `FloatingContent::transformed_bounds`).
pub fn mat3_apply(m: &Mat3, x: f32, y: f32) -> (f32, f32) {
    let px = m[0] * x + m[1] * y + m[2];
    let py = m[3] * x + m[4] * y + m[5];
    let w = m[6] * x + m[7] * y + m[8];
    (px / w, py / w)
}

/// Invert a projective matrix via the adjugate / determinant. Returns `None`
/// when the matrix is singular (`det ≈ 0`).
pub fn mat3_inverse(m: &Mat3) -> Option<Mat3> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let a00 = e * i - f * h;
    let a01 = f * g - d * i;
    let a02 = d * h - e * g;
    let det = a * a00 + b * a01 + c * a02;
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    // adjugate (transpose of cofactor matrix) × 1/det
    Some([
        a00 * inv,
        (c * h - b * i) * inv,
        (b * f - c * e) * inv,
        a01 * inv,
        (a * i - c * g) * inv,
        (c * d - a * f) * inv,
        a02 * inv,
        (b * g - a * h) * inv,
        (a * e - b * d) * inv,
    ])
}

/// Build the homography mapping the source rect `[0, src_w] × [0, src_h]` onto
/// four destination corners `[TL, TR, BR, BL]` (the source rect's corners at
/// `(0,0)`, `(w,0)`, `(w,h)`, `(0,h)`).
///
/// Returns `None` on degenerate input — a collapsed quad, or one that folds a
/// corner behind the camera (a destination corner whose homogeneous `w` is
/// non-positive). Both the live drag preview and the final commit gate on
/// this, so a mid-drag corner sweeping toward infinity can't produce a garbage
/// matrix.
///
/// Ported from GIMP `gimp_transform_matrix_perspective` (unit-square→quad,
/// Heckbert closed form), specialized to our always-axis-aligned source rect.
pub fn homography_from_corners(src_w: f32, src_h: f32, corners: [(f32, f32); 4]) -> Option<Mat3> {
    if src_w <= 0.0 || src_h <= 0.0 {
        return None;
    }
    // GIMP corner naming: t1 at unit (0,0), t2 at (1,0), t3 at (0,1), t4 at
    // (1,1). Our source rect normalized to the unit square maps
    // TL→(0,0), TR→(1,0), BR→(1,1), BL→(0,1), so:
    let (t_x1, t_y1) = corners[0]; // TL → (0,0)
    let (t_x2, t_y2) = corners[1]; // TR → (1,0)
    let (t_x4, t_y4) = corners[2]; // BR → (1,1)
    let (t_x3, t_y3) = corners[3]; // BL → (0,1)

    let dx1 = t_x2 - t_x4;
    let dx2 = t_x3 - t_x4;
    let dx3 = t_x1 - t_x2 + t_x4 - t_x3;
    let dy1 = t_y2 - t_y4;
    let dy2 = t_y3 - t_y4;
    let dy3 = t_y1 - t_y2 + t_y4 - t_y3;

    // trafo = unit-square → quad (row-major 3×3).
    let trafo: Mat3 = if dx3.abs() < 1e-12 && dy3.abs() < 1e-12 {
        // Affine (parallelogram) — no perspective term.
        [
            t_x2 - t_x1,
            t_x3 - t_x1,
            t_x1,
            t_y2 - t_y1,
            t_y3 - t_y1,
            t_y1,
            0.0,
            0.0,
            1.0,
        ]
    } else {
        let det2 = dx1 * dy2 - dy1 * dx2;
        if det2.abs() < 1e-12 {
            return None;
        }
        let g = (dx3 * dy2 - dy3 * dx2) / det2;
        let h = (dx1 * dy3 - dy1 * dx3) / det2;
        [
            t_x2 - t_x1 + g * t_x2,
            t_x3 - t_x1 + h * t_x3,
            t_x1,
            t_y2 - t_y1 + g * t_y2,
            t_y3 - t_y1 + h * t_y3,
            t_y1,
            g,
            h,
            1.0,
        ]
    };

    // The homogeneous w at the four unit-square corners is the bottom row
    // evaluated there: 1, 1+g, 1+h, 1+g+h. All must share a sign (here:
    // stay positive) or a corner has folded behind the camera.
    let g = trafo[6];
    let h = trafo[7];
    for w in [1.0, 1.0 + g, 1.0 + h, 1.0 + g + h] {
        if w <= 1e-6 {
            return None;
        }
    }

    // Compose with the source-rect → unit-square normalization (scale by
    // 1/w, 1/h). trafo · scale(1/src_w, 1/src_h).
    let norm: Mat3 = [1.0 / src_w, 0.0, 0.0, 0.0, 1.0 / src_h, 0.0, 0.0, 0.0, 1.0];
    let m = mat3_multiply(&trafo, &norm);
    mat3_inverse(&m)?; // reject anything still non-invertible
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-3, "expected {b}, got {a}");
    }

    fn approx_pt(p: (f32, f32), x: f32, y: f32) {
        approx(p.0, x);
        approx(p.1, y);
    }

    /// Pins the row-major [`Mat3`] convention + the rect→quad mapping. A
    /// mirrored vitest transforms the SAME corners and must agree.
    #[test]
    fn homography_contract() {
        // A trapezoid: top edge narrowed (classic vanishing-point look).
        let w = 100.0;
        let h = 80.0;
        let corners = [(20.0, 0.0), (80.0, 0.0), (100.0, 80.0), (0.0, 80.0)];
        let m = homography_from_corners(w, h, corners).expect("non-degenerate");
        // The four source corners must land on the requested dest corners.
        approx_pt(mat3_apply(&m, 0.0, 0.0), 20.0, 0.0);
        approx_pt(mat3_apply(&m, w, 0.0), 80.0, 0.0);
        approx_pt(mat3_apply(&m, w, h), 100.0, 80.0);
        approx_pt(mat3_apply(&m, 0.0, h), 0.0, 80.0);
    }

    #[test]
    fn inverse_round_trips() {
        let corners = [(10.0, 5.0), (90.0, -10.0), (110.0, 70.0), (-5.0, 95.0)];
        let m = homography_from_corners(100.0, 80.0, corners).expect("non-degenerate");
        let inv = mat3_inverse(&m).expect("invertible");
        let p = mat3_apply(&m, 37.0, 19.0);
        let back = mat3_apply(&inv, p.0, p.1);
        approx_pt(back, 37.0, 19.0);
    }

    #[test]
    fn affine_widens_losslessly() {
        let aff: super::super::Affine2D = [2.0, 0.0, 10.0, 0.0, 3.0, 20.0];
        let m = affine_to_mat3(&aff);
        let a = super::super::affine_transform(&aff, 5.0, 7.0);
        let b = mat3_apply(&m, 5.0, 7.0);
        approx_pt(b, a.0, a.1);
        // Bottom row is the affine sentinel.
        approx(m[6], 0.0);
        approx(m[7], 0.0);
        approx(m[8], 1.0);
    }

    #[test]
    fn identity_maps_rect_corners() {
        // A rect mapped to its own corners reproduces the identity-equivalent
        // (here a pure translate/scale, no perspective).
        let w = 64.0;
        let h = 48.0;
        let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
        let m = homography_from_corners(w, h, corners).expect("non-degenerate");
        approx(m[6], 0.0);
        approx(m[7], 0.0);
        approx_pt(mat3_apply(&m, w / 2.0, h / 2.0), w / 2.0, h / 2.0);
    }

    #[test]
    fn degenerate_returns_none() {
        // All four corners collapsed to a point.
        assert!(homography_from_corners(100.0, 80.0, [(0.0, 0.0); 4]).is_none());
        // Zero source extent.
        let unit = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        assert!(homography_from_corners(0.0, 80.0, unit).is_none());
    }
}
