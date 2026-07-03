import { describe, it, expect } from 'vitest';
import {
    type Subdivision,
    type SplitChild,
    makeGroup,
    findGroup,
    firstGroupId,
    removeTab,
    insertTab,
    reorderTab,
    insertSplitAdjacent,
    splitPanelGroup,
    prune,
    collectPanelTypes,
    isEmptyLayout,
    foldPanelsIntoMain,
    defaultMainLayout,
    loadOrDefault,
    type PanelType,
} from '../tree';

// --- builders --------------------------------------------------------------

function split(...children: SplitChild[]): Subdivision {
    return { kind: 'split', children };
}
function slot(subdivision: Subdivision, size = 1): SplitChild {
    return { subdivision, size };
}
function group(id: number, tabs: PanelType[], active = 0): SplitChild {
    return slot(makeGroup(id, tabs, active), 1);
}

/** Shape assertion helper: flatten the tree into a nested array of tab-lists. */
function shape(node: Subdivision): unknown {
    if (node.kind === 'group') return node.state.tabs;
    return node.children.map((c) => shape(c.subdivision));
}

// --- insertSplitAdjacent ---------------------------------------------------

describe('insertSplitAdjacent', () => {
    it('inserts as a sibling when the split axis matches (horizontal at depth 0)', () => {
        const root = split(group(1, ['layers']), group(2, ['properties']));
        const ok = insertSplitAdjacent(root, 2, slot(makeGroup(3, ['history'] as unknown as PanelType[])), false, true, 0);
        expect(ok).toBe(true);
        expect(root.kind === 'split' && root.children.length).toBe(3);
        // Group 3 landed after group 2.
        expect(shape(root)).toEqual([['layers'], ['properties'], ['history']]);
    });

    it('honors insertBefore for the insertion side', () => {
        const root = split(group(1, ['layers']), group(2, ['properties']));
        insertSplitAdjacent(root, 1, slot(makeGroup(9, ['x'] as unknown as PanelType[])), true, true, 0);
        expect(shape(root)).toEqual([['x'], ['layers'], ['properties']]);
    });

    it('wraps the target in a sub-split when the axis is mismatched', () => {
        // Root is horizontal (depth 0). Requesting a vertical split (needsHorizontal=false)
        // around a direct child wraps it in a nested split.
        const root = split(group(1, ['layers']), group(2, ['properties']));
        insertSplitAdjacent(root, 1, slot(makeGroup(3, ['y'] as unknown as PanelType[])), false, false, 0);
        // child[0] is now a split of [layers, y]; child[1] is still properties.
        expect(shape(root)).toEqual([[['layers'], ['y']], ['properties']]);
    });

    it('recurses into deeper splits to find the target', () => {
        const inner = split(group(10, ['a'] as unknown as PanelType[]), group(11, ['b'] as unknown as PanelType[]));
        const root = split(slot(inner, 1), group(2, ['properties']));
        // inner is a vertical split at depth 1; a vertical insert next to group 10 is a sibling there.
        const ok = insertSplitAdjacent(root, 10, slot(makeGroup(20, ['c'] as unknown as PanelType[])), false, false, 0);
        expect(ok).toBe(true);
        expect(shape(root)).toEqual([[['a'], ['c'], ['b']], ['properties']]);
    });

    it('splits sibling sizes so they still sum to ~1', () => {
        const root = split(group(1, ['layers']));
        insertSplitAdjacent(root, 1, slot(makeGroup(2, ['properties'])), false, true, 0);
        const total = (root as { children: SplitChild[] }).children.reduce((a, c) => a + c.size, 0);
        expect(total).toBeCloseTo(1, 6);
    });
});

// --- tab ops ---------------------------------------------------------------

describe('reorderTab', () => {
    it('shifts a tab within its group and follows it with the active index', () => {
        const root = makeGroup(1, ['layers', 'properties', 'history'] as unknown as PanelType[], 0);
        reorderTab(root, 1, 0, 2);
        const g = findGroup(root, 1)!;
        expect(g.state.tabs).toEqual(['properties', 'history', 'layers']);
        expect(g.state.activeTabIndex).toBe(2);
    });
});

describe('removeTab', () => {
    it('removes and clamps the active index', () => {
        const root = makeGroup(1, ['layers', 'properties'], 1);
        removeTab(root, 1, 'properties');
        const g = findGroup(root, 1)!;
        expect(g.state.tabs).toEqual(['layers']);
        expect(g.state.activeTabIndex).toBe(0);
    });

    it('decrements the active index when an earlier tab is removed', () => {
        const root = makeGroup(1, ['layers', 'properties'], 1);
        removeTab(root, 1, 'layers');
        expect(findGroup(root, 1)!.state.activeTabIndex).toBe(0);
    });
});

describe('moving a tab across two separate trees (the cross-window path)', () => {
    it('removes from source (pruning it empty) and inserts into target', () => {
        const source = split(group(1, ['layers']), group(2, ['properties']));
        const target = split(group(3, ['history'] as unknown as PanelType[]));

        removeTab(source, 1, 'layers');
        prune(source);
        insertTab(target, 3, 'layers', 0);

        // Source lost group 1 entirely; only properties remains.
        expect(shape(source)).toEqual([['properties']]);
        expect(shape(target)).toEqual([['layers', 'history']]);
        // Inserted tab becomes active.
        expect(findGroup(target, 3)!.state.activeTabIndex).toBe(0);
    });

    it('prunes a source tree down to a single group when its last split empties', () => {
        const source = split(group(1, ['layers']));
        removeTab(source, 1, 'layers');
        prune(source);
        expect(isEmptyLayout(source)).toBe(true);
    });
});

