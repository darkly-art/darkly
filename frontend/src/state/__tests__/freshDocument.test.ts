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
        layerTree: vi.fn(() => Promise.resolve({ layers: [{ type: 'divider', id: 999 }] })),
        moveLayer: vi.fn(() => Promise.resolve(null)),
        groupLayers: vi.fn((_req: { ids: number[] }) => Promise.resolve(nextId++)),
        setLayerName: vi.fn(),
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

        it('seeds four effect layers and moves the divider below all of them', async () => {
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
            // One divider move at the end, not one per layer: adding a layer
            // never crosses the divider on its own. The divider goes below the
            // bottom-most seeded effect, putting all four in viewport space.
            expect(api.moveLayer).toHaveBeenCalledTimes(1);
            const firstSeeded = await api.addFilter.mock.results[0].value;
            expect(api.moveLayer).toHaveBeenCalledWith({
                id: 999,
                target: { target_type: 'before', target_id: firstSeeded },
            });
        });

        // The four are one named group, so the starter document reads as a
        // single row. Grouped after the boundary moves, because the new group
        // inherits the topmost source's side of the divider — group first and
        // the whole arrangement lands in canvas space.
        it('wraps the four effects in one named group, after the divider moves', async () => {
            const { engine, api } = fakeEngine();
            const { inst } = fakeInstance(engine);
            await RECIPES.demo.seedViewportEffects(inst, 800, 600);

            expect(api.groupLayers).toHaveBeenCalledTimes(1);
            const grouped = api.groupLayers.mock.calls[0][0].ids;
            const seeded = await Promise.all(
                api.addFilter.mock.results.map((r: any) => r.value),
            );
            expect(grouped).toEqual(seeded);

            expect(api.setLayerName).toHaveBeenCalledWith({
                id: await api.groupLayers.mock.results[0].value,
                name: 'Viewport Effects',
            });

            const boundaryOrder = api.moveLayer.mock.invocationCallOrder[0];
            const groupOrder = api.groupLayers.mock.invocationCallOrder[0];
            expect(boundaryOrder).toBeLessThan(groupOrder);
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
            expect(api.moveLayer).not.toHaveBeenCalled();
        });
    });
});
