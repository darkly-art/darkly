import { describe, it, expect } from 'vitest';
import {
    layoutWheel,
    sectorAt,
    advance,
    hitKey,
    HUB_R,
    RING_T,
    CHILD_STEP,
    type SectorGeom,
    type Hit,
} from '../wheel_geometry';
import type { WheelBranch, WheelLeaf, WheelNode, WheelTree } from '../model';

const leaf = (id: string): WheelLeaf =>
    ({ kind: 'leaf', id, label: id, visual: { kind: 'icon', icon: '' }, select: () => {} });
const branch = (id: string, children: WheelNode[]): WheelBranch =>
    ({ kind: 'branch', id, label: id, visual: { kind: 'icon', icon: '' }, children });

/** 4 color leaves below; above, a 3-leaf branch and a branch whose first
 *  child is itself a branch (depth 3). Root order: bottom 0-3, top 4-5. */
const tree: WheelTree = {
    bottom: [leaf('c0'), leaf('c1'), leaf('c2'), leaf('c3')],
    top: [
        branch('recent', [leaf('r0'), leaf('r1'), leaf('r2')]),
        branch('dry', [branch('charcoals', [leaf('k0'), leaf('k1')]), leaf('d1')]),
    ],
};

const ring = (layout: SectorGeom[], k: number) => layout.filter(s => s.ring === k);

/** A point inside sector geometry: polar at the sector's angular middle. */
const at = (theta: number, r: number): [number, number] =>
    [r * Math.cos(theta), r * Math.sin(theta)];

describe('layoutWheel ring 0', () => {
    const layout = layoutWheel(tree, []);

    it('splits each half evenly among its nodes', () => {
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

    it('leaves an empty half sectorless', () => {
        const empties = layoutWheel({ top: tree.top, bottom: [] }, []);
        expect(ring(empties, 0).every(s => s.path[0] >= 0)).toBe(true);
        expect(ring(empties, 0)).toHaveLength(2);
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
            bottom: tree.bottom,
            top: [branch('wide', Array.from({ length: 10 }, (_, i) => leaf(`w${i}`)))],
        };
        const fan = ring(layoutWheel(wide, [4]), 1);
        // 10 · 22.5° = 225° clamps to 180°.
        expect(fan[0].span * fan.length).toBeCloseTo(Math.PI, 9);

        const six: WheelTree = {
            bottom: tree.bottom,
            top: [branch('six', Array.from({ length: 8 }, (_, i) => leaf(`s${i}`))), branch('other', [leaf('o')])],
        };
        const fan8 = ring(layoutWheel(six, [4]), 1);
        // 8 · 22.5° = 180°: exactly at the clamp, wider than the 90° parent.
        expect(fan8[0].span * fan8.length).toBeCloseTo(8 * CHILD_STEP, 9);
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
        const rMid1 = HUB_R + RING_T * 1.5;
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
        const [x, y] = at(0, HUB_R + RING_T * 1.5);
        expect(sectorAt(layout, x, y)).toEqual({ kind: 'gap', ring: 1 });
    });

    it('resolves an empty half to a gap on ring 0', () => {
        const layout = layoutWheel({ top: tree.top, bottom: [] }, []);
        const [x, y] = at(Math.PI / 2, HUB_R + 10);
        expect(sectorAt(layout, x, y)).toEqual({ kind: 'gap', ring: 0 });
    });

    it('hit-tests wrap-aware across the ±π seam', () => {
        // A fan of 8 around the top-left parent (mid -3π/4) spans π: its
        // start angle -5π/4 wraps past the seam, so theta just above +3π/4
        // (the wrapped image of the fan's first slice) must hit child 0.
        const wide: WheelTree = {
            bottom: tree.bottom,
            top: [branch('wide', Array.from({ length: 8 }, (_, i) => leaf(`w${i}`))), branch('other', [leaf('o')])],
        };
        const layout = layoutWheel(wide, [4]);
        const [x, y] = at(0.8 * Math.PI, HUB_R + RING_T * 1.5);
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
