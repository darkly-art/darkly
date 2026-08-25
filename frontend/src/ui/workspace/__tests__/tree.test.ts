import { describe, it, expect } from 'vitest';
import {
    type Subdivision,
    type SplitChild,
    makeGroup,
    findGroup,
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
    ensureDocument,
    resolveSplitByPath,
    loadOrDefault,
    firstDockableGroupId,
    type PanelType,
} from '../tree';

/** What `defaultMainLayout` ships, sorted. One constant so adding a default
 *  panel is one edit here rather than five. */
const DEFAULT_PANELS: PanelType[] = ['brushes', 'document', 'layers', 'properties'];

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
    it('split then move-back returns to the default panel set', () => {
        const layout = defaultMainLayout(0, 1, 2);
        const root = layout.root;
        // Split a scratch panel off to the right of the Layers group.
        const layersId = findGroup(root, 1)!.id;
        const newId = splitPanelGroup(root, layersId, 'right', ['history'] as unknown as PanelType[], 0, 99)!;
        prune(root);
        expect(collectPanelTypes(root)).toContain('history');
        // Remove it again.
        removeTab(root, newId, 'history' as unknown as PanelType);
        prune(root);
        expect(collectPanelTypes(root).sort()).toEqual(DEFAULT_PANELS);
    });
});

// --- loadOrDefault ---------------------------------------------------------

describe('loadOrDefault', () => {
    function persist(...roots: Subdivision[]): string {
        return JSON.stringify({ workspaces: roots.map((root, i) => ({ id: i, layout: { root } })) });
    }

    it('falls back to the default on null', () => {
        const { root } = loadOrDefault(null);
        expect(collectPanelTypes(root).sort()).toEqual(DEFAULT_PANELS);
    });

    it('falls back to the default on malformed JSON', () => {
        const { root } = loadOrDefault('{not json');
        expect(collectPanelTypes(root).sort()).toEqual(DEFAULT_PANELS);
    });

    it('strips unknown panel types (and injects the missing Document)', () => {
        const { root } = loadOrDefault(persist(split(group(0, ['layers', 'bogus'] as unknown as PanelType[]))));
        expect(collectPanelTypes(root).sort()).toEqual(['document', 'layers']);
    });

    it('drops a group emptied by stripping (strip then prune)', () => {
        const raw = persist(split(group(0, ['layers']), group(1, ['bogus'] as unknown as PanelType[])));
        const { root } = loadOrDefault(raw);
        // The emptied group is gone; Document is prepended to the surviving row.
        expect(shape(root)).toEqual([['document'], ['layers']]);
    });

    it('falls back to the default when stripping empties everything', () => {
        const { root } = loadOrDefault(persist(split(group(0, ['bogus'] as unknown as PanelType[]))));
        expect(collectPanelTypes(root).sort()).toEqual(DEFAULT_PANELS);
    });

    it('preserves a persisted Document without duplicating it', () => {
        const raw = persist(split(group(0, ['document']), group(1, ['layers'])));
        const { root } = loadOrDefault(raw);
        expect(collectPanelTypes(root).filter((t) => t === 'document')).toHaveLength(1);
        expect(collectPanelTypes(root).sort()).toEqual(['document', 'layers']);
    });

    it('folds pop-out workspace trees back into the main tree', () => {
        // Main has layers; a pop-out workspace holds properties. Document is
        // injected into main.
        const raw = persist(split(group(0, ['layers'])), split(group(1, ['properties'])));
        const { root, nextGroupId } = loadOrDefault(raw);
        // Only what was persisted, plus the injected Document — a stored
        // layout is not topped up with the default panel set.
        expect(collectPanelTypes(root).sort()).toEqual(['document', 'layers', 'properties']);
        // Ids renumbered contiguously; nextGroupId past the max.
        expect(nextGroupId).toBeGreaterThan(0);
    });
});

describe('resolveSplitByPath', () => {
    it('resolves the root split and nested splits, disambiguating shared first-groups', () => {
        // Row[ Document, Column[ layers, properties ] ]. The inner column shares
        // its first-group (layers) with... nothing here, but the root and the
        // column are distinct splits reachable only by path.
        const inner = split(group(1, ['layers']), group(2, ['properties']));
        const root = split(group(0, ['document']), slot(inner, 1));
        expect(resolveSplitByPath(root, [])).toBe(root);
        expect(resolveSplitByPath(root, [1])).toBe(inner);
    });

    it('returns null when the path does not land on a split', () => {
        const root = split(group(0, ['document']), group(1, ['layers']));
        expect(resolveSplitByPath(root, [0])).toBeNull(); // a group, not a split
        expect(resolveSplitByPath(root, [5])).toBeNull(); // out of range
    });
});

describe('ensureDocument', () => {
    it('prepends the canvas when absent and renormalizes', () => {
        const root = split(group(0, ['layers']), group(1, ['properties']));
        ensureDocument(root, 9);
        expect(shape(root)).toEqual([['document'], ['layers'], ['properties']]);
        const total = (root as { children: SplitChild[] }).children.reduce((a, c) => a + c.size, 0);
        expect(total).toBeCloseTo(1, 6);
    });

    it('is a no-op when the canvas is already present', () => {
        const root = split(group(0, ['document']), group(1, ['layers']));
        ensureDocument(root, 9);
        expect(collectPanelTypes(root).filter((t) => t === 'document')).toHaveLength(1);
    });
});

describe('foldPanelsIntoMain', () => {
    it('appends orphans to the first group and dedups already-present panels', () => {
        const root = split(group(0, ['layers']));
        foldPanelsIntoMain(root, ['properties', 'layers']);
        expect(collectPanelTypes(root)).toEqual(['layers', 'properties']);
    });

    it('never folds into the anchor group', () => {
        const root = split(group(0, ['document']), group(1, ['layers']));
        foldPanelsIntoMain(root, ['properties']);
        expect(findGroup(root, 0)!.state.tabs).toEqual(['document']);
        expect(findGroup(root, 1)!.state.tabs).toEqual(['layers', 'properties']);
    });
});

describe('firstDockableGroupId', () => {
    it('skips the canvas group in the default layout', () => {
        // `firstGroupId` returns the canvas here. A tab docked into an anchor
        // group is unreachable: the group renders no tab bar and its body shows
        // only the active tab, so the canvas would simply be replaced.
        const { root } = loadOrDefault(null);
        const target = firstDockableGroupId(root)!;
        expect(findGroup(root, target)!.state.tabs).not.toContain('document');
    });

    it('returns null when every group is an anchor', () => {
        const root = split(group(0, ['document']));
        expect(firstDockableGroupId(root)).toBeNull();
    });

    it('a revealed panel lands beside Layers, not in the canvas', () => {
        const { root } = loadOrDefault(null);
        const target = firstDockableGroupId(root)!;
        insertTab(root, target, 'brushes');

        const canvas = collectPanelTypes(root).indexOf('document');
        expect(canvas).toBe(0);
        expect(findGroup(root, target)!.state.tabs).toContain('brushes');
        expect(findGroup(root, target)!.state.tabs).toContain('layers');
    });
});
