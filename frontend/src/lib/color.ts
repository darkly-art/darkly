/**
 * Hex ↔ color conversions.
 *
 * Darkly is display-referred: every color (the picker, `app.foreground`, paint
 * colors, fill/gradient, filter/veil params, and the stored texels) is the
 * same raw sRGB value, and nothing rescales it. So these are plain byte
 * normalizations; there is deliberately no gamma/linear conversion anywhere in
 * the color path.
 */
import type { Color } from '../state/app.svelte';

const HEX = /^#?([0-9a-fA-F]{6}(?:[0-9a-fA-F]{2})?)$/;

/**
 * Parse `#rrggbb` or `#rrggbbaa` into a byte `Color`. Returns `null` on
 * anything else; callers that want a fallback must say so, because silently
 * returning black makes a malformed value indistinguishable from a black one.
 * A 6-digit input is opaque.
 */
export function hexToColor(hex: string): Color | null {
    const m = HEX.exec(hex.trim());
    if (!m) return null;
    const d = m[1];
    const n = parseInt(d.slice(0, 6), 16);
    return {
        r: (n >> 16) & 0xff,
        g: (n >> 8) & 0xff,
        b: n & 0xff,
        a: d.length === 8 ? parseInt(d.slice(6, 8), 16) : 255,
    };
}

const hx = (v: number) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, '0');

/** A byte `Color` as `#rrggbbaa`, lowercase. Always 8 digits, so a round trip
 *  through {@link hexToColor} preserves alpha. This is the canonical storage
 *  form: what recents and pack colors are written as. */
export function colorToHex(c: Color): string {
    return `#${hx(c.r)}${hx(c.g)}${hx(c.b)}${hx(c.a)}`;
}

/** A byte `Color` as `#rrggbb`, dropping alpha. The form shown to the painter
 *  in a hex field, where a trailing `ff` on every opaque color is noise. */
export function colorToHexRgb(c: Color): string {
    return `#${hx(c.r)}${hx(c.g)}${hx(c.b)}`;
}

/** A `#rrggbb`/`#rrggbbaa` hex string as a normalized sRGB `[r, g, b]` triple
 *  in `[0, 1]`. Alpha is discarded. Malformed input reads as black, which is
 *  what this helper's callers have always assumed. */
export function hexToRgb01(hex: string): [number, number, number] {
    const c = hexToColor(hex);
    if (!c) return [0, 0, 0];
    return [c.r / 255, c.g / 255, c.b / 255];
}

/** Inverse of {@link hexToRgb01}: a normalized sRGB `[0,1]` triple to
 *  `#rrggbb`. Components are clamped and rounded. */
export function rgb01ToHex(rgb: [number, number, number]): string {
    const to255 = (c: number) => Math.max(0, Math.min(255, Math.round(c * 255)));
    const hx = (c: number) => to255(c).toString(16).padStart(2, '0');
    return `#${hx(rgb[0])}${hx(rgb[1])}${hx(rgb[2])}`;
}

/** Hue in degrees `[0, 360)`, saturation and value in `[0, 1]`. */
export interface Hsv {
    h: number;
    s: number;
    v: number;
}

/** HSV for a byte `Color`. Achromatic input (any gray) has no hue and reads
 *  as `h = 0`; a caller that needs to keep a previous hue across grays does
 *  that itself (see `ui/color/wheel_model.ts`). Alpha is ignored. */
export function rgbToHsv(c: Color): Hsv {
    const r = c.r / 255, g = c.g / 255, b = c.b / 255;
    const max = Math.max(r, g, b), min = Math.min(r, g, b);
    const d = max - min;
    let h = 0;
    if (d !== 0) {
        if (max === r) h = ((g - b) / d + (g < b ? 6 : 0)) * 60;
        else if (max === g) h = ((b - r) / d + 2) * 60;
        else h = ((r - g) / d + 4) * 60;
    }
    return { h, s: max === 0 ? 0 : d / max, v: max };
}

/** A byte `Color` for HSV plus the given alpha. Components are rounded to
 *  bytes, so the round trip through {@link rgbToHsv} is lossy. */
export function hsvToRgb(hsv: Hsv, a: number): Color {
    const h = (((hsv.h % 360) + 360) % 360) / 60;
    const c = hsv.v * hsv.s;
    const x = c * (1 - Math.abs((h % 2) - 1));
    const m = hsv.v - c;
    let r = 0, g = 0, b = 0;
    if (h < 1) { r = c; g = x; }
    else if (h < 2) { r = x; g = c; }
    else if (h < 3) { g = c; b = x; }
    else if (h < 4) { g = x; b = c; }
    else if (h < 5) { r = x; b = c; }
    else { r = c; b = x; }
    return {
        r: Math.round((r + m) * 255),
        g: Math.round((g + m) * 255),
        b: Math.round((b + m) * 255),
        a,
    };
}
