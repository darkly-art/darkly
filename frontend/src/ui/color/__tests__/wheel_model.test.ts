import { describe, expect, it } from 'vitest';
import { hsvToRgb } from '../../../lib/color';
import {
    barycentricFor,
    hsvFromColor,
    hueAt,
    pointForHue,
    pointForSv,
    regionAt,
    svAt,
    triangleRadius,
    triangleVertices,
    triangleWeights,
    wheelGeometry,
} from '../wheel_model';

const g = wheelGeometry(200);
const C = 100;

describe('hsvFromColor', () => {
    it('keeps_the_previous_hue_for_black_white_and_gray', () => {
        const prev = { h: 120, s: 0.5, v: 0.5 };
        for (const gray of [0, 128, 255]) {
            const got = hsvFromColor({ r: gray, g: gray, b: gray, a: 255 }, prev);
            expect(got.h, `gray ${gray}`).toBe(120);
            expect(got.s).toBe(0);
        }
    });

    it('returns_the_previous_state_itself_when_it_already_describes_the_bytes', () => {
        const prev = { h: 200.37, s: 0.731, v: 0.42 };
        const c = hsvToRgb(prev, 255);
        expect(hsvFromColor(c, prev)).toBe(prev);
    });

    it('re_derives_for_a_chromatic_color', () => {
        const got = hsvFromColor({ r: 0, g: 0, b: 255, a: 255 }, { h: 10, s: 0.2, v: 0.3 });
        expect(got).toEqual({ h: 240, s: 1, v: 1 });
    });
});

describe('ring', () => {
    it('hueAt_reads_clockwise_from_plus_x', () => {
        expect(hueAt(g, C + 50, C)).toBeCloseTo(0);
        expect(hueAt(g, C, C + 50)).toBeCloseTo(90);
        expect(hueAt(g, C - 50, C)).toBeCloseTo(180);
        expect(hueAt(g, C, C - 50)).toBeCloseTo(270);
    });

    it('pointForHue_lands_on_the_ring_midline_and_reads_back', () => {
        for (const h of [0, 33, 120, 200, 359]) {
            const p = pointForHue(g, h);
            expect(regionAt(g, h, p.x, p.y)).toBe('ring');
            expect(hueAt(g, p.x, p.y)).toBeCloseTo(h);
        }
    });

    it('regionAt_classifies_ring_triangle_gap_and_outside', () => {
        expect(regionAt(g, 0, C + (g.size / 2 - g.ringWidth / 2), C)).toBe('ring');
        expect(regionAt(g, 0, C, C)).toBe('triangle');
        // Between the triangle's flat side and the ring, opposite the hue corner.
        expect(regionAt(g, 0, C - 60, C)).toBeNull();
        expect(regionAt(g, 0, 1, 1)).toBeNull();
    });

    it('the_gap_keeps_the_triangle_clear_of_the_ring', () => {
        // Every corner sits inside the ring's hole by the full gap, so a press
        // on one is unambiguously the triangle.
        const inner = g.size / 2 - g.ringWidth;
        for (const h of [0, 90, 210]) {
            const vs = triangleVertices(g, h);
            for (const p of [vs.hue, vs.black, vs.white]) {
                expect(Math.hypot(p.x - C, p.y - C)).toBeCloseTo(inner - g.gap, 6);
                expect(regionAt(g, h, p.x, p.y)).toBe('triangle');
            }
        }
    });
});

