import { describe, it, expect } from 'vitest';
import { reduce, CLOSED, type MachineState } from '../machine';
import { HUB_R, RING_T } from '../wheel_geometry';
import type { WheelBranch, WheelLeaf, WheelNode, WheelTree } from '../model';

const leaf = (id: string): WheelLeaf =>
    ({ kind: 'leaf', id, label: id, visual: { kind: 'icon', icon: '' }, select: () => {} });
const branch = (id: string, children: WheelNode[]): WheelBranch =>
    ({ kind: 'branch', id, label: id, visual: { kind: 'icon', icon: '' }, children });

/** Same shape as the geometry suite's fixture: 4 color leaves below, a
 *  3-leaf Recent branch and a depth-3 branch above. Root order 0-3, 4, 5. */
const tree: WheelTree = {
    bottom: [leaf('c0'), leaf('c1'), leaf('c2'), leaf('c3')],
    top: [
        branch('recent', [leaf('r0'), leaf('r1'), leaf('r2')]),
        branch('dry', [branch('charcoals', [leaf('k0'), leaf('k1')]), leaf('d1')]),
    ],
};

const PID = 7;
const CENTER = { x: 640, y: 360 };

/** A move event at polar (theta, r) around CENTER. */
const moveAt = (theta: number, r: number, pointerId = PID) => ({
    kind: 'move' as const,
    pointerId,
    x: CENTER.x + r * Math.cos(theta),
    y: CENTER.y + r * Math.sin(theta),
});

const down = (pointerId = PID, x = CENTER.x, y = CENTER.y) =>
    ({ kind: 'down' as const, pointerId, x, y });
const up = (pointerId = PID) => ({ kind: 'up' as const, pointerId });

const RING0_MID = HUB_R + RING_T / 2;
const RING1_MID = HUB_R + RING_T * 1.5;
/** Middle of the Recent branch's ring-0 sector (top half, first sector). */
const RECENT_MID = -3 * Math.PI / 4;
/** Middle of the first color leaf's ring-0 sector (bottom half). */
const COLOR_MID = Math.PI / 8;

const engaged = (s: MachineState) => {
    expect(s.kind).toBe('engaged');
    return s as Extract<MachineState, { kind: 'engaged' }>;
};

describe('opening', () => {
    it('DOWN opens centered exactly at the pen-down point, unclamped', () => {
        // Coordinates near a viewport corner: no clamping ever moves them.
        const { state } = reduce(CLOSED, down(PID, 3, 2), tree);
        const e = engaged(state);
        expect(e.center).toEqual({ x: 3, y: 2 });
        expect(e.path).toEqual([]);
        expect(e.highlight).toEqual({ kind: 'hub' });
        expect(e.pointerId).toBe(PID);
    });

    it('MOVE and UP while closed are no-ops (guard-suppressed opens)', () => {
        expect(reduce(CLOSED, moveAt(0, 100), tree).state).toBe(CLOSED);
        const r = reduce(CLOSED, up(), tree);
        expect(r.state).toBe(CLOSED);
        expect(r.effect).toBeUndefined();
    });

    it('a second DOWN while engaged is ignored', () => {
        const s1 = reduce(CLOSED, down(), tree).state;
        const s2 = reduce(s1, down(9, 0, 0), tree).state;
        expect(s2).toBe(s1);
    });
});

describe('threading', () => {
    it('one MOVE into a branch expands it with no intervening event', () => {
        const s1 = reduce(CLOSED, down(), tree).state;
        const s2 = engaged(reduce(s1, moveAt(RECENT_MID, RING0_MID), tree).state);
        expect(s2.path).toEqual([4]);
        expect(s2.highlight.kind).toBe('sector');
    });

    it('threads outward to a ring-1 leaf and back inward', () => {
        let s = reduce(CLOSED, down(), tree).state;
        s = reduce(s, moveAt(RECENT_MID, RING0_MID), tree).state;
        s = reduce(s, moveAt(RECENT_MID, RING1_MID), tree).state;
        const out = engaged(s);
        expect(out.path).toEqual([4]);
        const h = out.highlight;
        expect(h.kind === 'sector' && h.sector.node.kind === 'leaf').toBe(true);
        // Back inward onto a different ring-0 branch: subtree swaps in one step.
        s = reduce(s, moveAt(-Math.PI / 4, RING0_MID), tree).state;
        expect(engaged(s).path).toEqual([5]);
    });

    it('ignores MOVE and UP from non-latched pointers', () => {
        const s1 = reduce(CLOSED, down(), tree).state;
        const s2 = reduce(s1, moveAt(RECENT_MID, RING0_MID, 9), tree).state;
        expect(s2).toBe(s1);
        const r = reduce(s1, up(9), tree);
        expect(r.state).toBe(s1);
        expect(r.effect).toBeUndefined();
    });
});

describe('release', () => {
    it('UP with a leaf highlighted commits exactly that leaf and closes', () => {
        let s = reduce(CLOSED, down(), tree).state;
        s = reduce(s, moveAt(RECENT_MID, RING0_MID), tree).state;
        s = reduce(s, moveAt(RECENT_MID, RING1_MID), tree).state;
        const leafPath = (engaged(s).highlight as any).sector.path;
        const r = reduce(s, up(), tree);
        expect(r.state).toBe(CLOSED);
        expect(r.effect).toEqual({ kind: 'commit', path: leafPath });
    });

    it('commit derives from the last-MOVE highlight: DOWN then UP with zero movement cancels over the hub', () => {
        const s = reduce(CLOSED, down(), tree).state;
        const r = reduce(s, up(), tree);
        expect(r.state).toBe(CLOSED);
        expect(r.effect).toBeUndefined();
    });

    it('UP over a ring-0 color leaf commits it', () => {
        let s = reduce(CLOSED, down(), tree).state;
        s = reduce(s, moveAt(COLOR_MID, RING0_MID), tree).state;
        const r = reduce(s, up(), tree);
        expect(r.effect).toEqual({ kind: 'commit', path: [0] });
    });

    it('UP over a branch cancels', () => {
        let s = reduce(CLOSED, down(), tree).state;
        s = reduce(s, moveAt(RECENT_MID, RING0_MID), tree).state;
        const r = reduce(s, up(), tree);
        expect(r.state).toBe(CLOSED);
        expect(r.effect).toBeUndefined();
    });

    it('UP over a gap cancels', () => {
        let s = reduce(CLOSED, down(), tree).state;
        s = reduce(s, moveAt(RECENT_MID, RING0_MID), tree).state;
        // Ring-1 radius at an angle far outside the fan: a gap.
        s = reduce(s, moveAt(Math.PI / 8, RING1_MID), tree).state;
        expect(engaged(s).highlight.kind).toBe('gap');
        const r = reduce(s, up(), tree);
        expect(r.state).toBe(CLOSED);
        expect(r.effect).toBeUndefined();
    });

    it('CANCEL closes from engaged without committing', () => {
        let s = reduce(CLOSED, down(), tree).state;
        s = reduce(s, moveAt(COLOR_MID, RING0_MID), tree).state;
        const r = reduce(s, { kind: 'cancel' }, tree);
        expect(r.state).toBe(CLOSED);
        expect(r.effect).toBeUndefined();
    });
});
