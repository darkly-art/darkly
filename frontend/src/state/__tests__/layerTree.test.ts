import { describe, it, expect } from 'vitest';
import {
    appearedRoots,
    collapsedAncestorsOf,
    indexLayerTree,
    nextActiveAfterRemoval,
} from '../layerTree';

function layer(id: number, extra: Record<string, unknown> = {}) {
    return { type: 'raster', id, name: `l${id}`, visible: true, modifiers: [], ...extra };
}
function group(id: number, children: unknown[], extra: Record<string, unknown> = {}) {
    return {
        type: 'group', id, name: `g${id}`, visible: true, collapsed: false,
        children, modifiers: [], ...extra,
    };
}
function mask(id: number) {
    return { type: 'mask', id, name: `m${id}` };
}

/** Sibling list order matches the panel: index 0 is the top of the stack. */
const flat = [layer(3), layer(2), layer(1)];

describe('indexLayerTree', () => {
    it('collects every selectable id at any depth, modifiers included', () => {
        const index = indexLayerTree([
            group(100, [layer(1, { modifiers: [mask(900)] })]),
            layer(2),
        ]);
        expect([...index.ids].sort((a, b) => a - b)).toEqual([1, 2, 100, 900]);
    });

    it('orders rows top to bottom with each host followed by its modifiers', () => {
        const index = indexLayerTree([
            layer(3, { modifiers: [mask(900)] }),
            group(100, [layer(1), layer(2)]),
        ]);
        expect(index.order).toEqual([3, 900, 100, 1, 2]);
    });

    it('descends into collapsed groups for `order` but not `visibleOrder`', () => {
        const tree = [group(100, [layer(1), layer(2)], { collapsed: true }), layer(3)];
        const index = indexLayerTree(tree);
        expect(index.order).toEqual([100, 1, 2, 3]);
        expect(index.visibleOrder).toEqual([100, 3]);
        expect([...index.collapsed]).toEqual([100]);
    });

    it('hides rows nested under any collapsed ancestor', () => {
        const index = indexLayerTree([
            group(100, [group(200, [layer(1)])], { collapsed: true }),
        ]);
        expect(index.order).toEqual([100, 200, 1]);
        expect(index.visibleOrder).toEqual([100]);
    });

    it('records a modifier’s parent as its host and its siblings as the host’s modifiers', () => {
        const index = indexLayerTree([layer(1, { modifiers: [mask(900), mask(901)] })]);
        expect(index.slots.get(900)).toEqual({ parent: 1, siblings: [900, 901] });
        expect(index.slots.get(1)?.parent).toBeNull();
    });

    it('tolerates a malformed tree', () => {
        const index = indexLayerTree([null, undefined, { name: 'no id' }, layer(1)] as any);
        expect([...index.ids]).toEqual([1]);
        expect(indexLayerTree(undefined as any).ids.size).toBe(0);
    });
});