// --- prune -----------------------------------------------------------------

describe('prune', () => {
    it('removes empty groups and renormalizes sibling sizes', () => {
        const root = split(group(1, ['layers']), group(2, []));
        // sizes 1 + 1 = 2 before; after removal one child rescaled to 1.
        prune(root);
        expect(shape(root)).toEqual([['layers']]);
        expect((root as { children: SplitChild[] }).children[0].size).toBeCloseTo(1, 6);
    });

    it('flattens a single-split-in-split, rescaling sizes and preserving axis', () => {
        // Outer horizontal split (depth 0) whose single child is a split whose
        // single child is ANOTHER split of [a 0.25, b 0.75]. The two wrappers
        // are redundant; grandchildren hoist to depth 0.
        const inner = split(
            { subdivision: makeGroup(10, ['a'] as unknown as PanelType[]), size: 0.25 },
            { subdivision: makeGroup(11, ['b'] as unknown as PanelType[]), size: 0.75 },
        );
        const root: Subdivision = {
            kind: 'split',
            children: [{ subdivision: { kind: 'split', children: [{ subdivision: inner, size: 1 }] }, size: 1 }],
        };
        prune(root);
        // Grandchildren hoisted directly into root (a shift of two depth levels).
        expect(shape(root)).toEqual([['a'], ['b']]);
        const children = (root as { children: SplitChild[] }).children;
        expect(children[0].size).toBeCloseTo(0.25, 6);
        expect(children[1].size).toBeCloseTo(0.75, 6);
    });

    it('does NOT collapse a single group-in-split', () => {
        const root = split(slot(split(group(5, ['layers'])), 1));
        prune(root);
        // The lone group stays nested (not hoisted / re-oriented).
        expect(shape(root)).toEqual([[['layers']]]);
    });

    it('renormalizes sizes that drifted from 1', () => {
        const root = split(
            { subdivision: makeGroup(1, ['layers']), size: 2 },
            { subdivision: makeGroup(2, ['properties']), size: 2 },
        );
        prune(root);
        const children = (root as { children: SplitChild[] }).children;
        expect(children[0].size + children[1].size).toBeCloseTo(1, 6);
        expect(children[0].size).toBeCloseTo(0.5, 6);
    });
});

// --- op-sequence round trip ------------------------------------------------

describe('op-sequence round trip', () => {
    it('split then move-back returns to a single group', () => {
        const layout = defaultMainLayout(0, 1);
        const root = layout.root;
        // Split properties off to the right of layers.
        const layersId = firstGroupId(root)!;
        const newId = splitPanelGroup(root, layersId, 'right', ['history'] as unknown as PanelType[], 0, 99)!;
        prune(root);
        expect(collectPanelTypes(root)).toContain('history');
        // Remove it again.
        removeTab(root, newId, 'history' as unknown as PanelType);
        prune(root);
        expect(collectPanelTypes(root).sort()).toEqual(['layers', 'properties']);
    });
});

// --- loadOrDefault ---------------------------------------------------------

describe('loadOrDefault', () => {
    function persist(...roots: Subdivision[]): string {
        return JSON.stringify({ workspaces: roots.map((root, i) => ({ id: i, layout: { root } })) });
    }

    it('falls back to the default on null', () => {
        const { root } = loadOrDefault(null);
        expect(collectPanelTypes(root).sort()).toEqual(['layers', 'properties']);
    });

    it('falls back to the default on malformed JSON', () => {
        const { root } = loadOrDefault('{not json');
        expect(collectPanelTypes(root).sort()).toEqual(['layers', 'properties']);
    });

    it('strips unknown panel types', () => {
        const { root } = loadOrDefault(persist(split(group(0, ['layers', 'bogus'] as unknown as PanelType[]))));
        expect(collectPanelTypes(root)).toEqual(['layers']);
    });

    it('drops a group emptied by stripping (strip then prune)', () => {
        const raw = persist(split(group(0, ['layers']), group(1, ['bogus'] as unknown as PanelType[])));
        const { root } = loadOrDefault(raw);
        expect(collectPanelTypes(root)).toEqual(['layers']);
        // The emptied group is gone, not a zero-tab phantom.
        expect(shape(root)).toEqual([['layers']]);
    });

    it('falls back to the default when stripping empties everything', () => {
        const { root } = loadOrDefault(persist(split(group(0, ['bogus'] as unknown as PanelType[]))));
        expect(collectPanelTypes(root).sort()).toEqual(['layers', 'properties']);
    });

    it('folds pop-out workspace trees back into the main tree', () => {
        // Main has only layers; a pop-out workspace holds properties.
        const raw = persist(split(group(0, ['layers'])), split(group(1, ['properties'])));
        const { root, nextGroupId } = loadOrDefault(raw);
        expect(collectPanelTypes(root).sort()).toEqual(['layers', 'properties']);
        // Ids renumbered contiguously; nextGroupId past the max.
        expect(nextGroupId).toBeGreaterThan(0);
    });
});

describe('foldPanelsIntoMain', () => {
    it('appends orphans to the first group and dedups already-present panels', () => {
        const root = split(group(0, ['layers']));
        foldPanelsIntoMain(root, ['properties', 'layers']);
        expect(collectPanelTypes(root)).toEqual(['layers', 'properties']);
    });
});
