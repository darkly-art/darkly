/**
 * Hex ↔ color conversions.
 *
 * Darkly is display-referred: every color — the picker, `app.foreground`, paint
 * colors, fill/gradient, filter/veil params, and the stored texels — is the
 * same raw sRGB value, and nothing rescales it. So these are plain byte
 * normalizations; there is deliberately no gamma/linear conversion anywhere in
 * the color path.
 */
import type { Color } from '../state/app.svelte';

const HEX = /^#?([0-9a-fA-F]{6}(?:[0-9a-fA-F]{2})?)$/;

/**
 * Parse `#rrggbb` or `#rrggbbaa` into a byte `Color`. Returns `null` on
 * anything else — callers that want a fallback must say so, because silently
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
 *  form — what recents and pack colors are written as. */
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

/** Inverse of {@link hexToRgb01} — a normalized sRGB `[0,1]` triple to
 *  `#rrggbb`. Components are clamped and rounded. */
export function rgb01ToHex(rgb: [number, number, number]): string {
    const to255 = (c: number) => Math.max(0, Math.min(255, Math.round(c * 255)));
    const hx = (c: number) => to255(c).toString(16).padStart(2, '0');
    return `#${hx(rgb[0])}${hx(rgb[1])}${hx(rgb[2])}`;
}
