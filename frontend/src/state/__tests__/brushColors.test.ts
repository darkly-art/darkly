import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// The lock is a config pref, and the config store's reads go through WASM
// once it is ready. Stand in for the store so the test can flip the lock;
// everything else the module exports stays real.
const { fakeConfig } = vi.hoisted(() => {
    const fakeConfig = {
        locked: false,
        get: (key: string) => (key === 'colors.lockToBrush' ? fakeConfig.locked : undefined),
    };
    return { fakeConfig };
});
vi.mock('../../config/store.svelte', async (importOriginal) => ({
    ...(await importOriginal<object>()),
    config: fakeConfig,
}));

import { app, DarklyInstance, setActiveInstance } from '../app.svelte';
import { BrushGraphState, type BrushGraph } from '../brush_graph.svelte';
import { brushColors, type ColorPair } from '../brushColors.svelte';
import type { Color } from '../app.svelte';

const emptyGraph: BrushGraph = { nodes: {}, connections: [] };

const RED: Color = { r: 255, g: 0, b: 0, a: 255 };
const GREEN: Color = { r: 0, g: 255, b: 0, a: 255 };
const BLUE: Color = { r: 0, g: 0, b: 255, a: 255 };
const WHITE: Color = { r: 255, g: 255, b: 255, a: 255 };

/** Engine stub covering `loadBrush`'s refresh chain and `resetToDefault`.
 *  `brushLoad` either resolves or rejects, which is the branch under test. */
function fakeEngine(loadOk: boolean) {
    return {
        api: {
            brushLoad: async () => {
                if (!loadOk) throw new Error('no such brush');
                return null;
            },
            brushGraphReset: () => {},
            brushGraphActive: async () => emptyGraph,
            brushExposedPorts: async () => [],
            brushActiveCapabilities: async () => ({}),
            brushTopologyVersion: async () => ({ value: 0 }),
        },
    } as unknown as NonNullable<typeof app.engine>;
}

function pair(foreground: Color, background: Color): ColorPair {
    return { foreground: { ...foreground }, background: { ...background } };
}

function paint(inst: DarklyInstance, foreground: Color, background: Color) {
    inst.foreground = { ...foreground };
    inst.background = { ...background };
}

beforeEach(() => {
    brushColors.clear();
    fakeConfig.locked = false;
});

describe('brush color memory', () => {
    it('record_then_restore_sets_both_colors', () => {
        brushColors.record('ink', pair(RED, GREEN));

        const target = pair(WHITE, WHITE);
        expect(brushColors.restore('ink', target)).toBe(true);
        expect(target.foreground).toEqual(RED);
        expect(target.background).toEqual(GREEN);
    });

    it('a_never_recorded_brush_leaves_the_pair_untouched', () => {
        const target = pair(WHITE, BLUE);
        expect(brushColors.restore('pencil', target)).toBe(false);
        expect(target.foreground).toEqual(WHITE);
        expect(target.background).toEqual(BLUE);
    });

    it('recording_stores_a_copy', () => {
        const source = pair(RED, GREEN);
        brushColors.record('ink', source);
        source.foreground.r = 0;
        source.background = { ...BLUE };

        const target = pair(WHITE, WHITE);
        brushColors.restore('ink', target);
        expect(target.foreground).toEqual(RED);
        expect(target.background).toEqual(GREEN);
    });

    it('a_null_id_records_nothing', () => {
        brushColors.record(null, pair(RED, GREEN));
        const target = pair(WHITE, WHITE);
        expect(brushColors.restore(null, target)).toBe(false);
        expect(target.foreground).toEqual(WHITE);
    });
});

describe('paint stays on the brush across loadBrush', () => {
    let inst: DarklyInstance;
    let state: BrushGraphState;

    beforeEach(() => {
        inst = new DarklyInstance();
        setActiveInstance(inst);
        app.engine = fakeEngine(true);
        state = new BrushGraphState();
    });
    afterEach(() => {
        setActiveInstance(null);
    });

    it('switching_away_records_the_outgoing_pair', async () => {
        await state.loadBrush('A', 'a');
        paint(inst, RED, GREEN);

        await state.loadBrush('B', 'b');

        const probe = pair(WHITE, WHITE);
        expect(brushColors.restore('a', probe)).toBe(true);
        expect(probe).toEqual(pair(RED, GREEN));
    });

    it('locked_switching_back_restores_the_pair', async () => {
        fakeConfig.locked = true;
        await state.loadBrush('A', 'a');
        paint(inst, RED, GREEN);
        await state.loadBrush('B', 'b');
        paint(inst, BLUE, WHITE);

        await state.loadBrush('A', 'a');

        expect(inst.foreground).toEqual(RED);
        expect(inst.background).toEqual(GREEN);
    });

    it('unlocked_switching_back_restores_nothing', async () => {
        await state.loadBrush('A', 'a');
        paint(inst, RED, GREEN);
        await state.loadBrush('B', 'b');
        paint(inst, BLUE, WHITE);

        await state.loadBrush('A', 'a');

        expect(inst.foreground).toEqual(BLUE);
        expect(inst.background).toEqual(WHITE);
    });

    it('a_failed_load_restores_nothing', async () => {
        fakeConfig.locked = true;
        await state.loadBrush('A', 'a');
        paint(inst, RED, GREEN);
        await state.loadBrush('B', 'b');
        paint(inst, BLUE, WHITE);

        app.engine = fakeEngine(false);
        await state.loadBrush('A', 'a');

        expect(state.error).not.toBeNull();
        expect(inst.foreground).toEqual(BLUE);
        expect(inst.background).toEqual(WHITE);
    });

    it('a_saved_brush_records_under_its_new_id', async () => {
        await state.loadBrush('A', 'a');
        paint(inst, RED, GREEN);
        // Save As: the graph is now a different library brush.
        state.setActiveBrush({ name: 'A copy', id: 'a_copy' });
        paint(inst, BLUE, WHITE);

        await state.loadBrush('B', 'b');

        expect(state.activeBrush).toBe('B');
        const probe = pair(WHITE, WHITE);
        expect(brushColors.restore('a_copy', probe)).toBe(true);
        expect(probe).toEqual(pair(BLUE, WHITE));
        // The original was never switched away from, so it has no paint.
        expect(brushColors.restore('a', pair(WHITE, WHITE))).toBe(false);
    });

    it('load_sets_the_active_id_and_reset_clears_it', async () => {
        await state.loadBrush('A', 'a');
        expect(state.activeBrush).toBe('A');
        expect(state.activeBrushId).toBe('a');

        await state.resetToDefault();
        expect(state.activeBrush).toBeNull();
        expect(state.activeBrushId).toBeNull();
    });
});
