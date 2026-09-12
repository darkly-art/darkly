import { describe, it, expect } from 'vitest';
import { reduce, CLOSED, type MachineState } from '../machine';
import {
    HUB_R,
    RING_T,
    hitKey,
    layoutWheel,
    midAngle,
    selectionPath,
} from '../wheel_geometry';
import type { WheelBranch, WheelLeaf, WheelNode, WheelTree } from '../model';
import { NEUTRAL_PALETTE } from '../../../lib/packPalette';
import { recentTree } from './trees';

const paint = { visual: { kind: 'icon', icon: '' }, palette: NEUTRAL_PALETTE } as const;
const leaf = (id: string): WheelLeaf =>
    ({ kind: 'leaf', id, label: id, ...paint, select: () => {} });
const branch = (id: string, children: WheelNode[]): WheelBranch =>
    ({ kind: 'branch', id, label: id, ...paint, children });

/** Same shape as the geometry suite's fixture: two half-arc sections, 4
 *  color leaves below, a 3-leaf Recent branch and a depth-3 branch above.
 *  Root order 0-3, 4, 5. */
const tree: WheelTree = {
    sections: [
        { a0: 0, span: Math.PI, nodes: [leaf('c0'), leaf('c1'), leaf('c2'), leaf('c3')] },
        {
            a0: -Math.PI,
            span: Math.PI,
            nodes: [
                branch('recent', [leaf('r0'), leaf('r1'), leaf('r2')]),
                branch('dry', [branch('charcoals', [leaf('k0'), leaf('k1')]), leaf('d1')]),
            ],
        },
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
const RING1_MID = HUB_R + RING_T + RING_T / 2;
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
        expect(e.cursor).toEqual({ x: 3, y: 2 });
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

    it('tracks the latched pointer as the cursor', () => {
        const s1 = engaged(reduce(CLOSED, down(), tree).state);
        expect(s1.cursor).toEqual(CENTER);
        const s2 = engaged(
            reduce(s1, { kind: 'move', pointerId: PID, x: 700, y: 400 }, tree).state);
        expect(s2.cursor).toEqual({ x: 700, y: 400 });
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

// ---------------------------------------------------------------------------
// Stability of the widened fan.
//
// The wheel paints the layout it hit-tests, so the layout a frame draws is the
// one the next pointer sample is resolved against. That is a loop, and these
// are the tests that keep it closed: a pointer must never skip a sector it
// swept across, and must never deselect what it just selected.
// ---------------------------------------------------------------------------

/** `n` packs under one full-circumference Library branch: the Library's own
 *  shape, and the only fan on the wheel whose size is unbounded. */
const packNames = (n: number) => Array.from({ length: n }, (_, i) => `p${i}`);
const packTree = (n: number): WheelTree => ({
    sections: [{
        a0: 0,
        span: 2 * Math.PI,
        nodes: [{
            ...branch('library', packNames(n).map(name => branch(name, [leaf(`${name}-b`)]))),
            spread: 'full' as const,
        }],
    }],
});

/** Ring 1's label circumference divided `n` ways: the width at which a name no
 *  longer fits the arc it was dealt. */
const crowding = (n: number) => 2 * Math.PI * 95.5 / n;

/** Open, drop into the ring 0 branch at `enter`, and return the engaged
 *  state. */
const intoBranch = (tree: WheelTree, widths: Map<string, number>, enter = 0) => {
    const s0 = reduce(CLOSED, down(), tree, widths).state;
    return reduce(s0, moveAt(enter, RING0_MID), tree, widths).state;
};

/** The whole of ring 1, entered at theta 0: the Library's fan, which is the
 *  only kind that occupies its ring entirely. */
const FULL_TURN = { enter: 0, a0: 0, span: 2 * Math.PI };

/** Walk the pointer once across a ring 1 fan and report the members it
 *  selected, in order, collapsing repeats.
 *
 *  Half-open in `arc`, which is what makes a full turn's wrap back onto its
 *  first sector not read as a skip, and what keeps a bounded fan's sweep inside
 *  the fan. */
const sweep = (
    tree: WheelTree,
    widths: Map<string, number>,
    steps = 4000,
    arc = FULL_TURN,
) => {
    let s = intoBranch(tree, widths, arc.enter);
    const seen: number[] = [];
    for (let t = 0; t < steps; t++) {
        const theta = arc.a0 + arc.span * t / steps;
        s = reduce(s, moveAt(theta, RING1_MID), tree, widths).state;
        const e = engaged(s);
        if (e.highlight.kind !== 'sector' || e.highlight.sector.ring !== 1) continue;
        const i = e.highlight.sector.path[1];
        if (seen[seen.length - 1] !== i) seen.push(i);
    }
    return seen;
};

/** A pack of `n` brushes, reached at ring 2 through one full-circle Library. */
const brushPackTree = (names: string[]): WheelTree => ({
    sections: [{
        a0: 0,
        span: 2 * Math.PI,
        nodes: [{
            ...branch('library', [branch('pack', names.map(n => ({
                kind: 'leaf' as const,
                id: n,
                label: n,
                visual: { kind: 'brush' as const, name: n, icon: null },
                palette: NEUTRAL_PALETTE,
                select: () => {},
            })))]),
            spread: 'full' as const,
        }],
    }],
});

describe('hovering a brush', () => {
    const names = Array.from({ length: 12 }, (_, i) => `brush${i}`);
    const widths = new Map(names.map(l => [l, 140]));
    const RING2_MID = HUB_R + 2 * RING_T + RING_T / 2;

    /** The layout as the wheel currently draws it, which is what the next
     *  pointer sample is resolved against. */
    const drawn = (tree: WheelTree, st: MachineState, w = widths) => {
        const e = engaged(st);
        return layoutWheel(tree, e.path, w, selectionPath(e.path, e.highlight));
    };

    /** Open, into the Library, then into its pack. The single pack spans the
     *  whole circle, so its brush fan is centred on the pack's own mid-angle
     *  rather than anywhere in particular: the probe angle is read off the
     *  layout instead of guessed. */
    const intoPack = (tree: WheelTree, w = widths) => {
        let s = reduce(CLOSED, down(), tree, w).state;
        s = reduce(s, moveAt(0, RING0_MID), tree, w).state;
        const pack = drawn(tree, s, w).find(g => g.ring === 1);
        return reduce(s, moveAt(midAngle(pack!), RING1_MID), tree, w).state;
    };

    it('widens the brush under the pen and names it', () => {
        // A leaf never enters the expansion path (`advance` drops it), so a
        // widening keyed on the path alone can never select one, and every
        // brush stays its resting width with no name however long the pen
        // hovers it.
        const tree = brushPackTree(names);
        let s = intoPack(tree);
        const before = drawn(tree, s).filter(g => g.ring === 2);
        expect(before.length).toBe(names.length);
        const rest = before[0].span;
        expect(before.some(g => g.showsName)).toBe(false);

        s = reduce(s, moveAt(midAngle(before[4]), RING2_MID), tree, widths).state;
        const hit = engaged(s).highlight;
        expect(hit.kind).toBe('sector');
        const idx = hit.kind === 'sector' ? hit.sector.path[2] : -1;
        expect(idx).toBe(4);

        const after = drawn(tree, s).filter(g => g.ring === 2);
        expect(after[idx].span).toBeGreaterThan(rest);
        expect(after[idx].showsName).toBe(true);
        expect(after.filter(g => g.showsName)).toHaveLength(1);
    });

    it('keeps the pack it belongs to widened while a brush is hovered', () => {
        // The other half of the same question: reaching past a pack for one of
        // its brushes must not collapse the pack ring under the pen.
        const tree = brushPackTree(names);
        let s = intoPack(tree);
        const packSpan = (st: MachineState) =>
            drawn(tree, st).filter(g => g.ring === 1)[0].span;
        const held = packSpan(s);
        const brush = drawn(tree, s).filter(g => g.ring === 2)[7];
        s = reduce(s, moveAt(midAngle(brush), RING2_MID), tree, widths).state;
        expect(engaged(s).path).toEqual([0, 0]);
        expect(packSpan(s)).toBeCloseTo(held, 9);
    });
});

describe('widened fan stability', () => {
    it('enters every pack, in order, on a slow sweep of a crowded ring', () => {
        const n = 50;
        const tree = packTree(n);
        const widths = new Map(packNames(n).map(l => [l, crowding(n)]));
        expect(sweep(tree, widths)).toEqual(packNames(n).map((_, i) => i));
    });

    it('enters every member of a bounded fan, in order, on a slow sweep', () => {
        // A bounded fan is the kind whose size its names decide: the Library's
        // packs hold the whole circumference and are dealt the same arc
        // whatever they are called, so every sweep above this one is a sweep of
        // the one fan the sizing cannot reach. This is the shape a painter
        // opens the wheel onto.
        const names = ['b0', 'b1', 'b2', 'b3', 'b4'];
        const widths = new Map(names.map(l => [l, 160]));
        const tree = recentTree(names);
        const layout = layoutWheel(tree, [0], widths, [0, 0]);
        const fan = layout.filter(g => g.ring === 1);
        const arc = {
            enter: midAngle(layout.find(g => g.ring === 0 && g.path[0] === 0)!),
            a0: fan[0].a0,
            span: fan.reduce((a, g) => a + g.span, 0),
        };
        expect(sweep(tree, widths, 4000, arc)).toEqual(names.map((_, i) => i));
    });

    it('skips nothing when one pack has a long name and its neighbour a short one', () => {
        // The standing fence against a per-sector widening amount. Widening
        // only the sectors that need it is the obvious economy, and it breaks
        // the wheel: a pointer leaving a widened sector lands past several of
        // its unwidened neighbours, which are then unreachable from that side.
        // Under a per-sector rule this sweep reads 0, 1, 3, ...; sector 2 is
        // never entered.
        const n = 10;
        const tree = packTree(n);
        const widths = new Map<string, number>([['p0', 200], ['p1', 18]]);
        expect(sweep(tree, widths)).toEqual(packNames(n).map((_, i) => i));
    });

    it('does not deselect what it just selected, when the sample is replayed', () => {
        const n = 30;
        const tree = packTree(n);
        const widths = new Map(packNames(n).map(l => [l, crowding(n)]));
        let s = intoBranch(tree, widths);
        for (let t = 0; t < 600; t++) {
            const theta = 2 * Math.PI * t / 600;
            s = reduce(s, moveAt(theta, RING1_MID), tree, widths).state;
            const first = engaged(s).highlight;
            // The identical sample again, against the layout the first one
            // produced. It must resolve to the same place.
            const again = reduce(s, moveAt(theta, RING1_MID), tree, widths).state;
            expect(hitKey(engaged(again).highlight)).toBe(hitKey(first));
            s = again;
        }
    });

    it('lands where it is aimed after a jump across the whole fan', () => {
        const n = 40;
        const tree = packTree(n);
        const widths = new Map(packNames(n).map(l => [l, crowding(n)]));
        let s = intoBranch(tree, widths);
        s = reduce(s, moveAt(0.05, RING1_MID), tree, widths).state;
        for (const theta of [Math.PI, 1.2, 5.9, 0.3, 3.7]) {
            s = reduce(s, moveAt(theta, RING1_MID), tree, widths).state;
            const landed = engaged(s).highlight;
            const again = reduce(s, moveAt(theta, RING1_MID), tree, widths).state;
            expect(hitKey(engaged(again).highlight)).toBe(hitKey(landed));
            s = again;
        }
    });

    it('keeps the pack selected while the pointer is inside its brush fan', () => {
        // The gesture the widening exists to serve: read a pack's name, then
        // reach past it for a brush. Keying the widening on the expansion path
        // rather than the momentary highlight is what stops the whole ring
        // collapsing under the pointer at that moment.
        const n = 30;
        const tree = packTree(n);
        const widths = new Map(packNames(n).map(l => [l, crowding(n)]));
        let s = intoBranch(tree, widths);
        s = reduce(s, moveAt(0.05, RING1_MID), tree, widths).state;
        const pack = engaged(s).path;
        expect(pack).toHaveLength(2);
        // Outward into the brush fan, along the pack's own mid-angle.
        const mid = engaged(s).highlight;
        expect(mid.kind).toBe('sector');
        const theta = mid.kind === 'sector'
            ? mid.sector.a0 + mid.sector.span / 2
            : 0;
        s = reduce(s, moveAt(theta, HUB_R + 2 * RING_T + RING_T / 2), tree, widths).state;
        expect(engaged(s).path).toEqual(pack);
    });

    it('is today’s geometry when nothing has been measured', () => {
        const n = 24;
        const tree = packTree(n);
        let bare = intoBranch(tree, new Map());
        let old = reduce(CLOSED, down(), tree).state;
        old = reduce(old, moveAt(0, RING0_MID), tree).state;
        for (let t = 0; t <= 200; t++) {
            const theta = 2 * Math.PI * t / 200;
            bare = reduce(bare, moveAt(theta, RING1_MID), tree, new Map()).state;
            old = reduce(old, moveAt(theta, RING1_MID), tree).state;
            expect(hitKey(engaged(bare).highlight)).toBe(hitKey(engaged(old).highlight));
        }
    });
});
