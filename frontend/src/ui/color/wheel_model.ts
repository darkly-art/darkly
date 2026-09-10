/**
 * Geometry and state rules for the hue ring + saturation/value triangle color
 * wheel. Pure (no DOM, no runes): the component paints from `pointForHue` /
 * `pointForSv` and hit-tests exclusively through `regionAt` / `hueAt` / `svAt`,
 * so paint and hit can never disagree.
 *
 * The ring, triangle, and clamped edge projection are a port of GIMP's
 * `gimpcolorwheel.c` (`compute_triangle`, `is_in_ring`, `compute_sv`,
 * `is_in_triangle`, `compute_v`) by Simon Budig, Federico Mena-Quintero,
 * Jonathan Blandford and Michael Natterer, GPL-3.0-or-later:
 * https://gitlab.gnome.org/GNOME/gimp/-/blob/master/modules/gimpcolorwheel.c
 *
 * The keep-last-hue rule in `hsvFromColor` is Krita's
 * `KisVisualColorModel::convertKoColorToChannelValues`
 * (libs/widgets/KisVisualColorModel.cpp): when a color has no hue, the
 * previous one stands.
 *
 * Coordinates are screen space relative to the wheel's top-left corner, +y
 * down. Hue 0 sits at +x and increases clockwise on screen, which is what a
 * CSS `conic-gradient(from 90deg, ...)` draws, so the ring needs no per-pixel
 * paint. `ui/palette_popup/wheel_geometry.ts` uses the same polar convention.
 */
import { angularOffset } from '../../lib/angle';
import { hsvToRgb, rgbToHsv, type Hsv } from '../../lib/color';
import type { Color } from '../../state/app.svelte';

export interface Pt {
    x: number;
    y: number;
}

/** A square wheel of `size` px: a hue ring `ringWidth` px thick, then `gap`
 *  px of clear space, then the triangle inscribed in what is left. The gap is
 *  what keeps the triangle's corners off the ring, so the two never touch. */
export interface WheelGeometry {
    size: number;
    ringWidth: number;
    gap: number;
}

/** Ring thickness as a fraction of the diameter: thin, so the wheel reads as
 *  a circle drawn around the triangle rather than a band containing it. */
const RING_FRACTION = 0.06;
/** Clear space between ring and triangle, as a fraction of the diameter. */
const GAP_FRACTION = 0.05;

/** The wheel's proportions at a given diameter. One home for them, so a
 *  consumer never restates the fractions. */
export function wheelGeometry(size: number): WheelGeometry {
    return {
        size,
        ringWidth: Math.round(size * RING_FRACTION),
        gap: Math.round(size * GAP_FRACTION),
    };
}

export type WheelRegion = 'ring' | 'triangle' | null;

const DEG = 180 / Math.PI;

/**
 * The wheel's own HSV for an incoming color. Returns `prev` itself when it
 * already describes the same bytes (the wheel's own writes round-trip through
 * the host and must not quantize its state), and keeps `prev.h` when the color
 * is achromatic (a gray has no hue; snapping to 0 would undo the ring pick
 * that produced the gray).
 */
export function hsvFromColor(c: Color, prev: Hsv): Hsv {
    const back = hsvToRgb(prev, c.a);
    if (back.r === c.r && back.g === c.g && back.b === c.b) return prev;
    const next = rgbToHsv(c);
    return next.s === 0 ? { ...next, h: prev.h } : next;
}

function center(g: WheelGeometry): number {
    return g.size / 2;
}

/** Inside edge of the hue ring. */
function innerRadius(g: WheelGeometry): number {
    return g.size / 2 - g.ringWidth;
}

/** Circumcircle of the saturation/value triangle: the ring's inner edge
 *  pulled in by the gap. */
export function triangleRadius(g: WheelGeometry): number {
    return innerRadius(g) - g.gap;
}

/** Which part of the wheel a point lands on given the current hue `h` (the
 *  triangle turns with it), or `null` for the corners and the gaps between the
 *  triangle and the ring. */
export function regionAt(g: WheelGeometry, h: number, x: number, y: number): WheelRegion {
    // The gap keeps the two apart, so nothing is ever both.
    if (isInTriangle(g, h, x, y)) return 'triangle';
    const c = center(g);
    const dx = x - c, dy = y - c;
    const dist = dx * dx + dy * dy;
    const outer = g.size / 2, inner = innerRadius(g);
    return dist >= inner * inner && dist <= outer * outer ? 'ring' : null;
}

/** Hue in degrees `[0, 360)` for the direction from the center to a point. */
export function hueAt(g: WheelGeometry, x: number, y: number): number {
    const c = center(g);
    return angularOffset(Math.atan2(y - c, x - c), 0) * DEG;
}

/** The point on the ring's midline at hue `h`. */
export function pointForHue(g: WheelGeometry, h: number): Pt {
    const c = center(g);
    const r = g.size / 2 - g.ringWidth / 2;
    const a = h / DEG;
    return { x: c + Math.cos(a) * r, y: c + Math.sin(a) * r };
}

