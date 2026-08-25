import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { app, DarklyInstance, setActiveInstance } from '../app.svelte';
import { BrushGraphState, type BrushGraph } from '../brush_graph.svelte';
import { recentBrushes, recentColors } from '../recents.svelte';

const emptyGraph: BrushGraph = { nodes: {}, connections: [] };

/** Engine stub covering `loadBrush`'s refresh chain. `brushLoad` either
 *  resolves or rejects, which is the branch under test. */
function fakeEngine(loadOk: boolean) {
    return {
        api: {
            brushLoad: async () => {
                if (!loadOk) throw new Error('no such brush');
                return null;
            },
            brushGraphActive: async () => emptyGraph,
            brushExposedPorts: async () => [],
            brushActiveCapabilities: async () => ({}),
            brushTopologyVersion: async () => ({ value: 0 }),
        },
    } as unknown as NonNullable<typeof app.engine>;
}

let inst: DarklyInstance;

beforeEach(() => {
    inst = new DarklyInstance();
    setActiveInstance(inst);
});
afterEach(() => {
    setActiveInstance(null);
});

describe('recording what was actually used', () => {
    it('consuming_the_foreground_records_it', () => {
        inst.foreground = { r: 0x33, g: 0x55, b: 0xff, a: 255 };

        const got = inst.consumeForeground();

        expect(got).toEqual({ r: 0x33, g: 0x55, b: 0xff, a: 255 });
        expect(recentColors.items[0]).toBe('#3355ffff');
    });

    it('consuming_the_same_color_twice_leaves_one_entry', () => {
        inst.foreground = { r: 1, g: 2, b: 3, a: 255 };
        inst.consumeForeground();
        inst.consumeForeground();
        inst.consumeForeground();

        expect(recentColors.items.filter(c => c === '#010203ff')).toHaveLength(1);
    });

    it('loading_a_brush_records_it', async () => {
        const state = new BrushGraphState();
        app.engine = fakeEngine(true);

        await state.loadBrush('Ink Pen', 'ink_pen');

        // The name is what the engine loads by and what the UI shows; the id
        // is what recents keeps, so a rename cannot drop the entry.
        expect(state.activeBrush).toBe('Ink Pen');
        expect(recentBrushes.items[0]).toBe('ink_pen');
    });

    it('a_failed_brush_load_records_nothing', async () => {
        const state = new BrushGraphState();
        app.engine = fakeEngine(true);
        await state.loadBrush('Ink Pen', 'ink_pen');

        app.engine = fakeEngine(false);
        await state.loadBrush('Nonexistent', 'nonexistent');

        // The failed load left the front alone — a brush that never loaded
        // was never used.
        expect(state.error).not.toBeNull();
        expect(recentBrushes.items).not.toContain('nonexistent');
        expect(recentBrushes.items[0]).toBe('ink_pen');
    });
});
