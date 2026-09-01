import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { app, DarklyInstance, setActiveInstance } from '../app.svelte';
import { BrushGraphState, type BrushGraph } from '../brush_graph.svelte';
import { brushLibrary } from '../brush_library.svelte';
import { recentBrushes } from '../recents.svelte';

const emptyGraph: BrushGraph = { nodes: {}, connections: [] };

/** Ids are YAML file stems, names are display strings, and the two differ.
 *  That difference is the whole subject of this file: recents is id-keyed
 *  (`recents.svelte.ts`) and the pruner in `BrushLibraryStore.refresh` retains
 *  against live ids, so anything recorded under a display name is dropped by
 *  the next refresh, which `hydrate` runs at boot. */
function fakeEngine() {
    return {
        api: {
            libraryList: async () => ({
                brushes: [
                    { id: 'ink_pen', name: 'Ink Pen', author: '', description: '', tags: [], icon: null },
                ],
                packs: [],
            }),
            brushNodeTypes: async () => [],
            brushLoad: async () => null,
            brushGraphActive: async () => emptyGraph,
            brushExposedPorts: async () => [],
            brushActiveCapabilities: async () => ({}),
            brushTopologyVersion: async () => ({ value: 0 }),
        },
    } as unknown as NonNullable<typeof app.engine>;
}

beforeEach(async () => {
    setActiveInstance(new DarklyInstance());
    app.engine = fakeEngine();
    // The pruner and the writer both live on module singletons, so the
    // singletons are what this drives. Clear the ring between cases.
    recentBrushes.retain(() => false);
    await brushLibrary.refresh();
});

afterEach(() => {
    setActiveInstance(null);
});

describe('recents identity', () => {
    it('a_loaded_brush_survives_a_library_refresh', async () => {
        const state = new BrushGraphState();

        await state.loadBrush('Ink Pen', 'ink_pen');
        // What `hydrate` does at boot, and what any library mutation does in
        // between.
        await brushLibrary.refresh();

        expect(recentBrushes.items).toEqual(['ink_pen']);
    });

    it('a_failed_load_records_nothing', async () => {
        const state = new BrushGraphState();
        const engine = fakeEngine();
        engine.api.brushLoad = async () => {
            throw new Error('no such brush');
        };
        app.engine = engine;

        await state.loadBrush('Nonexistent', 'nonexistent');

        expect(state.error).not.toBeNull();
        expect(recentBrushes.items).toEqual([]);
    });

    it('the_boot_selection_is_not_a_recent', async () => {
        // `init` picks a brush so `activeBrush` renders something; the painter
        // did not reach for it, so it must not take the top recents slot from
        // whatever they last used.
        const state = new BrushGraphState();
        await state.init();

        expect(state.activeBrush).toBe('Ink Pen');
        expect(recentBrushes.items).toEqual([]);
    });
});
