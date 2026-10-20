import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { app, DarklyInstance, setActiveInstance } from '../app.svelte';
import { BrushGraphState, type BrushGraph } from '../brush_graph.svelte';
import { fakeBrushEngine, type FakeBrushEngine } from './fakeBrushEngine';

// The loaded brush, and whether it has unsaved tweaks, is the engine's fact.
// `BrushGraphState` only caches it, so after every graph operation its label
// must say what the engine says.

const graph: BrushGraph = { nodes: { a: {} as never }, connections: [] };

let fake: FakeBrushEngine;
let state: BrushGraphState;

beforeEach(() => {
    setActiveInstance(new DarklyInstance());
    fake = fakeBrushEngine({
        brushSetExposedPort: async () => {
            // A scrub on a loaded brush leaves it loaded and tweaked.
            if (fake.loaded) fake.loaded.modified = true;
            return { graph };
        },
        brushRevert: async () => {
            if (fake.loaded) fake.loaded.modified = false;
            return null;
        },
        brushGraphImportYaml: async () => {
            fake.loaded = null;
            return null;
        },
    });
    app.engine = fake.engine;
    state = new BrushGraphState();
});
afterEach(() => {
    setActiveInstance(null);
});

describe('loaded brush label', () => {
    it('a_load_mirrors_the_engine', async () => {
        await state.loadBrush('Airbrush', 'airbrush');
        expect(state.activeBrush).toBe('Airbrush');
        expect(state.activeBrushId).toBe('airbrush');
        expect(state.modified).toBe(false);
    });

    it('a_tweak_keeps_the_name_and_marks_it_modified', async () => {
        await state.loadBrush('Airbrush', 'airbrush');
        await state.setExposedPortValue('brush_settings', 'size', 250);
        expect(state.activeBrush).toBe('Airbrush');
        expect(state.modified).toBe(true);
    });

    it('revert_clears_modified', async () => {
        await state.loadBrush('Airbrush', 'airbrush');
        await state.setExposedPortValue('brush_settings', 'size', 250);
        await state.revert();
        expect(state.activeBrush).toBe('Airbrush');
        expect(state.modified).toBe(false);
    });

    it('reset_and_import_leave_no_brush_loaded', async () => {
        await state.loadBrush('Airbrush', 'airbrush');
        await state.resetToDefault();
        expect(state.activeBrush).toBeNull();
        expect(state.modified).toBe(false);

        await state.loadBrush('Airbrush', 'airbrush');
        expect(await state.importYaml('nodes: {}')).toBeNull();
        expect(state.activeBrushId).toBeNull();
    });
});
