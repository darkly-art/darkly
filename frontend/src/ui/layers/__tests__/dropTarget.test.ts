import { describe, it, expect } from 'vitest';
import {
    ROW_BASE_PAD,
    ROW_INDENT,
    bandToGap,
    gapDepthRange,
    resolveGapDrop,
    rootGapIndex,
} from '../dropTarget';
import type { DropRow } from '../../../state/layerTree';

function row(id: number, depth: number, isGroup = false): DropRow {
    return { id, depth, isGroup };
}

/** The pointer offset that asks for `depth`, in the panel's indent arithmetic. */
function xFor(depth: number): number {
    return ROW_BASE_PAD + depth * ROW_INDENT;
}

/**
 * The report's tree: one group at root holding one layer.
 *
 *   g1        depth 0, group
 *     l2      depth 1
 */
const singleGroup: DropRow[] = [row(1, 0, true), row(2, 1)];

/**
 *   g1          depth 0, group
 *     g2        depth 1, group
 *       l3      depth 2
 *   l4          depth 0
 */
const nested: DropRow[] = [row(1, 0, true), row(2, 1, true), row(3, 2), row(4, 0)];

describe('gapDepthRange', () => {
    it('opens to the root at both ends of the list', () => {
        expect(gapDepthRange(singleGroup, 0)).toEqual({ min: 0, max: 0 });
        expect(gapDepthRange(nested, 0)).toEqual({ min: 0, max: 0 });
    });

    it('lets the gap below the last child escape to any enclosing depth', () => {
        // Below l2, with nothing after it: the row above is at depth 1, and
        // there is no row below to hold the floor up.
        expect(gapDepthRange(singleGroup, 2)).toEqual({ min: 0, max: 1 });
    });

    it('offers one level deeper below a group header', () => {
        // Gap between g1 and l2: the row below pins the floor at its own depth,
        // and g1 being a group would allow depth 1 — the same number.
        expect(gapDepthRange(singleGroup, 1)).toEqual({ min: 1, max: 1 });
    });

    it('is degenerate between two siblings at the same depth', () => {
        const rows = [row(1, 0), row(2, 0)];
        expect(gapDepthRange(rows, 1)).toEqual({ min: 0, max: 0 });
    });

    it('spans every level being closed at once', () => {
        // Below l3 (depth 2) and above l4 (depth 0): the drop may land inside
        // g2, inside g1, or at root.
        expect(gapDepthRange(nested, 3)).toEqual({ min: 0, max: 2 });
    });

    it('never lets max fall below min', () => {
        // A collapsed group's header followed by a deeper row cannot happen,
        // but an out-of-order list must still yield a usable range.
        const rows = [row(1, 0), row(2, 3)];
        const range = gapDepthRange(rows, 1);
        expect(range.max).toBeGreaterThanOrEqual(range.min);
    });
});

describe('bandToGap', () => {
    it('splits a leaf row at its midpoint, naming no depth', () => {
        expect(bandToGap(2, false, 0.1)).toEqual({ band: 'above', gap: 2 });
        expect(bandToGap(2, false, 0.9)).toEqual({ band: 'below', gap: 3 });
    });

    it('gives a group header a third band for its interior', () => {
        expect(bandToGap(0, true, 0.1).band).toBe('above');
        expect(bandToGap(0, true, 0.5).band).toBe('into');
        expect(bandToGap(0, true, 0.9).band).toBe('below');
    });

    it('resolves the into band to the gap below the header', () => {
        expect(bandToGap(0, true, 0.5)).toEqual({ band: 'into', gap: 1, pin: 'max' });
    });
});

