/**
 * Convert an sRGB 8-bit component (0-255) to linear 0-1.
 *
 * Darkly's GPU compositor expects linear RGBA throughout; the frontend's
 * picker and `app.foreground` store sRGB 8-bit, so any color crossing into
 * WASM must go through this function.
 */
export function srgbToLinear(c: number): number {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
}

/** Convert an sRGB `{ r, g, b, a }` color (0-255, a 0-255) to a linear RGBA `Float32Array`. */
export function srgbColorToLinearRgbaF32(c: { r: number; g: number; b: number; a: number }): Float32Array {
    return new Float32Array([
        srgbToLinear(c.r),
        srgbToLinear(c.g),
        srgbToLinear(c.b),
        c.a / 255,
    ]);
}
