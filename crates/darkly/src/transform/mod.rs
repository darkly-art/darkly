//! Generic 2D transform record — a dependency-free helper.
//!
//! This module is **consumer-agnostic**: it knows nothing about voids, layers,
//! floating content, the compositor, or the document. It defines the affine
//! math and the [`Transform`] record (mode-tagged) that consumers *store* and
//! that the frontend gizmo *edits*. Dependencies point consumer → here, never
//! the other way: nothing in this file may reference `gpu`, `layer`,
//! `engine`, or `document` types.
//!
//! A consumer that wants to be user-transformable owns a [`Transform`] and
//! wires the gizmo to itself through a thin binding (see
//! `frontend/src/tools/transform_gizmo.ts`). The gizmo takes a bounding box +
//! the current transform + pointer input and outputs an updated [`Transform`].

// ---------------------------------------------------------------------------
// Affine math  ([a, b, tx, c, d, ty], row-major)
// ---------------------------------------------------------------------------

/// 2D affine matrix stored row-major as `[a, b, tx, c, d, ty]`.
/// Transforms point `(x, y)` → `(a*x + b*y + tx, c*x + d*y + ty)`:
///
/// ```text
/// | a  b  tx |
/// | c  d  ty |
/// | 0  0  1  |
/// ```
///
/// **CONTRACT:** this row-major `[a, b, tx, c, d, ty]` layout is mirrored by
/// the TS gizmo (`frontend/src/tools/transform_affine.ts`) and by the WGSL
/// that samples through the inverse. The three cannot share one
/// implementation — the gizmo computes interactively in JS and ships the baked
/// affine over the wire — so the layout is pinned by [`tests::affine_contract`]
/// here and a mirrored vitest on the TS side. Do not reorder the components.
pub type Affine2D = [f32; 6];

/// Identity affine: no transformation.
pub const IDENTITY: Affine2D = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

/// Compute the inverse of a 2D affine matrix.
/// Returns `None` if the matrix is singular (det ≈ 0).
pub fn affine_inverse(m: &Affine2D) -> Option<Affine2D> {
    let [a, b, tx, c, d, ty] = *m;
    let det = a * d - b * c;
    if det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    Some([
        d * inv_det,
        -b * inv_det,
        (b * ty - d * tx) * inv_det,
        -c * inv_det,
        a * inv_det,
        (c * tx - a * ty) * inv_det,
    ])
}

/// Transform a point by an affine matrix.
pub fn affine_transform(m: &Affine2D, x: f32, y: f32) -> (f32, f32) {
    let [a, b, tx, c, d, ty] = *m;
    (a * x + b * y + tx, c * x + d * y + ty)
}

/// Multiply two affine matrices: `result = a ∘ b` (apply `b` first, then `a`).
pub fn affine_multiply(a: &Affine2D, b: &Affine2D) -> Affine2D {
    [
        a[0] * b[0] + a[1] * b[3],
        a[0] * b[1] + a[1] * b[4],
        a[0] * b[2] + a[1] * b[5] + a[2],
        a[3] * b[0] + a[4] * b[3],
        a[3] * b[1] + a[4] * b[4],
        a[3] * b[2] + a[4] * b[5] + a[5],
    ]
}

/// Build a translation affine.
pub fn affine_translate(tx: f32, ty: f32) -> Affine2D {
    [1.0, 0.0, tx, 0.0, 1.0, ty]
}

/// Build a scale affine.
pub fn affine_scale(sx: f32, sy: f32) -> Affine2D {
    [sx, 0.0, 0.0, 0.0, sy, 0.0]
}

/// Build a rotation affine (angle in radians, CCW).
pub fn affine_rotate(angle: f32) -> Affine2D {
    let (s, c) = angle.sin_cos();
    [c, -s, 0.0, s, c, 0.0]
}

// ---------------------------------------------------------------------------
// Transform record — mode-tagged, serializable
// ---------------------------------------------------------------------------

/// A user-editable 2D transform. Mode-tagged so future interaction modes
/// (perspective homography, warp mesh) slot in additively without consumers
/// changing how they store or apply it.
///
/// `Basic` stores the affine directly — lossless, and exactly what the gizmo's
/// handle math produces. The `mode_tag` value crossing the WASM boundary picks
/// the frontend mode strategy.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "mode", content = "data")]
pub enum Transform {
    /// Affine: pan / scale / rotate. Stored as [`Affine2D`].
    Basic(Affine2D),
}

impl Default for Transform {
    fn default() -> Self {
        Transform::Basic(IDENTITY)
    }
}