describe('triangle', () => {
    it('vertices_map_to_hue_white_and_black', () => {
        const h = 45;
        const { hue, black, white } = triangleVertices(g, h);
        const w = (p: { x: number; y: number }) => triangleWeights(g, h, p.x, p.y);
        expect(w(hue).hue).toBeCloseTo(1);
        expect(w(hue).white).toBeCloseTo(0);
        expect(w(white).white).toBeCloseTo(1);
        expect(w(white).hue).toBeCloseTo(0);
        expect(w(black).hue).toBeCloseTo(0);
        expect(w(black).white).toBeCloseTo(0);
        expect(svAt(g, h, hue.x, hue.y)).toEqual({ s: 1, v: 1 });
        expect(svAt(g, h, white.x, white.y).s).toBeCloseTo(0);
        expect(svAt(g, h, white.x, white.y).v).toBeCloseTo(1);
        expect(svAt(g, h, black.x, black.y).v).toBeCloseTo(0);
    });

    it('svAt_inverts_pointForSv_across_the_interior', () => {
        for (const h of [0, 77, 180, 300]) {
            for (let s = 0.1; s <= 1; s += 0.3) {
                for (let v = 0.1; v <= 1; v += 0.3) {
                    const p = pointForSv(g, h, s, v);
                    const got = svAt(g, h, p.x, p.y);
                    expect(got.s, `h=${h} s=${s} v=${v}`).toBeCloseTo(s, 6);
                    expect(got.v, `h=${h} s=${s} v=${v}`).toBeCloseTo(v, 6);
                }
            }
        }
    });

    it('points_beyond_each_edge_clamp_onto_it_without_nan', () => {
        const h = 0;
        const { hue, black, white } = triangleVertices(g, h);
        const mid = (a: typeof hue, b: typeof hue) => ({ x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 });
        const push = (p: { x: number; y: number }, away: { x: number; y: number }) => ({
            x: p.x + (p.x - away.x),
            y: p.y + (p.y - away.y),
        });
        // Beyond the black-hue edge: s stays 1.
        const a = svAt(g, h, push(mid(black, hue), white).x, push(mid(black, hue), white).y);
        expect(a.s).toBe(1);
        expect(a.v).toBeCloseTo(0.5);
        // Beyond the black-white edge: s stays 0.
        const b = svAt(g, h, push(mid(black, white), hue).x, push(mid(black, white), hue).y);
        expect(b.s).toBe(0);
        expect(b.v).toBeCloseTo(0.5);
        // Beyond the white-hue edge: v stays 1.
        const c = svAt(g, h, push(mid(white, hue), black).x, push(mid(white, hue), black).y);
        expect(c.v).toBe(1);
        expect(c.s).toBeCloseTo(0.5);
        for (const r of [a, b, c]) {
            expect(Number.isNaN(r.s)).toBe(false);
            expect(Number.isNaN(r.v)).toBe(false);
        }
        // Far outside, past a corner.
        const far = svAt(g, h, 10_000, -10_000);
        expect(far.s).toBeGreaterThanOrEqual(0);
        expect(far.s).toBeLessThanOrEqual(1);
        expect(far.v).toBeGreaterThanOrEqual(0);
        expect(far.v).toBeLessThanOrEqual(1);
    });
});

describe('edge coverage', () => {
    // Painting antialiases by turning each weight into a distance in pixels,
    // so the scale has to be a true perpendicular distance.
    it('a_weight_times_the_edge_scale_is_the_distance_in_pixels', () => {
        const b = barycentricFor(g, 0);
        const { hue, black, white } = triangleVertices(g, 0);
        const inradius = triangleRadius(g) / 2;

        // The centroid sits one inradius from every edge, and its three
        // weights are all 1/3.
        const c = { x: (hue.x + black.x + white.x) / 3, y: (hue.y + black.y + white.y) / 3 };
        const w = b.weights(c.x, c.y);
        expect(w.hue).toBeCloseTo(1 / 3);
        expect(w.white).toBeCloseTo(1 / 3);
        expect(w.hue * b.edgeScale).toBeCloseTo(inradius, 6);
    });

    it('the_scale_is_the_same_at_every_hue', () => {
        const at0 = barycentricFor(g, 0).edgeScale;
        for (const h of [37, 120, 250, 359]) {
            expect(barycentricFor(g, h).edgeScale).toBeCloseTo(at0, 6);
        }
    });

    it('a_corner_weight_is_zero_on_its_opposite_edge_and_negative_outside', () => {
        const b = barycentricFor(g, 0);
        const { hue, black, white } = triangleVertices(g, 0);
        const mid = { x: (black.x + white.x) / 2, y: (black.y + white.y) / 2 };
        expect(b.weights(mid.x, mid.y).hue * b.edgeScale).toBeCloseTo(0, 6);
        // One pixel beyond that edge, away from the hue corner.
        const away = Math.hypot(mid.x - hue.x, mid.y - hue.y);
        const out = {
            x: mid.x + (mid.x - hue.x) / away,
            y: mid.y + (mid.y - hue.y) / away,
        };
        expect(b.weights(out.x, out.y).hue * b.edgeScale).toBeCloseTo(-1, 6);
    });
});
