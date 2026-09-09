import { describe, it, expect } from 'vitest';
import {
    layoutWheel,
    sectorAt,
    advance,
    hitKey,
    midAngle,
    labelArc,
    HUB_R,
    RING_T,
    CHILD_STEP,
    type SectorGeom,
    type Hit,
} from '../wheel_geometry';
import type { WheelBranch, WheelLeaf, WheelNode, WheelTree } from '../model';
import { NEUTRAL_PALETTE } from '../../../lib/packPalette';

const paint = { visual: { kind: 'icon', icon: '' }, palette: NEUTRAL_PALETTE } as const;
const leaf = (id: string): WheelLeaf =>
    ({ kind: 'leaf', id, label: id, ...paint, select: () => {} });
const branch = (id: string, children: WheelNode[]): WheelBranch =>
    ({ kind: 'branch', id, label: id, ...paint, children });

/** Two half-arc sections: 4 color leaves below; above, a 3-leaf branch and
 *  a branch whose first child is itself a branch (depth 3). Root order:
 *  bottom 0-3, top 4-5. */
const bottomNodes = [leaf('c0'), leaf('c1'), leaf('c2'), leaf('c3')];
const topNodes: WheelNode[] = [
    branch('recent', [leaf('r0'), leaf('r1'), leaf('r2')]),
    branch('dry', [branch('charcoals', [leaf('k0'), leaf('k1')]), leaf('d1')]),
];
const tree: WheelTree = {
    sections: [
        { a0: 0, span: Math.PI, nodes: bottomNodes },
        { a0: -Math.PI, span: Math.PI, nodes: topNodes },
    ],
};

const ring = (layout: SectorGeom[], k: number) => layout.filter(s => s.ring === k);

/** A point inside sector geometry: polar at the sector's angular middle. */
const at = (theta: number, r: number): [number, number] =>
    [r * Math.cos(theta), r * Math.sin(theta)];

describe('midAngle', () => {
    /** Probe along a sector's mid-angle and ask the wheel what is there. A
     *  round trip through `sectorAt` rather than a restatement of
     *  `a0 + span / 2`: it fails on a sign error, a degrees/radians slip, or
     *  an off-by-half-span, which is what would point a rotated chip the
     *  wrong way. */
    const landsOnItself = (layout: SectorGeom[]) => {
        for (const s of layout) {
            const hit = sectorAt(layout, ...at(midAngle(s), (s.r0 + s.r1) / 2));
            expect(hit.kind).toBe('sector');
            expect((hit as { sector: SectorGeom }).sector.path).toEqual(s.path);
        }
    };

    it('points into its own sector, on every ring and in every quadrant', () => {
        landsOnItself(layoutWheel(tree, [5, 0]));
    });

    it('points into its own sector across the ±π seam', () => {
        // The shipped shape: the brushes arc starts past π and wraps.
        const seam: WheelTree = {
            sections: [
                { a0: Math.PI / 6, span: (2 * Math.PI) / 3, nodes: [leaf('c0'), leaf('c1')] },
                {
                    a0: (5 * Math.PI) / 6,
                    span: (4 * Math.PI) / 3,
                    nodes: [branch('recent', [leaf('r0'), leaf('r1')]), branch('lib', [leaf('l')])],
                },
            ],
        };
        landsOnItself(layoutWheel(seam, [2, 1]));
    });

    it('is the quantity the sector paths are built from', () => {
        // Pins the de-duplication: the component's placement math and this
        // function are the same number, so a chip cannot drift off the badge
        // it is drawn in.
        for (const s of layoutWheel(tree, [5])) {
            expect(midAngle(s)).toBeCloseTo(s.a0 + s.span / 2, 12);
        }
    });
});

