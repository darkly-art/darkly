import { describe, it, expect, beforeEach, vi } from 'vitest';
import { DarklyInstance } from '../app.svelte';
import type { Engine } from '../../engine/protocol';

// In the frontend tree, index 0 of any `children` array is the TOP of the
// stack, so a higher index is lower in the panel. "Sibling below" therefore
// means the next higher index. See docs/plans/layer-delete-reselection.md §3.1.
function layer(id: number, extra: Record<string, unknown> = {}) {
    return { type: 'raster', id, name: `l${id}`, visible: true, modifiers: [], ...extra };
}
function group(id: number, children: unknown[], extra: Record<string, unknown> = {}) {
    return {
        type: 'group', id, name: `g${id}`, visible: true, collapsed: false,
        children, modifiers: [], ...extra,
    };
}

/// A `DarklyInstance` whose engine serves `treeRef.current`, so a test can swap
/// the tree between refreshes the way a real mutation would.
function instanceServing(treeRef: { current: any[] }) {
    const inst = new DarklyInstance();
    inst.requestFrame = vi.fn();
    const api = {
        layerTree: () => Promise.resolve(treeRef.current),
        setIsolatedNode: vi.fn().mockResolvedValue(null),
        setGroupCollapsed: vi.fn(),
    };
    inst.engine = { api } as unknown as Engine;
    return { inst, api };
}

let treeRef: { current: any[] };
let inst: DarklyInstance;
let api: { setIsolatedNode: ReturnType<typeof vi.fn>; setGroupCollapsed: ReturnType<typeof vi.fn> };

beforeEach(() => {
    vi.stubGlobal('requestAnimationFrame', vi.fn());
    treeRef = { current: [] };
    ({ inst, api } = instanceServing(treeRef) as any);
});

/** Seed the pre-mutation index, then apply `mutate` and refresh again. */
async function refreshThen(mutate: () => void) {
    await inst.refreshLayerTree();
    mutate();
    await inst.refreshLayerTree();
}

