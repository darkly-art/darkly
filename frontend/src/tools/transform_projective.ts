/**
 * Projective (3×3 homography) helpers for the transform gizmo — the
 * perspective generalization of `transform_affine.ts`.
 *
 * CONTRACT: row-major `[m00, m01, m02, m10, m11, m12, m20, m21, m22]`,
 * transforming `(x, y)` → `((m00·x + m01·y + m02)/w, (m10·x + m11·y + m12)/w)`
 * with `w = m20·x + m21·y + m22`. This is a hand-kept mirror of the Rust
 * helpers in `crates/darkly/src/transform/mat3.rs` — the two cannot share one
 * implementation (the gizmo computes interactively in JS and ships the baked
 * homography over the WASM boundary), so the layout is pinned by
 * `transform_projective.test.ts` here and `mat3::tests::homography_contract`
 * on the Rust side. **Do not reorder the components.**
 *
 * Affine is the special case of a [`Mat3`] whose bottom row is `[0, 0, 1]`.
 * `homographyFromCorners` ports GIMP's `gimp_transform_matrix_perspective`
 * (Heckbert unit-square→quad closed form).
 */
import type { Affine2D } from './transform_affine';

export type Mat3 = [number, number, number, number, number, number, number, number, number];

export const MAT3_IDENTITY: Mat3 = [1, 0, 0, 0, 1, 0, 0, 0, 1];

/** Widen a 2D affine `[a, b, tx, c, d, ty]` to a `Mat3` (bottom row [0,0,1]). */
export function affineToMat3(m: Affine2D): Mat3 {
    return [m[0], m[1], m[2], m[3], m[4], m[5], 0, 0, 1];
}

/** Drop a `Mat3`'s projective bottom row to an affine. Lossy unless the matrix
 *  is affine (bottom row [0,0,1]); used only where the wire format is affine. */
export function mat3ToAffine(m: Mat3): Affine2D {
    return [m[0], m[1], m[2], m[3], m[4], m[5]];
}

/** `a · b` — apply `b` first, then `a`. */
export function mat3Multiply(a: Mat3, b: Mat3): Mat3 {
    const out = new Array(9) as Mat3;
    for (let r = 0; r < 3; r++) {
        for (let c = 0; c < 3; c++) {
            out[r * 3 + c] =
                a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    return out;
}

/** Transform a point, applying the perspective divide. */
export function mat3Apply(m: Mat3, x: number, y: number): [number, number] {
    const px = m[0] * x + m[1] * y + m[2];
    const py = m[3] * x + m[4] * y + m[5];
    const w = m[6] * x + m[7] * y + m[8];
    return [px / w, py / w];
}

/** Invert a projective matrix (adjugate / det). `null` if singular. */
export function mat3Inverse(m: Mat3): Mat3 | null {
    const [a, b, c, d, e, f, g, h, i] = m;
    const a00 = e * i - f * h;
    const a01 = f * g - d * i;
    const a02 = d * h - e * g;
    const det = a * a00 + b * a01 + c * a02;
    if (Math.abs(det) < 1e-12) return null;
    const inv = 1 / det;
    return [
        a00 * inv,
        (c * h - b * i) * inv,
        (b * f - c * e) * inv,
        a01 * inv,
        (a * i - c * g) * inv,
        (c * d - a * f) * inv,
        a02 * inv,
        (b * g - a * h) * inv,
        (a * e - b * d) * inv,
    ];
}

/**
 * Homography mapping the source rect `[0,srcW]×[0,srcH]` onto four destination
 * corners `[TL, TR, BR, BL]`. Returns `null` on a degenerate / behind-camera
 * quad (a corner whose homogeneous `w` is non-positive) so a live drag that
 * sweeps a corner toward infinity keeps the last valid matrix instead of
 * emitting NaNs.
 *
 * Mirrors Rust `homography_from_corners`.
 */
export function homographyFromCorners(
    srcW: number,
    srcH: number,
    corners: [[number, number], [number, number], [number, number], [number, number]],
): Mat3 | null {
    if (srcW <= 0 || srcH <= 0) return null;

    // GIMP corner naming: t1 at unit (0,0), t2 at (1,0), t3 at (0,1), t4 at
    // (1,1). Our rect normalized to the unit square: TL→(0,0), TR→(1,0),
    // BR→(1,1), BL→(0,1).
    const [tx1, ty1] = corners[0]; // TL → (0,0)
    const [tx2, ty2] = corners[1]; // TR → (1,0)
    const [tx4, ty4] = corners[2]; // BR → (1,1)
    const [tx3, ty3] = corners[3]; // BL → (0,1)

    const dx1 = tx2 - tx4;
    const dx2 = tx3 - tx4;
    const dx3 = tx1 - tx2 + tx4 - tx3;
    const dy1 = ty2 - ty4;
    const dy2 = ty3 - ty4;
    const dy3 = ty1 - ty2 + ty4 - ty3;

    let trafo: Mat3;
    if (Math.abs(dx3) < 1e-12 && Math.abs(dy3) < 1e-12) {
        // Affine (parallelogram) — no perspective term.
        trafo = [tx2 - tx1, tx3 - tx1, tx1, ty2 - ty1, ty3 - ty1, ty1, 0, 0, 1];
    } else {
        const det2 = dx1 * dy2 - dy1 * dx2;
        if (Math.abs(det2) < 1e-12) return null;
        const g = (dx3 * dy2 - dy3 * dx2) / det2;
        const h = (dx1 * dy3 - dy1 * dx3) / det2;
        trafo = [
            tx2 - tx1 + g * tx2,
            tx3 - tx1 + h * tx3,
            tx1,
            ty2 - ty1 + g * ty2,
            ty3 - ty1 + h * ty3,
            ty1,
            g,
            h,
            1,
        ];
    }

    // Homogeneous w at the unit-square corners: 1, 1+g, 1+h, 1+g+h. All must
    // stay positive or a corner has folded behind the camera.
    const g = trafo[6];
    const h = trafo[7];
    for (const w of [1, 1 + g, 1 + h, 1 + g + h]) {
        if (w <= 1e-6) return null;
    }

    // Compose with the source-rect → unit-square normalization.
    const norm: Mat3 = [1 / srcW, 0, 0, 0, 1 / srcH, 0, 0, 0, 1];
    const m = mat3Multiply(trafo, norm);
    if (!mat3Inverse(m)) return null;
    return m;
}
