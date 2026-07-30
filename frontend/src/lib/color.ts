/**
 * Convert a `#rrggbb` hex string to a normalized sRGB `[r, g, b]` triple in
 * `[0, 1]`.
 *
 * Darkly is display-referred: every color — the picker, `app.foreground`, paint
 * colors, fill/gradient, filter/veil params, and the stored texels — is the
 * same raw sRGB value, and nothing rescales it. So this is a plain byte→[0,1]
 * normalization; there is deliberately no gamma/linear conversion anywhere in
 * the color path.
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
