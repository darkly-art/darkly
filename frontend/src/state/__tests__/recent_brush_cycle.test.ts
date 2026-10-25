import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// The color lock is a config pref read through WASM once it is ready. Stand in
// for the store so a case can flip it; everything else stays real.
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
import { BrushGraphState, brushGraph } from '../brush_graph.svelte';
import { brushLibrary } from '../brush_library.svelte';
import { brushColors } from '../brushColors.svelte';
import { recentBrushes } from '../recents.svelte';
import { beginPaintStroke } from '../../tools/paint_stroke';
import type { Color } from '../../lib/color';
import { fakeBrushEngine, type FakeBrushEngine } from './fakeBrushEngine';

const PAUSE_MS = 700;
const RED: Color = { r: 255, g: 0, b: 0, a: 255 };
const BLUE: Color = { r: 0, g: 0, b: 255, a: 255 };
const WHITE: Color = { r: 255, g: 255, b: 255, a: 255 };

let fake: FakeBrushEngine;
let state: BrushGraphState;
let inst: DarklyInstance;

/** Let every queued engine answer land without running the chain's pause. */
const landed = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => {
    vi.useFakeTimers();
    inst = new DarklyInstance();
    setActiveInstance(inst);
    fake = fakeBrushEngine({ beginStroke: async () => null });
    app.engine = fake.engine;
    fakeConfig.locked = false;
    brushColors.clear();
    brushLibrary.brushes = ['A', 'B', 'C', 'D'].map(name => ({
        id: name.toLowerCase(),
        name,
        author: '',
        description: '',
        tags: [],
        icon: null,
        can_edit: true,
    }));
    recentBrushes.retain(() => false);
    state = new BrushGraphState();
});
afterEach(() => {
    vi.useRealTimers();
    setActiveInstance(null);
});

/** Use `ids` oldest first, so the ring reads newest first, then load the
 *  newest so it is the active brush. */
async function seed(target: BrushGraphState, ...ids: string[]) {
    for (const id of ids.slice(0, -1)) recentBrushes.use(id);
    const last = ids[ids.length - 1];
    await target.loadBrush(last.toUpperCase(), last);
    fake.loads.length = 0;
}

describe('brush switching', () => {
    // Regression: `loadBrush` did not serialize itself. Picks that landed
    // while an earlier one was still in flight each reached the engine and
    // each claimed a recents slot, though the painter only ever saw the last.
    it('a_burst_of_loadBrush_calls_coalesces_and_records_only_the_last', async () => {
        await state.loadBrush('A', 'a');
        const release = fake.hold();
        const picks = [state.loadBrush('B', 'b'), state.loadBrush('C', 'c'), state.loadBrush('D', 'd')];
        release();
        await Promise.all(picks);

        expect(fake.loads).toEqual(['A', 'B', 'D']);
        expect(state.activeBrushId).toBe('d');
        expect(recentBrushes.items).toEqual(['d', 'a']);
    });

    it('a_burst_records_each_outgoing_pair_under_the_brush_that_was_active', async () => {
        fakeConfig.locked = true;
        brushColors.record('d', { foreground: BLUE, background: WHITE });
        await state.loadBrush('A', 'a');
        inst.foreground = { ...RED };

        const release = fake.hold();
        const picks = [state.loadBrush('B', 'b'), state.loadBrush('C', 'c'), state.loadBrush('D', 'd')];
        release();
        await Promise.all(picks);

        const probe = { foreground: { ...WHITE }, background: { ...WHITE } };
        expect(brushColors.restore('a', probe)).toBe(true);
        expect(probe.foreground).toEqual(RED);
        expect(inst.foreground).toEqual(BLUE);
        expect(brushColors.restore('c', probe)).toBe(false);
    });
});

describe('recent brush cycle', () => {
    it('cycling_loads_through_the_engine_without_recording_until_the_pause', async () => {
        await seed(state, 'c', 'b', 'a');
        expect(state.cycleRecentBrush()).toBe('b');
        await landed();

        expect(fake.loads).toEqual(['B']);
        expect(state.activeBrushId).toBe('b');
        expect(recentBrushes.items).toEqual(['a', 'b', 'c']);

        vi.advanceTimersByTime(PAUSE_MS);
        expect(recentBrushes.items).toEqual(['b', 'a', 'c']);
    });

    it('a_pause_then_a_press_swaps_back', async () => {
        await seed(state, 'c', 'b', 'a');
        state.cycleRecentBrush();
        await landed();
        vi.advanceTimersByTime(PAUSE_MS);

        expect(state.cycleRecentBrush()).toBe('a');
        await landed();
        expect(state.activeBrushId).toBe('a');
    });

    it('a_load_from_elsewhere_settles_the_chain_first', async () => {
        await seed(state, 'c', 'b', 'a');
        state.cycleRecentBrush();
        state.cycleRecentBrush();
        await landed();
        expect(state.activeBrushId).toBe('c');

        await state.loadBrush('D', 'd');
        expect(recentBrushes.items.slice(0, 2)).toEqual(['d', 'c']);
    });

    it('a_burst_of_taps_coalesces_engine_loads', async () => {
        await seed(state, 'd', 'c', 'b', 'a');
        const release = fake.hold();
        expect([state.cycleRecentBrush(), state.cycleRecentBrush(), state.cycleRecentBrush()]).toEqual([
            'b',
            'c',
            'd',
        ]);
        release();
        await landed();

        expect(fake.loads).toEqual(['B', 'D']);
        expect(state.activeBrushId).toBe('d');
    });

    it('starting_a_stroke_settles_the_chain', async () => {
        // `beginPaintStroke` settles the app's one brush state, so this case
        // drives the singleton.
        await seed(brushGraph, 'c', 'b', 'a');
        brushGraph.cycleRecentBrush();
        await landed();

        beginPaintStroke(fake.engine, 1);
        expect(recentBrushes.items).toEqual(['b', 'a', 'c']);
        brushGraph.settleRecentCycle();
    });
});
