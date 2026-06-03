import { describe, it, expect, beforeEach } from 'vitest';
import { DarklyInstance } from '../app.svelte';

// Build a 3-level tree the selection methods will walk:
//   root
//     groupA (collapsed=false)
//       l1
//       l2
//     l3
//     groupB (collapsed=true)
//       l4    ← hidden from flattenedVisibleIds
function buildTree() {
    return [
        {
            type: 'group', id: 100, name: 'A', visible: true, collapsed: false,
            children: [
                { type: 'raster', id: 1, name: 'l1', visible: true, modifiers: [] },
                { type: 'raster', id: 2, name: 'l2', visible: true, modifiers: [] },
            ],
            modifiers: [],
        },
        { type: 'raster', id: 3, name: 'l3', visible: true, modifiers: [] },
        {
            type: 'group', id: 200, name: 'B', visible: true, collapsed: true,
            children: [
                { type: 'raster', id: 4, name: 'l4', visible: true, modifiers: [] },
            ],
            modifiers: [],
        },
    ];
}

let inst: DarklyInstance;
beforeEach(() => {
    inst = new DarklyInstance();
    inst.layerTree = buildTree();
});

describe('plain selectLayer', () => {
    it('replaces the selection set with one id', () => {
        inst.selectLayer(1);
        expect(inst.activeLayerId).toBe(1);
        expect([...inst.selectedLayerIds]).toEqual([1]);

        inst.selectLayer(3);
        expect(inst.activeLayerId).toBe(3);
        expect([...inst.selectedLayerIds]).toEqual([3]);
    });

    it('clears selection when id is null', () => {
        inst.selectLayer(1);
        inst.selectLayer(null);
        expect(inst.activeLayerId).toBeNull();
        expect(inst.selectedLayerIds.size).toBe(0);
    });
});

describe('ctrl-click (toggleLayer)', () => {
    it('adds an id and makes it active', () => {
        inst.selectLayer(1);
        inst.toggleLayer(3);
        expect(inst.activeLayerId).toBe(3);
        expect([...inst.selectedLayerIds].sort()).toEqual([1, 3]);
    });

    it('removes an id; non-active removal keeps the active id', () => {
        inst.selectLayer(1);
        inst.toggleLayer(3);
        // now selected={1,3}, active=3
        inst.toggleLayer(1);
        // removed non-active id (1)
        expect(inst.activeLayerId).toBe(3);
        expect([...inst.selectedLayerIds]).toEqual([3]);
    });

    it('removing the active id demotes to the next selected in tree order', () => {
        inst.selectLayer(1);
        inst.toggleLayer(2);
        inst.toggleLayer(3);
        // selected={1,2,3}, active=3
        inst.toggleLayer(3);
        // active was 3; should demote to one of {1,2}. Tree order visits 1
        // before 2, so the replacement is 1.
        expect([...inst.selectedLayerIds].sort()).toEqual([1, 2]);
        expect(inst.activeLayerId).toBe(1);
    });

    it('removing the only selected id clears active to null', () => {
        inst.selectLayer(1);
        inst.toggleLayer(1);
        expect(inst.activeLayerId).toBeNull();
        expect(inst.selectedLayerIds.size).toBe(0);
    });

    it('adds to an empty set as a plain select', () => {
        inst.toggleLayer(2);
        expect(inst.activeLayerId).toBe(2);
        expect([...inst.selectedLayerIds]).toEqual([2]);
    });
});

describe('shift-click (extendSelectionTo)', () => {
    it('degenerates to plain select when there is no anchor', () => {
        // No active layer → no anchor → behaves like selectLayer
        inst.extendSelectionTo(2);
        expect(inst.activeLayerId).toBe(2);
        expect([...inst.selectedLayerIds]).toEqual([2]);
    });

    it('selects the inclusive range from anchor to target in tree order', () => {
        inst.selectLayer(1);
        inst.extendSelectionTo(3);
        // Tree-visible order: [100, 1, 2, 3, 200] (collapsed B hides l4).
        // anchor=1 (idx 1), target=3 (idx 3) → range = [1, 2, 3].
        expect([...inst.selectedLayerIds].sort((a, b) => a - b)).toEqual([1, 2, 3]);
        expect(inst.activeLayerId).toBe(3);
    });

    it('handles the reverse direction (target above anchor)', () => {
        inst.selectLayer(3);
        inst.extendSelectionTo(1);
        // Range from 3 down to 1 in tree order — same set, just clicked backwards.
        expect([...inst.selectedLayerIds].sort((a, b) => a - b)).toEqual([1, 2, 3]);
        expect(inst.activeLayerId).toBe(1);
    });

    it('skips children of collapsed groups', () => {
        inst.selectLayer(1);
        // Try extending to l4, which lives inside the collapsed group B.
        // l4 is not in flattenedVisibleIds, so the range falls back to
        // selectLayer(4). The selection becomes just {4}.
        inst.extendSelectionTo(4);
        expect([...inst.selectedLayerIds]).toEqual([4]);
        expect(inst.activeLayerId).toBe(4);
    });
});

describe('isSelected', () => {
    it('returns membership in the selection set', () => {
        inst.selectLayer(1);
        inst.toggleLayer(3);
        expect(inst.isSelected(1)).toBe(true);
        expect(inst.isSelected(3)).toBe(true);
        expect(inst.isSelected(2)).toBe(false);
    });
});

describe('handleLayerRowClick router', () => {
    it('plain click → selectLayer', () => {
        const e = { shiftKey: false, ctrlKey: false, metaKey: false } as MouseEvent;
        inst.handleLayerRowClick(2, e);
        expect([...inst.selectedLayerIds]).toEqual([2]);
        expect(inst.activeLayerId).toBe(2);
    });

    it('ctrl-click → toggleLayer', () => {
        inst.selectLayer(1);
        const e = { shiftKey: false, ctrlKey: true, metaKey: false } as MouseEvent;
        inst.handleLayerRowClick(3, e);
        expect([...inst.selectedLayerIds].sort()).toEqual([1, 3]);
        expect(inst.activeLayerId).toBe(3);
    });

    it('shift-click → extendSelectionTo', () => {
        inst.selectLayer(1);
        const e = { shiftKey: true, ctrlKey: false, metaKey: false } as MouseEvent;
        inst.handleLayerRowClick(3, e);
        expect([...inst.selectedLayerIds].sort((a, b) => a - b)).toEqual([1, 2, 3]);
    });
});

describe('selectLayers (replace with N)', () => {
    it('sets the selection to the given ids and makes the last one active', () => {
        inst.selectLayers([1, 2, 3]);
        expect([...inst.selectedLayerIds].sort((a, b) => a - b)).toEqual([1, 2, 3]);
        expect(inst.activeLayerId).toBe(3);
    });

    it('empty input clears selection', () => {
        inst.selectLayer(1);
        inst.selectLayers([]);
        expect(inst.selectedLayerIds.size).toBe(0);
        expect(inst.activeLayerId).toBeNull();
    });
});
