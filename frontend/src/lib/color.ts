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

/**
 * Convert a `#rrggbb` hex string to a normalized sRGB `[r, g, b]` triple in
 * `[0, 1]`.
 *
 * NOTE: this deliberately does NOT apply `srgbToLinear`. That conversion is for
 * *paint* colors, which the GPU composites in linear space. `ParamValue::Color`
 * carries the value **as picked** — filter/veil params operate on already-stored
 * texel values (like the Curves LUT), so the sRGB triple is passed through raw.
 * Keep the two conventions distinct: paint → `srgbToLinear`; filter color → this.
 */
export function hexToRgb01(hex: string): [number, number, number] {
    const m = /^#?([0-9a-fA-F]{6})$/.exec(hex.trim());
    if (!m) return [0, 0, 0];
    const n = parseInt(m[1], 16);
    return [((n >> 16) & 0xff) / 255, ((n >> 8) & 0xff) / 255, (n & 0xff) / 255];
}

/** Inverse of {@link hexToRgb01} — a normalized sRGB `[0,1]` triple to `#rrggbb`.
 *  Components are clamped and rounded; see `hexToRgb01` for why no linear step. */
export function rgb01ToHex(rgb: [number, number, number]): string {
    const to255 = (c: number) => Math.max(0, Math.min(255, Math.round(c * 255)));
    const hx = (c: number) => to255(c).toString(16).padStart(2, '0');
    return `#${hx(rgb[0])}${hx(rgb[1])}${hx(rgb[2])}`;
}