describe('reselection after a removal', () => {
    // R1
    it('adopts the sibling below when a middle layer disappears', async () => {
        treeRef.current = [layer(3), layer(2), layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(2);

        treeRef.current = [layer(3), layer(1)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(1);
        expect([...inst.selectedLayerIds]).toEqual([1]);
    });

    // R2
    it('falls back to the sibling above when the bottom-most layer disappears', async () => {
        treeRef.current = [layer(3), layer(2), layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        treeRef.current = [layer(3), layer(2)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(2);
    });

    it('adopts the sibling below when the top layer disappears', async () => {
        treeRef.current = [layer(3), layer(2), layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(3);

        treeRef.current = [layer(2), layer(1)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(2);
    });

    // R3
    it('adopts the enclosing group when its only child disappears', async () => {
        treeRef.current = [group(100, [layer(1)]), layer(2)];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        treeRef.current = [group(100, []), layer(2)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(100);
        expect([...inst.selectedLayerIds]).toEqual([100]);
    });

    it('keeps the selection inside the group when a sibling remains', async () => {
        treeRef.current = [group(100, [layer(1), layer(2)]), layer(3)];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        treeRef.current = [group(100, [layer(2)]), layer(3)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(2);
    });

    it('escalates to the group’s own level when the group dies too', async () => {
        treeRef.current = [group(100, [layer(1)]), layer(2)];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        treeRef.current = [layer(2)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(2);
    });

    // R5
    it('reselects after a batch delete removes the whole selection', async () => {
        treeRef.current = [layer(4), layer(3), layer(2), layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(4);
        inst.toggleLayer(3);
        inst.toggleLayer(2);

        treeRef.current = [layer(1)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(1);
        expect([...inst.selectedLayerIds]).toEqual([1]);
    });

    // R6
    it('adopts the host when its only modifier disappears', async () => {
        const withMask = layer(1, { modifiers: [{ type: 'mask', id: 900, name: 'Mask' }] });
        treeRef.current = [withMask, layer(2)];
        await inst.refreshLayerTree();
        inst.selectLayer(900);

        treeRef.current = [layer(1), layer(2)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(1);
    });

    it('adopts a sibling modifier before falling back to the host', async () => {
        const two = layer(1, {
            modifiers: [
                { type: 'mask', id: 900, name: 'A' },
                { type: 'mask', id: 901, name: 'B' },
            ],
        });
        treeRef.current = [two];
        await inst.refreshLayerTree();
        inst.selectLayer(900);

        treeRef.current = [layer(1, { modifiers: [{ type: 'mask', id: 901, name: 'B' }] })];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(901);
    });

    it('leaves the selection alone when an unselected layer disappears', async () => {
        treeRef.current = [layer(3), layer(2), layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        treeRef.current = [layer(3), layer(1)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(1);
    });

    it('demotes to a surviving selection member before using the neighbour', async () => {
        treeRef.current = [layer(3), layer(2), layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(3);
        inst.toggleLayer(1);
        expect(inst.activeLayerId).toBe(1);

        treeRef.current = [layer(3), layer(2)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(3);
        expect([...inst.selectedLayerIds]).toEqual([3]);
    });

    it('gives up when the whole tree is replaced', async () => {
        treeRef.current = [layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        treeRef.current = [layer(77)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBeNull();
        expect(inst.selectedLayerIds.size).toBe(0);
    });
});

describe('collapsed groups', () => {
    // R4
    it('keeps a selection inside a group that gets collapsed', async () => {
        treeRef.current = [group(100, [layer(1), layer(2)])];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        treeRef.current = [group(100, [layer(1), layer(2)], { collapsed: true })];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(1);
        expect([...inst.selectedLayerIds]).toEqual([1]);
    });

    // R10
    it('expands the enclosing group when the fallback lands on a hidden row', async () => {
        treeRef.current = [group(100, [layer(1), layer(2)], { collapsed: true })];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        treeRef.current = [group(100, [layer(2)], { collapsed: true })];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(2);
        expect(api.setGroupCollapsed).toHaveBeenCalledWith({ id: 100, collapsed: false });
    });

    it('does not expand anything when the new active row is already visible', async () => {
        treeRef.current = [layer(2), layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(2);

        treeRef.current = [layer(1)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(1);
        expect(api.setGroupCollapsed).not.toHaveBeenCalled();
    });
});

describe('isolation target', () => {
    // R9: the isolated node dies while a different, still-live layer is
    // active, so the selection branch never runs and only the dedicated check
    // can clear the stale target.
    it('clears the isolation target when the isolated node leaves the tree', async () => {
        treeRef.current = [layer(2), layer(1)];
        await inst.refreshLayerTree();
        inst.activeLayerId = 1;
        inst.selectedLayerIds = new Set([1]);
        inst.isolatedNodeId = 2;

        treeRef.current = [layer(1)];
        await inst.refreshLayerTree();

        expect(inst.isolatedNodeId).toBeNull();
        expect(api.setIsolatedNode).toHaveBeenCalledWith({ id: null });
        expect(inst.activeLayerId).toBe(1);
    });

    it('keeps the isolation target when it survives the refresh', async () => {
        treeRef.current = [layer(2), layer(1)];
        await inst.refreshLayerTree();
        inst.isolatedNodeId = 2;

        treeRef.current = [layer(2)];
        await inst.refreshLayerTree();

        expect(inst.isolatedNodeId).toBe(2);
    });
});

describe('adopting restored rows (undo of a delete)', () => {
    // A1
    it('selects the layer an undo brought back', async () => {
        treeRef.current = [layer(3), layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        treeRef.current = [layer(3), layer(2), layer(1)];
        await inst.refreshLayerTree({ adoptAppeared: true });

        expect(inst.activeLayerId).toBe(2);
        expect([...inst.selectedLayerIds]).toEqual([2]);
    });

    // A2
    it('restores the whole set an undone batch delete brought back', async () => {
        treeRef.current = [layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        treeRef.current = [layer(3), layer(2), layer(1)];
        await inst.refreshLayerTree({ adoptAppeared: true });

        expect(inst.activeLayerId).toBe(3);
        expect([...inst.selectedLayerIds].sort()).toEqual([2, 3]);
    });

    // A5: only the topmost restored row, not its descendants
    it('selects only the group when an undone delete restores a whole subtree', async () => {
        treeRef.current = [layer(9)];
        await inst.refreshLayerTree();
        inst.selectLayer(9);

        treeRef.current = [
            group(100, [
                layer(1, { modifiers: [{ type: 'mask', id: 900, name: 'Mask' }] }),
                layer(2),
            ]),
            layer(9),
        ];
        await inst.refreshLayerTree({ adoptAppeared: true });

        expect(inst.activeLayerId).toBe(100);
        expect([...inst.selectedLayerIds]).toEqual([100]);
    });

    it('keeps a restored modifier on a surviving host', async () => {
        treeRef.current = [layer(1), layer(2)];
        await inst.refreshLayerTree();
        inst.selectLayer(2);

        treeRef.current = [layer(1, { modifiers: [{ type: 'mask', id: 900, name: 'Mask' }] }), layer(2)];
        await inst.refreshLayerTree({ adoptAppeared: true });

        expect(inst.activeLayerId).toBe(900);
    });

    // A3: undo of an *add* removes a row, so the neighbour fallback applies
    it('falls back to the neighbour when an undo removes rows instead', async () => {
        treeRef.current = [layer(3), layer(2), layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(2);

        treeRef.current = [layer(3), layer(1)];
        await inst.refreshLayerTree({ adoptAppeared: true });

        expect(inst.activeLayerId).toBe(1);
    });

    // A4, nothing changed, nothing to adopt
    it('leaves the selection alone when the tree is unchanged', async () => {
        treeRef.current = [layer(2), layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        await inst.refreshLayerTree({ adoptAppeared: true });

        expect(inst.activeLayerId).toBe(1);
    });

    // R15: an ordinary refresh must never seize the selection
    it('does not adopt appeared rows on an unflagged refresh', async () => {
        treeRef.current = [layer(1)];
        await inst.refreshLayerTree();
        inst.selectLayer(1);

        treeRef.current = [layer(2), layer(1)];
        await inst.refreshLayerTree();

        expect(inst.activeLayerId).toBe(1);
        expect([...inst.selectedLayerIds]).toEqual([1]);
    });
});
