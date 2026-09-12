import { describe, it, expect } from 'vitest';
import {
    layoutWheel,
    sectorAt,
    advance,
    hitKey,
    midAngle,
    labelArc,
    labelArcLen,
    labelPlacement,
    labelRadius,
    labelDemand,
    markWidth,
    HUB_R,
    RING_T,
    MARK,
    CHIP_ARC,
    CHILD_STEP,
    type SectorGeom,
    type Hit,
} from '../wheel_geometry';
import type { WheelBranch, WheelLeaf, WheelNode, WheelTree } from '../model';
import { brushLeaf, recentTree } from './trees';
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

/** Probe along a sector's mid-angle and ask the wheel what is there. A round
 *  trip through `sectorAt` rather than a restatement of `a0 + span / 2`: it
 *  fails on a sign error, a degrees/radians slip, or an off-by-half-span,
 *  which is what would point a rotated chip the wrong way. */
const landsOnItself = (layout: SectorGeom[]) => {
    for (const s of layout) {
        const hit = sectorAt(layout, ...at(midAngle(s), (s.r0 + s.r1) / 2));
        expect(hit.kind).toBe('sector');
        expect((hit as { sector: SectorGeom }).sector.path).toEqual(s.path);
    }
};

describe('midAngle', () => {
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

// ---------------------------------------------------------------------------
// Label fitting and the widening it arms.
// ---------------------------------------------------------------------------

/** A pack fan of `n` members under one full-circumference branch, which is the
 *  Library's shape and the only fan on the wheel whose size is unbounded. */
const packTree = (names: string[]): WheelTree => ({
    sections: [{
        a0: 0,
        span: 2 * Math.PI,
        nodes: [{
            ...branch('library', names.map(name => branch(name, [leaf(`${name}-b`)]))),
            spread: 'full' as const,
        }],
    }],
});

/** `n` packs named `p0 … p(n-1)`. */
const packs = (n: number) => packTree(Array.from({ length: n }, (_, i) => `p${i}`));


/** Every label measuring `px`, which is what makes a fan crowded or not. */
const widthsOf = (labels: string[], px: number) =>
    new Map(labels.map(l => [l, px]));

const ringOf = (layout: SectorGeom[], k: number) => layout.filter(s => s.ring === k);
/** Just the geometry, for asserting that a layout is placed identically to
 *  another. `showsName` is not geometry: measuring the names is exactly what
 *  changes it, so a layout given widths differs there and nowhere else when
 *  every name fits. */
const placement = (layout: SectorGeom[]) =>
    layout.map(({ showsName, node, ...geom }) => geom);
const total = (fan: SectorGeom[]) => fan.reduce((a, s) => a + s.span, 0);

describe('showsName', () => {
    /** The width at which every member of an `n`-pack fan is crowded. */
    const crowding = (n: number) => 2 * Math.PI * 95.5 / n;
    const namesOf = (n: number) => Array.from({ length: n }, (_, i) => `p${i}`);

    it('names every pack when the whole fan fits', () => {
        const n = 5;
        const fan = ringOf(layoutWheel(packs(n), [0, 2], widthsOf(namesOf(n), 20)), 1);
        expect(fan.map(s => s.showsName)).toEqual(fan.map(() => true));
    });

    it('names only the widened pack once any one name does not fit', () => {
        // The `Ink` case: one short name among long ones must not be the lone
        // label in a ring of marks. A fan decides together.
        const n = 8;
        const names = namesOf(n);
        const widths = new Map(names.map(l => [l, 20]));
        widths.set('p3', crowding(n) * 3);
        const fan = ringOf(layoutWheel(packs(n), [0, 6], widths), 1);
        expect(fan.map(s => s.showsName)).toEqual(fan.map((_, i) => i === 6));
    });

    it('names nothing in a crowded fan with no selection', () => {
        const n = 8;
        const names = namesOf(n);
        const widths = new Map(names.map(l => [l, 20]));
        widths.set('p3', crowding(n) * 3);
        // Path stops at the Library: the pack ring is drawn, none selected.
        const fan = ringOf(layoutWheel(packs(n), [0], widths), 1);
        expect(fan.some(s => s.showsName)).toBe(false);
    });

    it('names nothing that has not been measured', () => {
        const fan = ringOf(layoutWheel(packs(6), [0, 1], new Map()), 1);
        expect(fan.some(s => s.showsName)).toBe(false);
    });

    it('names the widened pack, at every index and on both halves of the wheel', () => {
        // The widening solves for the span at which a name exactly fits, so the
        // selected sector arrives at the fit check on a constructed tie. Landing
        // on the wrong side of it means widening a sector to fit a name and then
        // declining to draw it, which is the one outcome both halves exist to
        // prevent.
        const n = 24;
        const names = namesOf(n);
        const px = crowding(n);
        for (let i = 0; i < n; i++) {
            const fan = ringOf(layoutWheel(packs(n), [0, i], widthsOf(names, px)), 1);
            expect(fan[i].showsName).toBe(true);
            expect(labelDemand(markWidth(fan[i].node), px))
                .toBeLessThanOrEqual(labelArcLen(fan[i]) + 1e-6);
        }
    });

    it('declines to name a pack the fan could not buy enough room for', () => {
        // A fan too small to pay for its longest name must not draw it anyway:
        // an overflowing `<textPath>` sheds glyphs from both ends silently.
        const n = 6;
        const names = namesOf(n);
        const fan = ringOf(layoutWheel(packs(n), [0, 2], widthsOf(names, 5000)), 1);
        expect(fan.some(s => s.showsName)).toBe(false);
    });
});

describe('brush names', () => {
    /** One pack of `n` brushes, reached at ring 2. Brush leaves carry a chip
     *  rather than a glyph, so they exercise the run with the other mark. */
    const brushTree = (names: string[]): WheelTree => ({
        sections: [{
            a0: 0,
            span: 2 * Math.PI,
            nodes: [{
                ...branch('library', [branch('pack', names.map(brushLeaf))]),
                spread: 'full' as const,
            }],
        }],
    });

    it('measures a brush against its chip, not against a glyph', () => {
        expect(markWidth({
            kind: 'leaf', id: 'b', label: 'b', palette: NEUTRAL_PALETTE, select: () => {},
            visual: { kind: 'brush', name: 'b', icon: null },
        })).toBe(CHIP_ARC);
        expect(markWidth(branch('p', []))).toBe(MARK);
    });

    it('names the brush under the pen and no other', () => {
        const names = ['b0', 'b1', 'b2', 'b3', 'b4', 'b5'];
        const fan = ringOf(layoutWheel(brushTree(names), [0, 0, 3],
            widthsOf(names, 140)), 2);
        expect(fan.map(s => s.showsName)).toEqual(fan.map((_, i) => i === 3));
    });

    it('widens the brush under the pen enough to hold its name', () => {
        const names = ['b0', 'b1', 'b2', 'b3', 'b4', 'b5'];
        const px = 140;
        for (const i of [0, 2, 5]) {
            const fan = ringOf(layoutWheel(brushTree(names), [0, 0, i],
                widthsOf(names, px)), 2);
            expect(fan[i].showsName).toBe(true);
            expect(labelDemand(CHIP_ARC, px))
                .toBeLessThanOrEqual(labelArcLen(fan[i]) + 1e-6);
        }
    });

    it('leaves a brush fan alone when no name is measured', () => {
        const names = ['b0', 'b1', 'b2'];
        const withNames = layoutWheel(brushTree(names), [0, 0, 1], new Map());
        expect(ringOf(withNames, 2).some(s => s.showsName)).toBe(false);
    });

    it('centres an unnamed chip on its sector, and slides a named one aside', () => {
        const names = ['b0', 'b1', 'b2', 'b3'];
        const bare = ringOf(layoutWheel(brushTree(names), [0, 0, 1], new Map()), 2)[1];
        expect(labelPlacement(bare, 0).markA).toBeCloseTo(midAngle(bare), 12);

        // Named, the chip gives up half of the gap and the name to sit beside
        // them, both centred on the arc together. Which *way* it slides is not
        // asserted: `labelArc` reverses its direction of travel across the
        // horizontal so names stay readable, and the chip travels with it.
        const nameLen = 140;
        const named = ringOf(layoutWheel(brushTree(names), [0, 0, 1],
            widthsOf(names, nameLen)), 2)[1];
        const offset = Math.abs(labelPlacement(named, nameLen).markA - midAngle(named))
            * labelArc(named).r;
        // `labelDemand(0, n)` is the gap plus the name, the run without a mark.
        expect(offset).toBeCloseTo(labelDemand(0, nameLen) / 2, 9);
    });
});

describe('layoutWheel widening', () => {
    /** The width at which every member of an `n`-pack fan is crowded. */
    const crowding = (n: number) => 2 * Math.PI * 95.5 / n;

    it('is exactly the even layout when no name is measured', () => {
        const tree10 = packs(10);
        expect(layoutWheel(tree10, [0, 3], new Map())).toEqual(layoutWheel(tree10, [0, 3]));
    });

    it('places sectors exactly as the even layout does when every name fits', () => {
        const tree10 = packs(10);
        const names = Array.from({ length: 10 }, (_, i) => `p${i}`);
        const fits = widthsOf(names, 10);
        expect(placement(layoutWheel(tree10, [0, 3], fits)))
            .toEqual(placement(layoutWheel(tree10, [0, 3])));
    });

    it('contains the selected sector’s own base interval, at every index', () => {
        const n = 50;
        const tree50 = packs(n);
        const names = Array.from({ length: n }, (_, i) => `p${i}`);
        const w = widthsOf(names, crowding(n));
        const base = ringOf(layoutWheel(tree50, [0]), 1);
        for (let i = 0; i < n; i++) {
            const s = ringOf(layoutWheel(tree50, [0, i], w), 1)[i];
            expect(s.a0).toBeLessThanOrEqual(base[i].a0 + 1e-12);
            expect(s.a0 + s.span).toBeGreaterThanOrEqual(base[i].a0 + base[i].span - 1e-12);
        }
    });

    it('conserves the fan’s total span and pins its endpoints', () => {
        const n = 20;
        const tree20 = packs(n);
        const names = Array.from({ length: n }, (_, i) => `p${i}`);
        const w = widthsOf(names, crowding(n));
        const base = ringOf(layoutWheel(tree20, [0]), 1);
        for (const i of [0, 7, n - 1]) {
            const fan = ringOf(layoutWheel(tree20, [0, i], w), 1);
            expect(total(fan)).toBeCloseTo(total(base), 9);
            expect(fan[0].a0).toBeCloseTo(base[0].a0, 9);
            const end = fan[n - 1].a0 + fan[n - 1].span;
            expect(end).toBeCloseTo(base[0].a0 + 2 * Math.PI, 9);
        }
    });

    it('shrinks every sibling by the same amount', () => {
        const n = 12;
        const tree12 = packs(n);
        const names = Array.from({ length: n }, (_, i) => `p${i}`);
        const w = widthsOf(names, crowding(n));
        const fan = ringOf(layoutWheel(tree12, [0, 4], w), 1);
        const siblings = fan.filter((_, i) => i !== 4).map(s => s.span);
        for (const span of siblings) expect(span).toBeCloseTo(siblings[0], 12);
        expect(fan[4].span - 2 * Math.PI / n)
            .toBeCloseTo((n - 1) * (2 * Math.PI / n - siblings[0]), 12);
    });

    it('widens by one amount for the whole fan, not one per sector', () => {
        // A fan holding one long name and one short: selecting the short-named
        // member must widen it by exactly as much as selecting the long-named
        // one. This is the property the wheel's stability rests on, and it is
        // the one an "optimization" would most naturally break: why widen a
        // sector that does not need it? Because otherwise the pointer skips
        // sectors it swept across. See the machine suite's sweep tests.
        const tree10 = packs(10);
        const w = new Map([['p0', 200], ['p1', 5]]);
        const a = ringOf(layoutWheel(tree10, [0, 0], w), 1);
        const b = ringOf(layoutWheel(tree10, [0, 1], w), 1);
        expect(b[1].span).toBeCloseTo(a[0].span, 12);
    });

    it('leaves a one-member fan alone, without producing NaN', () => {
        const one = packs(1);
        const w = widthsOf(['p0'], 500);
        const fan = ringOf(layoutWheel(one, [0, 0], w), 1);
        expect(fan).toHaveLength(1);
        expect(Number.isFinite(fan[0].a0)).toBe(true);
        expect(Number.isFinite(fan[0].span)).toBe(true);
        expect(fan[0].span).toBeCloseTo(2 * Math.PI, 12);
    });

    it('closes a full-circumference fan on its own seam', () => {
        const n = 30;
        const tree30 = packs(n);
        const names = Array.from({ length: n }, (_, i) => `p${i}`);
        const fan = ringOf(layoutWheel(tree30, [0, 11], widthsOf(names, crowding(n))), 1);
        // Contiguous, with no gap and no overlap, to accumulation error: the
        // re-lay sums spans where the even layout multiplies.
        for (let i = 1; i < n; i++) {
            expect(fan[i].a0).toBeCloseTo(fan[i - 1].a0 + fan[i - 1].span, 9);
        }
    });

    it('holds siblings above the shrink floor and the selection below a half turn', () => {
        const n = 8;
        const tree8 = packs(n);
        const names = Array.from({ length: n }, (_, i) => `p${i}`);
        // Far more demand than the fan can pay for.
        const fan = ringOf(layoutWheel(tree8, [0, 2], widthsOf(names, 5000)), 1);
        const w = 2 * Math.PI / n;
        expect(fan[2].span).toBeLessThanOrEqual(Math.PI + 1e-12);
        for (const [i, s] of fan.entries()) {
            if (i !== 2) expect(s.span).toBeGreaterThanOrEqual(w * 0.5 - 1e-12);
        }
    });

    it('centres a widened pack’s brush fan under it, and leaves ring 0 alone', () => {
        const n = 24;
        const tree24 = packs(n);
        const names = Array.from({ length: n }, (_, i) => `p${i}`);
        const w = widthsOf(names, crowding(n));
        const plain = layoutWheel(tree24, [0, 5]);
        const wide = layoutWheel(tree24, [0, 5], w);
        expect(ringOf(wide, 0)).toEqual(ringOf(plain, 0));
        const pack = ringOf(wide, 1)[5];
        const brushes = ringOf(wide, 2);
        expect(brushes.length).toBeGreaterThan(0);
        const centre = midAngle(brushes[0]) + (midAngle(brushes[brushes.length - 1])
            - midAngle(brushes[0])) / 2;
        expect(centre).toBeCloseTo(midAngle(pack), 9);
    });

    it('still resolves every sector it draws', () => {
        // The "paint and hit can never disagree" invariant, asserted against a
        // widened layout rather than an even one.
        const n = 30;
        const tree30 = packs(n);
        const names = Array.from({ length: n }, (_, i) => `p${i}`);
        landsOnItself(layoutWheel(tree30, [0, 17], widthsOf(names, crowding(n))));
    });

    it('budgets against the tighter of the two label radii', () => {
        // `labelArc` picks its radius from which half of the wheel a sector's
        // midpoint falls in, and widening moves that midpoint. Budgeting
        // against the roomier radius would widen a sector by exactly enough to
        // leave its name still too long.
        const n = 16;
        const tree16 = packs(n);
        const names = Array.from({ length: n }, (_, i) => `p${i}`);
        const px = crowding(n);
        for (let i = 0; i < n; i++) {
            const s = ringOf(layoutWheel(tree16, [0, i], widthsOf(names, px)), 1)[i];
            expect(labelRadius(s)).toBeLessThanOrEqual((s.r0 + s.r1) / 2);
            expect(labelArcLen(s) + 1e-9)
                .toBeGreaterThanOrEqual(labelDemand(markWidth(s.node), px));
        }
    });
});

describe('fan demand', () => {
    const names = ['b0', 'b1', 'b2', 'b3', 'b4'];

    it('buys a small fan enough arc for the longest name it holds', () => {
        // Five brushes across the Recent branch's two thirds of ring 0 is the
        // shipped shape (RECENT_COUNT). A fan whose total is settled by its
        // member count and its parent alone caps the name it can draw at
        // whatever those two happen to allow, which for this fan is 118 px,
        // and drops every longer name in silence.
        const px = 160;
        const fan = ringOf(
            layoutWheel(recentTree(names), [0], widthsOf(names, px), [0, 3]), 1);
        // The arc carries the claim. `showsName` is the more readable
        // assertion and the weaker one: it asks whether the name was drawn,
        // where this asks whether the room to draw it was ever bought.
        expect(labelDemand(CHIP_ARC, px))
            .toBeLessThanOrEqual(labelArcLen(fan[3]) + 1e-6);
        expect(fan[3].showsName).toBe(true);
    });

    it('deals the same fan whichever member the pen is on', () => {
        // The wheel's stability rests on this. A fan's total is what the next
        // frame hit-tests, so a total that moved with the selection would
        // carry sectors out from under a pointer that had just entered them.
        const px = 160;
        const fans = [0, 2, 4].map(i =>
            ringOf(layoutWheel(recentTree(names), [0], widthsOf(names, px), [0, i]), 1));
        for (const fan of fans.slice(1)) {
            expect(total(fan)).toBeCloseTo(total(fans[0]), 12);
            expect(fan[0].a0).toBeCloseTo(fans[0][0].a0, 12);
        }
        for (const [k, i] of [0, 2, 4].entries()) {
            expect(fans[k][i].span).toBeCloseTo(fans[0][0].span, 12);
            expect(fans[k][i].showsName).toBe(true);
        }
    });

    it('leaves a fan at its own size when the name is unreachable anyway', () => {
        // A fit is all or nothing, so arc bought short of what the run needs
        // buys nothing at all: the name is as undrawn at a half turn as it was
        // at a quarter, and the fan has swallowed its ring for it.
        const two = ['b0', 'b1'];
        const fan = ringOf(
            layoutWheel(recentTree(two), [0], widthsOf(two, 600), [0, 1]), 1);
        // The total, not the division: the widening still moves the room a
        // fan already has around inside it, and still cannot make the name fit.
        expect(total(fan)).toBeCloseTo(total(ringOf(layoutWheel(recentTree(two), [0]), 1)), 12);
        expect(fan.some(s => s.showsName)).toBe(false);
    });

    it('is exactly the even layout when a bounded fan has no measured name', () => {
        const tree = recentTree(names);
        expect(layoutWheel(tree, [0], new Map(), [0, 2])).toEqual(layoutWheel(tree, [0]));
    });

    it('does not grow a fan whose names already fit', () => {
        const tree = recentTree(names);
        expect(placement(layoutWheel(tree, [0], widthsOf(names, 5), [0, 2])))
            .toEqual(placement(layoutWheel(tree, [0], new Map(), [0, 2])));
    });

    it('holds a section to its registered arc however long its names', () => {
        // Ring 0's arcs are a spatial contract: colours below, brushes across
        // the top. A section that grew would move its neighbour around the
        // wheel and steal the arc to do it.
        const tree = recentTree(names);
        const wide = ringOf(layoutWheel(tree, [0], widthsOf(['recent', 'library'], 900)), 0);
        expect(total(wide)).toBeCloseTo((4 * Math.PI) / 3, 12);
    });

    it('hit-tests a demand-sized fan back to itself', () => {
        landsOnItself(layoutWheel(recentTree(names), [0], widthsOf(names, 160), [0, 3]));
    });
});