describe('labelArc', () => {
    /** Where the ink lands, as a screen-space direction: glyphs stand to the
     *  left of a path's direction of travel, so this is the travel direction
     *  at the arc's midpoint turned a quarter that way. */
    const glyphUp = (arc: { a0: number; a1: number; r: number }) => {
        const mid = (arc.a0 + arc.a1) / 2;
        const forward = arc.a1 > arc.a0 ? 1 : -1;
        // d/da of (cos a, sin a), signed by the direction of travel.
        const tx = -Math.sin(mid) * forward;
        const ty = Math.cos(mid) * forward;
        // Left of travel, on a screen whose y grows downward.
        return [ty, -tx];
    };

    it('sets every name right side up, all the way round the wheel', () => {
        // A name is upright when the ink grows toward the top of the screen,
        // which is the whole point of reversing the arc across the horizontal:
        // one direction for the entire circle leaves the bottom half inverted.
        for (const s of layoutWheel(tree, [5, 0])) {
            const [, upY] = glyphUp(labelArc(s));
            expect(upY).toBeLessThan(0);
        }
    });

    it('centres the ink on the band, whichever way it grows', () => {
        // The baseline moves so the ink does not. A baseline is the foot of
        // the ink and not its middle, so a name that grows inward has to be
        // set a cap height further out than one that grows outward for the two
        // to land in the same place.
        for (const s of layoutWheel(tree, [5, 0])) {
            const arc = labelArc(s);
            const grows = arc.a1 > arc.a0 ? 1 : -1;
            const inkCentre = arc.r + (grows * 9) / 2;
            expect(inkCentre).toBeCloseTo((s.r0 + s.r1) / 2, 9);
        }
    });

    it('never asks for an arc between coincident points', () => {
        // A branch spread over the whole circumference: its children each take
        // a slice, but a lone child would take the entire turn, whose start and
        // end are the same point and which draws nothing.
        const full: WheelTree = {
            sections: [{
                a0: 0,
                span: 2 * Math.PI,
                nodes: [branch('lib', [leaf('only')])],
            }],
        };
        const full0 = { ...layoutWheel(full, [])[0] };
        expect(full0.span).toBeCloseTo(2 * Math.PI, 9);
        const arc = labelArc(full0);
        expect(Math.abs(arc.a1 - arc.a0)).toBeLessThan(2 * Math.PI);
    });
});

describe('layoutWheel ring 0', () => {
    const layout = layoutWheel(tree, []);

    it('splits each section arc evenly among its nodes', () => {
        const bottom = ring(layout, 0).filter(s => s.path[0] < 4);
        const top = ring(layout, 0).filter(s => s.path[0] >= 4);
        expect(bottom).toHaveLength(4);
        expect(top).toHaveLength(2);
        for (const s of bottom) expect(s.span).toBeCloseTo(Math.PI / 4, 9);
        for (const s of top) expect(s.span).toBeCloseTo(Math.PI / 2, 9);
        // Bottom tiles (0, π); top tiles (-π, 0).
        expect(bottom[0].a0).toBeCloseTo(0, 9);
        expect(bottom[3].a0 + bottom[3].span).toBeCloseTo(Math.PI, 9);
        expect(top[0].a0).toBeCloseTo(-Math.PI, 9);
        expect(top[1].a0 + top[1].span).toBeCloseTo(0, 9);
    });

    it('lays out thirds whose arcs cross the ±π seam', () => {
        // The shipped shape: colors on the bottom-center third, two brush
        // branches splitting the top two thirds; the brushes arc crosses ±π.
        const thirds: WheelTree = {
            sections: [
                { a0: Math.PI / 6, span: (2 * Math.PI) / 3, nodes: [leaf('c0'), leaf('c1')] },
                {
                    a0: (5 * Math.PI) / 6,
                    span: (4 * Math.PI) / 3,
                    nodes: [branch('recent', [leaf('r')]), branch('lib', [leaf('l')])],
                },
            ],
        };
        const l = layoutWheel(thirds, []);
        const rMid = HUB_R + RING_T / 2;
        // Screen-left lands on Recent (left third), up-right on lib.
        const left = sectorAt(l, ...at(Math.PI, rMid));
        expect(left.kind === 'sector' && left.sector.path).toEqual([2]);
        const right = sectorAt(l, ...at(-Math.PI / 3, rMid));
        expect(right.kind === 'sector' && right.sector.path).toEqual([3]);
        const down = sectorAt(l, ...at(Math.PI / 2, rMid));
        expect(down.kind === 'sector' && down.sector.path).toEqual([1]);
    });

    it('bands ring 0 radially at [HUB_R, HUB_R + RING_T)', () => {
        for (const s of ring(layout, 0)) {
            expect(s.r0).toBe(HUB_R);
            expect(s.r1).toBe(HUB_R + RING_T);
        }
    });

    it('is unbounded only when it is the outermost ring', () => {
        for (const s of ring(layout, 0)) expect(s.unbounded).toBe(true);
        for (const s of ring(layoutWheel(tree, [4]), 0)) expect(s.unbounded).toBe(false);
    });

    it('leaves an empty section sectorless without shifting root indices', () => {
        const empties = layoutWheel({
            sections: [
                { a0: 0, span: Math.PI, nodes: [] },
                { a0: -Math.PI, span: Math.PI, nodes: topNodes },
            ],
        }, []);
        expect(ring(empties, 0).map(s => s.path[0])).toEqual([0, 1]);
    });
});

