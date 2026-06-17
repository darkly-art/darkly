/**
 * Shared 2D affine helpers for the transform gizmo.
 *
 * CONTRACT: row-major `[a, b, tx, c, d, ty]`, transforming `(x, y)` →
 * `(a*x + b*y + tx, c*x + d*y + ty)`. This is a hand-kept mirror of the Rust
 * helpers in `crates/darkly/src/transform/mod.rs` — the two cannot share one
 * implementation (the gizmo computes interactively in JS and ships the baked
 * affine over the WASM boundary), so the layout is pinned by
 * `transform_affine.test.ts` here and `transform::tests::affine_contract`
 * on the Rust side. **Do not reorder the components.**
 */

export type Affine2D = [number, number, number, number, number, number];

export const IDENTITY: Affine2D = [1, 0, 0, 0, 1, 0];

export function affineTransform(m: Affine2D, x: number, y: number): [number, number] {
    return [m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]];
}

/** `a ∘ b` — apply `b` first, then `a`. */
export function affineMultiply(a: Affine2D, b: Affine2D): Affine2D {
    return [
        a[0] * b[0] + a[1] * b[3],
        a[0] * b[1] + a[1] * b[4],
        a[0] * b[2] + a[1] * b[5] + a[2],
        a[3] * b[0] + a[4] * b[3],
        a[3] * b[1] + a[4] * b[4],
        a[3] * b[2] + a[4] * b[5] + a[5],
    ];
}

export function affineTranslate(tx: number, ty: number): Affine2D {
    return [1, 0, tx, 0, 1, ty];
}

export function affineScale(sx: number, sy: number): Affine2D {
    return [sx, 0, 0, 0, sy, 0];
}

export function affineRotate(angle: number): Affine2D {
    const c = Math.cos(angle);
    const s = Math.sin(angle);
    return [c, -s, 0, s, c, 0];
}

export function affineInverse(m: Affine2D): Affine2D | null {
    const [a, b, tx, c, d, ty] = m;
    const det = a * d - b * c;
    if (Math.abs(det) < 1e-12) return null;
    const inv = 1 / det;
    return [
        d * inv,
        -b * inv,
        (b * ty - d * tx) * inv,
        -c * inv,
        a * inv,
        (c * tx - a * ty) * inv,
    ];
}
