/**
 * Range clamping.
 *
 * Five modules each declared their own `clamp` or `clamp01`. The function is
 * one line, so the duplication cost nothing in bytes; what it cost was five
 * places to disagree about argument order, which two of them already did
 * (`clamp(v, lo, hi)` against `clamp(v, min, max)`).
 */

/** `v` constrained to `[lo, hi]`. */
export function clamp(v: number, lo: number, hi: number): number {
    return Math.min(hi, Math.max(lo, v));
}

/** `v` constrained to `[0, 1]`, the common case for normalized parameters. */
export function clamp01(v: number): number {
    return clamp(v, 0, 1);
}