/** The triangle's corners for hue `h`: the pure hue (s = 1, v = 1), black
 *  (v = 0), and white (s = 0, v = 1), 120 degrees apart on the inner circle. */
export function triangleVertices(g: WheelGeometry, h: number): { hue: Pt; black: Pt; white: Pt } {
    const c = center(g);
    const r = triangleRadius(g);
    const at = (a: number): Pt => ({ x: c + Math.cos(a) * r, y: c + Math.sin(a) * r });
    const a = h / DEG;
    return { hue: at(a), black: at(a + (2 * Math.PI) / 3), white: at(a + (4 * Math.PI) / 3) };
}

/** The triangle's barycentric map at one hue. The vertices and determinant
 *  are constant across the triangle, so a per-pixel loop prepares this once
 *  instead of rebuilding the geometry (six trig calls) at every sample. */
export interface Barycentric {
    /** The share of the hue corner and of the white corner at a point; black
     *  is the remainder. Inside iff both are non-negative and sum to at most
     *  1. Linear RGB interpolation over these weights is exactly HSV → RGB at
     *  that hue: `rgb = v·s·hue + v·(1-s)·white`. */
    weights(x: number, y: number): { hue: number; white: number };
    /** Multiply a weight by this for the perpendicular distance in pixels
     *  from the edge that weight vanishes on, which is what turns the inside
     *  test into antialiased coverage. One number for all three edges: the
     *  triangle is equilateral, so their scales coincide. */
    edgeScale: number;
}

/** Prepare {@link Barycentric} for hue `h`. */
export function barycentricFor(g: WheelGeometry, h: number): Barycentric {
    const { hue: H, black: S, white: V } = triangleVertices(g, h);
    const hx = H.x - S.x, hy = H.y - S.y;
    const vx = V.x - S.x, vy = V.y - S.y;
    const det = vx * hy - vy * hx;
    return {
        edgeScale: Math.abs(det) / Math.hypot(hx, hy),
        weights(x: number, y: number) {
            const dx = x - S.x, dy = y - S.y;
            return { white: (dx * hy - dy * hx) / det, hue: (vx * dy - vy * dx) / det };
        },
    };
}

/** One point's barycentric weights. Hit-testing path; a paint loop prepares
 *  {@link barycentricFor} once instead. */
export function triangleWeights(g: WheelGeometry, h: number, x: number, y: number): { hue: number; white: number } {
    return barycentricFor(g, h).weights(x, y);
}

const EDGE_EPS = 1e-9;

function isInTriangle(g: WheelGeometry, h: number, x: number, y: number): boolean {
    const w = triangleWeights(g, h, x, y);
    return w.hue >= -EDGE_EPS && w.white >= -EDGE_EPS && w.hue + w.white <= 1 + EDGE_EPS;
}

const clamp01 = (t: number) => Math.max(0, Math.min(1, t));

/** Parameter of the projection of a point onto segment `a`→`b` (0 at `a`, 1 at `b`). */
function project(a: Pt, b: Pt, px: number, py: number): number {
    const dx = b.x - a.x, dy = b.y - a.y;
    return ((px - a.x) * dx + (py - a.y) * dy) / (dx * dx + dy * dy);
}

/**
 * Saturation and value for a point, given the current hue. Points outside
 * the triangle project onto the nearest edge, so a drag past the boundary
 * stays on it. The triangle is equilateral about the wheel's center, so the
 * vector from the center to a corner is the outward normal of the opposite
 * edge: a negative dot product with it puts the point beyond that edge.
 */
export function svAt(g: WheelGeometry, h: number, x: number, y: number): { s: number; v: number } {
    const { hue: H, black: S, white: V } = triangleVertices(g, h);
    const c = center(g);
    const beyond = (corner: Pt, edgeStart: Pt) =>
        (corner.x - c) * (x - edgeStart.x) + (corner.y - c) * (y - edgeStart.y) < 0;

    // Beyond the black-hue edge: s = 1, v runs from black to the hue corner.
    if (beyond(V, S)) return { s: 1, v: clamp01(project(S, H, x, y)) };
    // Beyond the black-white edge: s = 0, v runs from black to white.
    if (beyond(H, S)) return { s: 0, v: clamp01(project(S, V, x, y)) };
    // Beyond the white-hue edge: v = 1, s runs from white to the hue corner.
    if (beyond(S, H)) return { s: clamp01(project(V, H, x, y)), v: 1 };

    const w = triangleWeights(g, h, x, y);
    const v = w.hue + w.white;
    if (v <= 0) return { s: 0, v: 0 };
    return { s: clamp01(w.hue / v), v: clamp01(v) };
}

/** The point in the triangle for hue `h` at saturation `s` and value `v`. */
export function pointForSv(g: WheelGeometry, h: number, s: number, v: number): Pt {
    const { hue: H, black: S, white: V } = triangleVertices(g, h);
    return {
        x: S.x + v * (V.x - S.x) + v * s * (H.x - V.x),
        y: S.y + v * (V.y - S.y) + v * s * (H.y - V.y),
    };
}
