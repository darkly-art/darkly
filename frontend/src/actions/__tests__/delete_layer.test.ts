import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import { registerActions } from '../index';
import { actions } from '../registry';
import { DarklyInstance, setActiveInstance } from '../../state/app.svelte';
import type { Engine } from '../../engine/protocol';

// End-to-end cover for the reported symptom: deleting a layer must leave an
// appropriate row selected rather than nothing. The reconciler unit tests in
// `state/__tests__/reselection.test.ts` pin the fallback rule itself; this pins
// that the delete action lets it run.
beforeAll(() => {
    registerActions();
});

/** `Registry.dispatch` fires the handler without returning its promise, so
 *  reach for the registration to await the async work. */
function deleteLayer() {
    return actions.get('deleteLayer')!.handler({});
}

function layer(id: number) {
    return { type: 'raster', id, name: `l${id}`, visible: true, modifiers: [] };
}

let inst: DarklyInstance;
let tree: any[];
let removeLayer: ReturnType<typeof vi.fn>;
let removeLayers: ReturnType<typeof vi.fn>;

beforeEach(async () => {
    vi.stubGlobal('requestAnimationFrame', vi.fn());
    tree = [layer(3), layer(2), layer(1)];
    removeLayer = vi.fn().mockResolvedValue(undefined);
    removeLayers = vi.fn().mockResolvedValue(0);
    inst = new DarklyInstance();
    inst.requestFrame = vi.fn();
    inst.engine = {
        api: {
            layerTree: () => Promise.resolve({ layers: tree, screenSpaceCount: 0 }),
            removeLayer,
            removeLayers,
            setIsolatedNode: vi.fn().mockResolvedValue(null),
            setGroupCollapsed: vi.fn(),
        },
    } as unknown as Engine;
    setActiveInstance(inst);
    // Seed the pre-delete tree shape the fallback is computed against.
    await inst.refreshLayerTree();
});

describe('deleteLayer action', () => {
    it('selects the row below instead of clearing the selection', async () => {
        inst.selectLayer(2);
        removeLayer.mockImplementation(async () => {
            tree = tree.filter((n) => n.id !== 2);
        });

        await deleteLayer();

        expect(removeLayer).toHaveBeenCalledWith({ id: 2 });
        expect(inst.activeLayerId).toBe(1);
        expect([...inst.selectedLayerIds]).toEqual([1]);
    });

    it('selects the row above when the bottom layer is deleted', async () => {
        inst.selectLayer(1);
        removeLayer.mockImplementation(async () => {
            tree = tree.filter((n) => n.id !== 1);
        });

        await deleteLayer();

        expect(inst.activeLayerId).toBe(2);
    });

    it('reselects after a multi-layer delete', async () => {
        inst.selectLayer(3);
        inst.toggleLayer(2);
        removeLayers.mockImplementation(async () => {
            tree = tree.filter((n) => n.id !== 3 && n.id !== 2);
            return 0;
        });

        await deleteLayer();

        expect(removeLayers).toHaveBeenCalledWith({ ids: [3, 2] });
        expect(inst.activeLayerId).toBe(1);
        expect([...inst.selectedLayerIds]).toEqual([1]);
    });

    it('leaves the selection alone when the engine refuses the delete', async () => {
        inst.selectLayer(2);
        removeLayer.mockRejectedValue(new Error('Layer is locked'));

        await deleteLayer();

        expect(inst.activeLayerId).toBe(2);
    });
});