describe('nextActiveAfterRemoval', () => {
    const prev = indexLayerTree(flat);

    it('prefers the sibling below', () => {
        expect(nextActiveAfterRemoval(prev, new Set([3, 1]), 2)).toBe(1);
    });

    it('falls back to the sibling above for the bottom-most row', () => {
        expect(nextActiveAfterRemoval(prev, new Set([3, 2]), 1)).toBe(2);
    });

    it('skips dead siblings on the way down, then on the way up', () => {
        const wide = indexLayerTree([layer(5), layer(4), layer(3), layer(2), layer(1)]);
        expect(nextActiveAfterRemoval(wide, new Set([5, 1]), 3)).toBe(1);
        expect(nextActiveAfterRemoval(wide, new Set([5, 4]), 3)).toBe(4);
    });

    it('adopts the parent group when no sibling survives', () => {
        const nested = indexLayerTree([group(100, [layer(1)]), layer(2)]);
        expect(nextActiveAfterRemoval(nested, new Set([100, 2]), 1)).toBe(100);
    });

    it('stays inside the group when a sibling survives', () => {
        const nested = indexLayerTree([group(100, [layer(1), layer(2)]), layer(3)]);
        expect(nextActiveAfterRemoval(nested, new Set([100, 2, 3]), 1)).toBe(2);
    });

    it('escalates to the parent’s own level when the parent died too', () => {
        const nested = indexLayerTree([group(100, [layer(1)]), layer(2)]);
        expect(nextActiveAfterRemoval(nested, new Set([2]), 1)).toBe(2);
    });

    it('escalates through two dead levels', () => {
        const deep = indexLayerTree([
            group(100, [group(200, [layer(1)])]),
            layer(2),
        ]);
        expect(nextActiveAfterRemoval(deep, new Set([2]), 1)).toBe(2);
    });

    it('adopts a sibling modifier, then the host', () => {
        const withMods = indexLayerTree([layer(1, { modifiers: [mask(900), mask(901)] })]);
        expect(nextActiveAfterRemoval(withMods, new Set([1, 901]), 900)).toBe(901);
        expect(nextActiveAfterRemoval(withMods, new Set([1]), 900)).toBe(1);
    });

    it('gives up when nothing survives at any level', () => {
        expect(nextActiveAfterRemoval(prev, new Set(), 2)).toBeNull();
    });

    it('gives up for an id the previous tree never had', () => {
        expect(nextActiveAfterRemoval(prev, new Set([3, 2, 1]), 999)).toBeNull();
    });
});

describe('collapsedAncestorsOf', () => {
    it('returns nothing for a visible row', () => {
        const index = indexLayerTree([group(100, [layer(1)]), layer(2)]);
        expect(collapsedAncestorsOf(index, 1)).toEqual([]);
        expect(collapsedAncestorsOf(index, 2)).toEqual([]);
    });

    it('names the collapsed group hiding a row', () => {
        const index = indexLayerTree([group(100, [layer(1)], { collapsed: true })]);
        expect(collapsedAncestorsOf(index, 1)).toEqual([100]);
    });

    it('names every collapsed ancestor, outermost first', () => {
        const index = indexLayerTree([
            group(100, [group(200, [layer(1)], { collapsed: true })], { collapsed: true }),
        ]);
        expect(collapsedAncestorsOf(index, 1)).toEqual([100, 200]);
    });

    it('skips expanded groups in the chain', () => {
        const index = indexLayerTree([
            group(100, [group(200, [layer(1)])], { collapsed: true }),
        ]);
        expect(collapsedAncestorsOf(index, 1)).toEqual([100]);
    });

    it('returns nothing for an absent id', () => {
        expect(collapsedAncestorsOf(indexLayerTree(flat), 999)).toEqual([]);
    });
});

describe('appearedRoots', () => {
    it('finds a single new row', () => {
        const prev = indexLayerTree([layer(3), layer(1)]);
        const next = indexLayerTree(flat);
        expect(appearedRoots(prev, next)).toEqual([2]);
    });

    it('finds several new rows in panel order, topmost first', () => {
        const prev = indexLayerTree([layer(1)]);
        const next = indexLayerTree(flat);
        expect(appearedRoots(prev, next)).toEqual([3, 2]);
    });

    it('keeps only the subtree root when a whole group comes back', () => {
        const prev = indexLayerTree([layer(9)]);
        const next = indexLayerTree([
            group(100, [layer(1, { modifiers: [mask(900)] }), layer(2)]),
            layer(9),
        ]);
        expect(appearedRoots(prev, next)).toEqual([100]);
    });

    it('keeps a restored modifier whose host survived', () => {
        const prev = indexLayerTree([layer(1)]);
        const next = indexLayerTree([layer(1, { modifiers: [mask(900)] })]);
        expect(appearedRoots(prev, next)).toEqual([900]);
    });

    it('returns nothing when the tree only shrank or is unchanged', () => {
        const prev = indexLayerTree(flat);
        expect(appearedRoots(prev, indexLayerTree([layer(3), layer(1)]))).toEqual([]);
        expect(appearedRoots(prev, indexLayerTree(flat))).toEqual([]);
    });
});