impl Transform {
    /// The identity transform (`Basic(IDENTITY)`).
    pub const fn identity() -> Self {
        Transform::Basic(IDENTITY)
    }

    /// Wrap a raw affine as a `Basic` transform.
    pub fn from_affine(m: Affine2D) -> Self {
        Transform::Basic(m)
    }

    /// Bake to a single affine matrix. For `Basic` this is the stored matrix.
    pub fn to_affine(&self) -> Affine2D {
        match self {
            Transform::Basic(m) => *m,
        }
    }

    /// Stable numeric mode tag for the WASM boundary / gizmo mode registry.
    pub fn mode_tag(&self) -> u32 {
        match self {
            Transform::Basic(_) => 0,
        }
    }

    /// Decompose into translation / rotation / scale.
    ///
    /// **ASSUMES NO SHEAR** (translate-rotate-scale only) — lossy for sheared
    /// matrices. The gizmo never produces shear, so this is faithful for
    /// gizmo-authored transforms; it exists only to back a future numeric
    /// input panel, not to round-trip arbitrary affines.
    pub fn decompose(&self) -> Decomposed {
        let [a, b, tx, c, d, ty] = self.to_affine();
        let sx = (a * a + c * c).sqrt();
        let rotation = c.atan2(a);
        let det = a * d - b * c;
        // Signed y-scale via the determinant keeps reflections correct under
        // the no-shear assumption (sy = det / sx).
        let sy = if sx.abs() > 1e-12 { det / sx } else { 0.0 };
        Decomposed {
            offset: (tx, ty),
            rotation,
            scale: (sx, sy),
        }
    }
}

/// TRS view of a [`Transform`] — see [`Transform::decompose`] (no-shear).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decomposed {
    /// Translation `(tx, ty)`.
    pub offset: (f32, f32),
    /// Rotation in radians (CCW).
    pub rotation: f32,
    /// Scale `(sx, sy)`.
    pub scale: (f32, f32),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "expected {b}, got {a}");
    }

    /// Pins the row-major `[a, b, tx, c, d, ty]` contract. A mirrored vitest
    /// (`frontend/src/tools/__tests__/transform_affine.test.ts`) transforms the
    /// SAME matrix + point and must produce the SAME result — that's the guard
    /// against the JS/Rust conventions silently diverging.
    #[test]
    fn affine_contract() {
        // Scale 2x in x, translate (10, 20), no rotation.
        let m: Affine2D = [2.0, 0.0, 10.0, 0.0, 3.0, 20.0];
        let (x, y) = affine_transform(&m, 5.0, 7.0);
        approx(x, 2.0 * 5.0 + 10.0); // 20
        approx(y, 3.0 * 7.0 + 20.0); // 41
    }

    #[test]
    fn inverse_round_trips() {
        let m = affine_multiply(
            &affine_translate(13.0, -4.0),
            &affine_multiply(&affine_rotate(0.7), &affine_scale(2.0, 0.5)),
        );
        let inv = affine_inverse(&m).expect("invertible");
        let (x, y) = affine_transform(&m, 9.0, -3.0);
        let (rx, ry) = affine_transform(&inv, x, y);
        approx(rx, 9.0);
        approx(ry, -3.0);
    }

    #[test]
    fn multiply_applies_b_first() {
        // a ∘ b applies b first: translate-then-scale vs scale-then-translate.
        let scale = affine_scale(2.0, 2.0);
        let translate = affine_translate(1.0, 0.0);
        // scale ∘ translate: translate first (x+1), then scale (×2) → (x+1)*2
        let m = affine_multiply(&scale, &translate);
        let (x, _) = affine_transform(&m, 3.0, 0.0);
        approx(x, (3.0 + 1.0) * 2.0); // 8
    }

    #[test]
    fn default_is_identity() {
        assert_eq!(Transform::default(), Transform::Basic(IDENTITY));
        assert_eq!(Transform::identity().to_affine(), IDENTITY);
    }

    #[test]
    fn serde_round_trip_tagged() {
        let t = Transform::from_affine([1.5, 0.0, 4.0, 0.0, 2.0, -3.0]);
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("Basic"), "tagged enum: {json}");
        let back: Transform = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn decompose_trs_no_shear() {
        let angle = 0.5_f32;
        let m = affine_multiply(&affine_rotate(angle), &affine_scale(3.0, 2.0));
        let d = Transform::Basic([m[0], m[1], 12.0, m[3], m[4], -7.0]).decompose();
        approx(d.offset.0, 12.0);
        approx(d.offset.1, -7.0);
        approx(d.rotation, angle);
        approx(d.scale.0, 3.0);
        approx(d.scale.1, 2.0);
    }
}