describe('layoutWheel child fans', () => {
    it('centers the fan on the parent sector mid-angle', () => {
        const layout = layoutWheel(tree, [4]);
        const parent = ring(layout, 0).find(s => s.path[0] === 4)!;
        const fan = ring(layout, 1);
        expect(fan).toHaveLength(3);
        const fanMid = fan[0].a0 + (fan[0].span * fan.length) / 2;
        expect(fanMid).toBeCloseTo(parent.a0 + parent.span / 2, 9);
    });

    it('never fans narrower than the parent span', () => {
        // 2 children at CHILD_STEP would be 45°, narrower than the parent's
        // 90°: the fan widens to the parent span.
        const layout = layoutWheel(tree, [5]);
        const fan = ring(layout, 1);
        expect(fan).toHaveLength(2);
        expect(fan[0].span * fan.length).toBeCloseTo(Math.PI / 2, 9);
    });

    it('grows with the child count and clamps at π', () => {
        const wide: WheelTree = {
            sections: [
                { a0: 0, span: Math.PI, nodes: bottomNodes },
                { a0: -Math.PI, span: Math.PI, nodes: [branch('wide', Array.from({ length: 10 }, (_, i) => leaf(`w${i}`)))] },
            ],
        };
        const fan = ring(layoutWheel(wide, [4]), 1);
        // 10 · 22.5° = 225° clamps to 180°.
        expect(fan[0].span * fan.length).toBeCloseTo(Math.PI, 9);

        const six: WheelTree = {
            sections: [
                { a0: 0, span: Math.PI, nodes: bottomNodes },
                { a0: -Math.PI, span: Math.PI, nodes: [branch('six', Array.from({ length: 8 }, (_, i) => leaf(`s${i}`))), branch('other', [leaf('o')])] },
            ],
        };
        const fan8 = ring(layoutWheel(six, [4]), 1);
        // 8 · 22.5° = 180°: exactly at the clamp, wider than the 90° parent.
        expect(fan8[0].span * fan8.length).toBeCloseTo(8 * CHILD_STEP, 9);
    });

    it("spreads a 'full' branch's children around the entire circumference", () => {
        const packs = Array.from({ length: 5 }, (_, i) => branch(`p${i}`, [leaf(`b${i}`)]));
        const full: WheelTree = {
            sections: [
                { a0: 0, span: Math.PI, nodes: bottomNodes },
                { a0: -Math.PI, span: Math.PI, nodes: [{ ...branch('lib', packs), spread: 'full' }] },
            ],
        };
        const layout = layoutWheel(full, [4]);
        const fan = ring(layout, 1);
        expect(fan).toHaveLength(5);
        expect(fan[0].span * fan.length).toBeCloseTo(2 * Math.PI, 9);
        // Centered on the parent mid-angle (-π/2): the fan starts a half
        // turn before it.
        expect(fan[0].a0).toBeCloseTo(-Math.PI / 2 - Math.PI, 9);
        // No angular gaps anywhere on a full ring.
        const rMid1 = HUB_R + RING_T + RING_T / 2;
        for (const theta of [0, Math.PI / 2, Math.PI, -Math.PI / 2, 2.9]) {
            expect(sectorAt(layout, ...at(theta, rMid1)).kind).toBe('sector');
        }
    });

    it('marks only the outermost ring unbounded and bands radii per ring', () => {
        const layout = layoutWheel(tree, [5, 0]);
        expect(ring(layout, 2)).toHaveLength(2);
        for (const s of ring(layout, 1)) {
            expect(s.unbounded).toBe(false);
            expect(s.r0).toBe(HUB_R + RING_T);
        }
        for (const s of ring(layout, 2)) {
            expect(s.unbounded).toBe(true);
            expect(s.r0).toBe(HUB_R + 2 * RING_T);
        }
    });
});