describe('resolveGapDrop', () => {
    it('returns null for an empty panel', () => {
        expect(resolveGapDrop([], 0, 0)).toBeNull();
    });

    it('drops as the first child of the group above', () => {
        // Gap below g1's header, pinned deep: become g1's top child.
        const res = resolveGapDrop(singleGroup, 1, xFor(1), 'max');
        expect(res).toEqual({ depth: 1, target: { target_type: 'into_top', target_id: 1 } });
    });

    it('escapes the group when the pointer asks for a shallower depth', () => {
        // The reported bug: below l2, dragging left to depth 0 must land the
        // row below g1 rather than as l2's sibling.
        const res = resolveGapDrop(singleGroup, 2, xFor(0));
        expect(res).toEqual({ depth: 0, target: { target_type: 'before', target_id: 1 } });
    });

    it('stays inside the group when the pointer asks for the deeper depth', () => {
        const res = resolveGapDrop(singleGroup, 2, xFor(1));
        expect(res).toEqual({ depth: 1, target: { target_type: 'before', target_id: 2 } });
    });

    it('clamps an X reading past either end of the legal range', () => {
        expect(resolveGapDrop(singleGroup, 2, xFor(-5))!.depth).toBe(0);
        expect(resolveGapDrop(singleGroup, 2, xFor(99))!.depth).toBe(1);
    });

    it('picks the ancestor matching the requested depth through several levels', () => {
        // Below l3 at depth 2, asking for depth 1: land below g2, still in g1.
        expect(resolveGapDrop(nested, 3, xFor(1))).toEqual({
            depth: 1,
            target: { target_type: 'before', target_id: 2 },
        });
        // Asking for depth 0: land below g1, at root.
        expect(resolveGapDrop(nested, 3, xFor(0))).toEqual({
            depth: 0,
            target: { target_type: 'before', target_id: 1 },
        });
    });

    it('drops above the first row when the gap is the top of the list', () => {
        expect(resolveGapDrop(singleGroup, 0, xFor(0))).toEqual({
            depth: 0,
            target: { target_type: 'after', target_id: 1 },
        });
    });

    it('clamps a gap index outside the list', () => {
        expect(resolveGapDrop(singleGroup, 99, xFor(0), 'min')!.target).toEqual({
            target_type: 'before',
            target_id: 1,
        });
        expect(resolveGapDrop(singleGroup, -3, xFor(0), 'min')!.target).toEqual({
            target_type: 'after',
            target_id: 1,
        });
    });

    it('honours an explicit pin over the X reading', () => {
        // Same gap and a deep X, but pinned shallow: the pin wins.
        expect(resolveGapDrop(singleGroup, 2, xFor(99), 'min')!.depth).toBe(0);
        expect(resolveGapDrop(singleGroup, 2, xFor(-99), 'max')!.depth).toBe(1);
    });
});

/**
 * The reported bug, end to end through the two functions the action composes:
 * one group holding one layer, and the user drags that layer down and out.
 */
describe('the reported gesture', () => {
    it('drags the only child out of the only group', () => {
        // Lower half of l2 (row index 1), pointer swung left to the root indent.
        const band = bandToGap(1, false, 0.9);
        expect(resolveGapDrop(singleGroup, band.gap, xFor(0), band.pin)).toEqual({
            depth: 0,
            target: { target_type: 'before', target_id: 1 },
        });
    });

    it('keeps it in the group when the pointer stays at the child indent', () => {
        const band = bandToGap(1, false, 0.9);
        expect(resolveGapDrop(singleGroup, band.gap, xFor(1), band.pin)).toEqual({
            depth: 1,
            target: { target_type: 'before', target_id: 2 },
        });
    });

    it('a group into-band still names the deep end whatever X says', () => {
        const band = bandToGap(0, true, 0.5);
        expect(resolveGapDrop(singleGroup, band.gap, xFor(0), band.pin)).toEqual({
            depth: 1,
            target: { target_type: 'into_top', target_id: 1 },
        });
    });
});

describe('rootGapIndex', () => {
    it('counts past nested rows to the nth root child', () => {
        // nested: g1(0) g2(1) l3(2) l4(0) — root children are indices 0 and 3.
        expect(rootGapIndex(nested, 0)).toBe(0);
        expect(rootGapIndex(nested, 1)).toBe(3);
    });

    it('lands at the end of the list for a trailing divider', () => {
        expect(rootGapIndex(nested, 2)).toBe(nested.length);
        expect(rootGapIndex(nested, 99)).toBe(nested.length);
    });

    it('is the end of an empty list', () => {
        expect(rootGapIndex([], 0)).toBe(0);
    });
});
