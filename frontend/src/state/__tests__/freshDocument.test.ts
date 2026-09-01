import { describe, it, expect, vi } from 'vitest';
import { RECIPES } from '../freshDocument';
import type { Engine } from '../../engine/protocol';
import type { DarklyInstance } from '../app.svelte';

/** Minimal fake engine exposing just the `api` methods the recipes call. */
function fakeEngine() {
    let nextId = 1;
    const api = {
        fillBackground: vi.fn(),
        fillBackgroundColor: vi.fn(),
        resize: vi.fn(),
        addFilter: vi.fn((_req: { pipeline: string }) => Promise.resolve(nextId++)),
        setLayerVisible: vi.fn(),
        setScreenSpaceBoundary: vi.fn(),
    };
    return { engine: { api } as unknown as Engine, api };
}

/// A `DarklyInstance` stub carrying the two app-state hooks the seed calls.
function fakeInstance(engine: Engine) {
    const refreshLayerTree = vi.fn().mockResolvedValue(undefined);
    const requestFrame = vi.fn();
    const inst = { engine, refreshLayerTree, requestFrame } as unknown as DarklyInstance;
    return { inst, refreshLayerTree, requestFrame };
}

describe('freshDocument recipes', () => {
    describe('demo', () => {
        it('boots the watercolor brush with a black foreground on a white background', () => {
            expect(RECIPES.demo.defaultBrushName).toBe('Rough Watercolor');
            expect(RECIPES.demo.foreground).toEqual({ r: 0, g: 0, b: 0, a: 255 });
            expect(RECIPES.demo.background).toEqual({ r: 255, g: 255, b: 255, a: 255 });
        });

        it('fills the initial layer from the background image', () => {
            const { engine, api } = fakeEngine();
            RECIPES.demo.fillInitialLayer(engine, 7);
            expect(api.fillBackground).toHaveBeenCalledWith({ id: 7 });
            expect(api.fillBackgroundColor).not.toHaveBeenCalled();
        });

        it('seeds four effect layers and puts the divider above all of them', async () => {
            const { engine, api } = fakeEngine();
            const { inst } = fakeInstance(engine);
            await RECIPES.demo.seedViewportEffects(inst, 800, 600);
            expect(api.addFilter).toHaveBeenCalledTimes(4);
            expect(api.addFilter.mock.calls.map((c) => c[0].pipeline)).toEqual([
                'rainy_glass',
                'grain',
                'lens_blur',
                'vhs',
            ]);
            // One boundary call at the end, not one per layer: adding a layer
            // never crosses the divider on its own.
            expect(api.setScreenSpaceBoundary).toHaveBeenCalledTimes(1);
            expect(api.setScreenSpaceBoundary).toHaveBeenCalledWith({ count: 4 });
        });

        // Regression: the demo booted with all four effects applied, because
        // the seed added them at their default visibility. They exist to be
        // discovered, not to redecorate the canvas before the user has touched
        // anything.
        it('seeds every effect hidden', async () => {
            const { engine, api } = fakeEngine();
            const { inst } = fakeInstance(engine);
            await RECIPES.demo.seedViewportEffects(inst, 800, 600);
            expect(api.setLayerVisible).toHaveBeenCalledTimes(4);
            for (const call of api.setLayerVisible.mock.calls) {
                expect(call[0].visible).toBe(false);
            }
        });

        // Regression: the panel read the tree when it mounted, which is before
        // any of this exists, and nothing else refreshed it — so the seeded
        // effects were in the document but absent from the layer panel.
        it('refreshes the layer panel once the effects exist', async () => {
            const { engine } = fakeEngine();
            const { inst, refreshLayerTree } = fakeInstance(engine);
            await RECIPES.demo.seedViewportEffects(inst, 800, 600);
            expect(refreshLayerTree).toHaveBeenCalled();
        });
    });

    describe('app', () => {
        it('boots the ink pen with a white foreground on a black background', () => {
            expect(RECIPES.app.defaultBrushName).toBe('Ink Pen');
            expect(RECIPES.app.foreground).toEqual({ r: 255, g: 255, b: 255, a: 255 });
            // Regression: the background swatch was hardcoded to white in app
            // state, so the `app` build shipped white-on-white — an invisible
            // foreground/background pair.
            expect(RECIPES.app.background).toEqual({ r: 0, g: 0, b: 0, a: 255 });
            expect(RECIPES.app.background).not.toEqual(RECIPES.app.foreground);
        });

        it('fills the initial layer with opaque black', () => {
            const { engine, api } = fakeEngine();
            RECIPES.app.fillInitialLayer(engine, 3);
            expect(api.fillBackgroundColor).toHaveBeenCalledWith({ id: 3, rgba: [0, 0, 0, 255] });
            expect(api.fillBackground).not.toHaveBeenCalled();
        });

        it('seeds no viewport effects', () => {
            const { engine, api } = fakeEngine();
            const inst = { engine } as unknown as DarklyInstance;
            RECIPES.app.seedViewportEffects(inst, 800, 600);
            expect(api.addFilter).not.toHaveBeenCalled();
            expect(api.setScreenSpaceBoundary).not.toHaveBeenCalled();
        });
    });
});
