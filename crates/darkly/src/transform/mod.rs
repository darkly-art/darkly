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

pub mod mat3;
pub use mat3::{
    affine_to_mat3, homography_from_corners, mat3_apply, mat3_inverse, mat3_multiply, Mat3,
    MAT3_IDENTITY,
};

use crate::coord::{CanvasPoint, CanvasRect};

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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub enum Transform {
    /// Affine: pan / scale / rotate. Stored as [`Affine2D`].
    Basic(Affine2D),
    /// Projective: true perspective / vanishing-point warp, the user dragging
    /// four corners independently. Stored as a 3×3 homography [`Mat3`].
    Perspective(Mat3),
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

    /// Bake to a single affine matrix. For `Basic` this is the stored matrix;
    /// for `Perspective` it drops the projective bottom row (lossy — used only
    /// by the affine-only void path, which never stores `Perspective`).
    pub fn to_affine(&self) -> Affine2D {
        match self {
            Transform::Basic(m) => *m,
            Transform::Perspective(m) => [m[0], m[1], m[2], m[3], m[4], m[5]],
        }
    }

    /// Widen to a 3×3 projective matrix — what the GPU commit path consumes.
    /// `Basic` widens via [`affine_to_mat3`]; `Perspective` returns its matrix.
    pub fn to_projective(&self) -> Mat3 {
        match self {
            Transform::Basic(m) => affine_to_mat3(m),
            Transform::Perspective(m) => *m,
        }
    }

    /// Whether this is the exact canonical identity for its mode.
    ///
    /// This intentionally performs no epsilon comparison: callers use it to
    /// distinguish a semantic no-op from an operation that must still be
    /// evaluated, however small its effect.
    pub fn is_identity(&self) -> bool {
        match self {
            Transform::Basic(m) => *m == IDENTITY,
            Transform::Perspective(m) => *m == MAT3_IDENTITY,
        }
    }

    /// Stable numeric mode tag for the WASM boundary / gizmo mode registry.
    pub fn mode_tag(&self) -> u32 {
        match self {
            Transform::Basic(_) => 0,
            Transform::Perspective(_) => 1,
        }
    }

    /// The float payload crossing the WASM boundary for this transform: 6
    /// affine components for `Basic`, 9 homography components for
    /// `Perspective`. Paired with [`Self::mode_tag`] it round-trips through
    /// [`Self::from_tag_payload`].
    pub fn wire_payload(&self) -> Vec<f32> {
        match self {
            Transform::Basic(m) => m.to_vec(),
            Transform::Perspective(m) => m.to_vec(),
        }
    }

    /// Decode a `(mode_tag, payload)` pair from the wire into a [`Transform`].
    /// Tag `0` → 6 floats `Basic`; tag `1` → 9 floats `Perspective`. Unknown
    /// tags or short payloads yield `None`. One shared decoder for both the
    /// floating and void update handlers.
    pub fn from_tag_payload(tag: u32, data: &[f32]) -> Option<Transform> {
        match tag {
            0 if data.len() >= 6 => Some(Transform::Basic([
                data[0], data[1], data[2], data[3], data[4], data[5],
            ])),
            1 if data.len() >= 9 => Some(Transform::Perspective([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
            ])),
            _ => None,
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

/// Map a plane-space point through an operation expressed relative to the
/// origin of `operation_frame`.
///
/// Darkly stores matrices row-major and applies them to column vectors, giving
/// `p' = O + M × (p - O)`.
pub fn map_in_operation_frame(
    transform: &Transform,
    operation_frame: CanvasRect,
    point: (f32, f32),
) -> (f32, f32) {
    let origin = operation_frame.origin;
    let local_x = point.0 - origin.x as f32;
    let local_y = point.1 - origin.y as f32;
    let mapped = mat3_apply(&transform.to_projective(), local_x, local_y);
    (mapped.0 + origin.x as f32, mapped.1 + origin.y as f32)
}

/// Derive the evaluator for a target whose local `(0, 0)` is `target_origin`
/// in plane space from one canonical operation frame.
///
/// Under Darkly's row-major/column-vector convention this is
/// `T(O - o) × M × T(o - O)`. Applying the returned matrix to a target-local
/// point is therefore equivalent to lifting it to plane space, applying the
/// canonical operation, and lowering it back into the same target frame.
pub fn evaluator_for_target(
    transform: &Transform,
    operation_frame: CanvasRect,
    target_origin: CanvasPoint,
) -> Mat3 {
    let dx = (operation_frame.origin.x - target_origin.x) as f32;
    let dy = (operation_frame.origin.y - target_origin.y) as f32;
    let to_operation: Mat3 = [1.0, 0.0, -dx, 0.0, 1.0, -dy, 0.0, 0.0, 1.0];
    let from_operation: Mat3 = [1.0, 0.0, dx, 0.0, 1.0, dy, 0.0, 0.0, 1.0];
    mat3_multiply(
        &from_operation,
        &mat3_multiply(&transform.to_projective(), &to_operation),
    )
}

/// Plane-space pixels touched by clearing `extraction_bounds` and writing its
/// transformed footprint. The operation is relative to `operation_frame`;
/// target texture origins do not affect this plane-space result.
pub fn affected_bounds(
    transform: &Transform,
    operation_frame: CanvasRect,
    extraction_bounds: CanvasRect,
) -> CanvasRect {
    if extraction_bounds.is_empty() || transform.is_identity() {
        return extraction_bounds;
    }

    let corners = [
        (extraction_bounds.x0() as f32, extraction_bounds.y0() as f32),
        (extraction_bounds.x1() as f32, extraction_bounds.y0() as f32),
        (extraction_bounds.x1() as f32, extraction_bounds.y1() as f32),
        (extraction_bounds.x0() as f32, extraction_bounds.y1() as f32),
    ];
    let mut min = (f32::INFINITY, f32::INFINITY);
    let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for corner in corners {
        let mapped = map_in_operation_frame(transform, operation_frame, corner);
        let finite_bound = |value: f32| {
            if value.is_nan() {
                0.0
            } else {
                value.clamp(-1.0e9, 1.0e9)
            }
        };
        let x = finite_bound(mapped.0);
        let y = finite_bound(mapped.1);
        min.0 = min.0.min(x);
        min.1 = min.1.min(y);
        max.0 = max.0.max(x);
        max.1 = max.1.max(y);
    }
    let transformed = CanvasRect::from_corners(
        min.0.floor() as i32,
        min.1.floor() as i32,
        max.0.ceil() as i32,
        max.1.ceil() as i32,
    );
    extraction_bounds.union(transformed)
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
    fn identity_is_exact_in_both_modes() {
        assert!(Transform::Basic(IDENTITY).is_identity());
        assert!(Transform::Perspective(MAT3_IDENTITY).is_identity());

        let mut almost_affine = IDENTITY;
        almost_affine[2] = f32::EPSILON;
        assert!(!Transform::Basic(almost_affine).is_identity());

        let mut almost_projective = MAT3_IDENTITY;
        almost_projective[8] += f32::EPSILON;
        assert!(!Transform::Perspective(almost_projective).is_identity());
    }

    #[test]
    fn target_evaluator_matches_canonical_plane_mapping() {
        struct Case {
            name: &'static str,
            transform: Transform,
            operation_frame: CanvasRect,
            target_origin: CanvasPoint,
            local_point: (f32, f32),
        }

        let cases = [
            Case {
                name: "translation differing positive origins",
                transform: Transform::Basic(affine_translate(17.0, -9.0)),
                operation_frame: CanvasRect::from_xywh(40, 30, 80, 60),
                target_origin: CanvasPoint::new(7, 11),
                local_point: (5.0, 13.0),
            },
            Case {
                name: "rotation negative target origin",
                transform: Transform::Basic(affine_rotate(0.63)),
                operation_frame: CanvasRect::from_xywh(23, -14, 80, 60),
                target_origin: CanvasPoint::new(-80, -35),
                local_point: (19.0, 8.0),
            },
            Case {
                name: "non-uniform scale negative operation origin",
                transform: Transform::Basic(affine_scale(2.5, 0.4)),
                operation_frame: CanvasRect::from_xywh(-41, -9, 80, 60),
                target_origin: CanvasPoint::new(12, -70),
                local_point: (31.0, 27.0),
            },
            Case {
                name: "reflection across differing origins",
                transform: Transform::Basic(affine_scale(-1.0, 1.0)),
                operation_frame: CanvasRect::from_xywh(-3, 52, 80, 60),
                target_origin: CanvasPoint::new(-91, 6),
                local_point: (44.0, -2.0),
            },
            Case {
                name: "perspective across negative origins",
                transform: Transform::Perspective([
                    1.0, 0.08, 12.0, -0.03, 0.9, -7.0, 0.001, -0.0007, 1.0,
                ]),
                operation_frame: CanvasRect::from_xywh(-120, 45, 80, 60),
                target_origin: CanvasPoint::new(33, -64),
                local_point: (28.0, 17.0),
            },
        ];

        for case in cases {
            let evaluator =
                evaluator_for_target(&case.transform, case.operation_frame, case.target_origin);
            let direct = mat3_apply(&evaluator, case.local_point.0, case.local_point.1);
            let plane_point = (
                case.local_point.0 + case.target_origin.x as f32,
                case.local_point.1 + case.target_origin.y as f32,
            );
            let mapped_plane =
                map_in_operation_frame(&case.transform, case.operation_frame, plane_point);
            let expected = (
                mapped_plane.0 - case.target_origin.x as f32,
                mapped_plane.1 - case.target_origin.y as f32,
            );
            assert!(
                (direct.0 - expected.0).abs() < 1e-3 && (direct.1 - expected.1).abs() < 1e-3,
                "{}: expected {expected:?}, got {direct:?}",
                case.name
            );
        }
    }

    #[test]
    fn affected_bounds_unites_source_and_canonical_footprint() {
        let extraction = CanvasRect::from_xywh(-30, 10, 20, 10);
        let transform = Transform::Basic(affine_scale(-2.0, 3.0));
        assert_eq!(
            affected_bounds(
                &transform,
                CanvasRect::from_xywh(-20, 15, 20, 10),
                extraction
            ),
            CanvasRect::from_xywh(-40, 0, 40, 30)
        );
        assert_eq!(
            affected_bounds(
                &Transform::identity(),
                CanvasRect::from_xywh(999, -999, 20, 10),
                extraction
            ),
            extraction
        );
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
    fn perspective_serde_and_projective() {
        let m: Mat3 = [1.0, 0.1, 4.0, 0.2, 1.0, -3.0, 0.001, 0.002, 1.0];
        let t = Transform::Perspective(m);
        assert_eq!(t.mode_tag(), 1);
        assert_eq!(t.to_projective(), m);
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("Perspective"), "tagged enum: {json}");
        let back: Transform = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn basic_widens_to_projective() {
        let aff: Affine2D = [2.0, 0.0, 10.0, 0.0, 3.0, 20.0];
        let proj = Transform::from_affine(aff).to_projective();
        assert_eq!(proj, mat3::affine_to_mat3(&aff));
    }

    #[test]
    fn from_tag_payload_round_trips_both_modes() {
        let basic = Transform::from_affine([1.0, 0.0, 5.0, 0.0, 1.0, 6.0]);
        let p = basic.wire_payload();
        assert_eq!(Transform::from_tag_payload(0, &p), Some(basic));

        let persp = Transform::Perspective([1.0, 0.1, 4.0, 0.2, 1.0, -3.0, 0.001, 0.002, 1.0]);
        let p = persp.wire_payload();
        assert_eq!(Transform::from_tag_payload(1, &p), Some(persp));

        // Short payloads / unknown tags decode to None.
        assert_eq!(Transform::from_tag_payload(0, &[1.0, 2.0]), None);
        assert_eq!(Transform::from_tag_payload(1, &[1.0, 2.0, 3.0]), None);
        assert_eq!(Transform::from_tag_payload(7, &[0.0; 9]), None);
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