describe('sectorAt', () => {
    it('resolves the hub inside HUB_R', () => {
        const layout = layoutWheel(tree, []);
        expect(sectorAt(layout, 0, 0)).toEqual({ kind: 'hub' });
        expect(sectorAt(layout, HUB_R - 1, 0).kind).toBe('hub');
    });

    it('bands rings by radius', () => {
        const layout = layoutWheel(tree, [4]);
        const rMid0 = HUB_R + RING_T / 2;
        const rMid1 = HUB_R + RING_T + RING_T / 2;
        const [x0, y0] = at(Math.PI / 8, rMid0); // bottom half, first color
        const hit0 = sectorAt(layout, x0, y0);
        expect(hit0.kind).toBe('sector');
        expect((hit0 as Extract<Hit, { kind: 'sector' }>).sector.path).toEqual([0]);
        const [x1, y1] = at(-3 * Math.PI / 4, rMid1); // recent fan's middle
        const hit1 = sectorAt(layout, x1, y1);
        expect(hit1.kind).toBe('sector');
        expect((hit1 as Extract<Hit, { kind: 'sector' }>).sector.ring).toBe(1);
    });

    it('extends the outermost ring to infinity', () => {
        const layout = layoutWheel(tree, [4]);
        const [x, y] = at(-3 * Math.PI / 4, 5000);
        const hit = sectorAt(layout, x, y);
        expect(hit.kind).toBe('sector');
        expect((hit as Extract<Hit, { kind: 'sector' }>).sector.ring).toBe(1);
    });

    it('resolves angles outside a fan to a gap on that ring', () => {
        const layout = layoutWheel(tree, [4]);
        // Ring 1's fan is centered at -3π/4; theta 0 is far outside it.
        const [x, y] = at(0, HUB_R + RING_T + RING_T / 2);
        expect(sectorAt(layout, x, y)).toEqual({ kind: 'gap', ring: 1 });
    });

    it('resolves an empty section arc to a gap on ring 0', () => {
        const layout = layoutWheel(
            { sections: [{ a0: -Math.PI, span: Math.PI, nodes: topNodes }] }, []);
        const [x, y] = at(Math.PI / 2, HUB_R + 10);
        expect(sectorAt(layout, x, y)).toEqual({ kind: 'gap', ring: 0 });
    });

    it('hit-tests wrap-aware across the ±π seam', () => {
        // A fan of 8 around the top-left parent (mid -3π/4) spans π: its
        // start angle -5π/4 wraps past the seam, so theta just above +3π/4
        // (the wrapped image of the fan's first slice) must hit child 0.
        const wide: WheelTree = {
            sections: [
                { a0: 0, span: Math.PI, nodes: bottomNodes },
                { a0: -Math.PI, span: Math.PI, nodes: [branch('wide', Array.from({ length: 8 }, (_, i) => leaf(`w${i}`))), branch('other', [leaf('o')])] },
            ],
        };
        const layout = layoutWheel(wide, [4]);
        const [x, y] = at(0.8 * Math.PI, HUB_R + RING_T + RING_T / 2);
        const hit = sectorAt(layout, x, y);
        expect(hit.kind).toBe('sector');
        expect((hit as Extract<Hit, { kind: 'sector' }>).sector.path).toEqual([4, 0]);
    });
});

describe('advance (the maze rule)', () => {
    const sectorHit = (layout: SectorGeom[], path: number[]): Hit => {
        const sector = layout.find(s => s.path.join('.') === path.join('.'))!;
        expect(sector).toBeDefined();
        return { kind: 'sector', sector };
    };

    it('hub retracts everything', () => {
        expect(advance([5, 0], { kind: 'hub' })).toEqual([]);
    });

    it('entering a branch expands it', () => {
        const layout = layoutWheel(tree, []);
        expect(advance([], sectorHit(layout, [4]))).toEqual([4]);
    });

    it('moving onto a sibling branch replaces the subtree in one step', () => {
        const layout = layoutWheel(tree, [4]);
        expect(advance([4], sectorHit(layout, [5]))).toEqual([5]);
    });

    it('descends through nested branches', () => {
        const layout = layoutWheel(tree, [5]);
        expect(advance([5], sectorHit(layout, [5, 0]))).toEqual([5, 0]);
    });

    it('a leaf terminates the chain at its ring', () => {
        const deep = layoutWheel(tree, [5, 0]);
        // A ring-1 leaf while ring 2 is expanded: rings beyond retract.
        expect(advance([5, 0], sectorHit(deep, [5, 1]))).toEqual([5]);
        // A ring-0 leaf retracts everything beyond ring 0.
        expect(advance([5, 0], sectorHit(deep, [2]))).toEqual([]);
    });

    it('a gap keeps rings through its own and retracts deeper ones', () => {
        expect(advance([5, 0], { kind: 'gap', ring: 1 })).toEqual([5]);
        expect(advance([5, 0], { kind: 'gap', ring: 0 })).toEqual([]);
        // On the outermost ring this degenerates to "unchanged".
        expect(advance([5, 0], { kind: 'gap', ring: 2 })).toEqual([5, 0]);
    });
});

describe('hitKey', () => {
    it('distinguishes hub, gaps by ring, and sectors by path', () => {
        const layout = layoutWheel(tree, []);
        const keys = new Set([
            hitKey({ kind: 'hub' }),
            hitKey({ kind: 'gap', ring: 0 }),
            hitKey({ kind: 'gap', ring: 1 }),
            hitKey(sectorAt(layout, HUB_R + 10, 10)),
        ]);
        expect(keys.size).toBe(4);
    });
});
