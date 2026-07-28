import { describe, it, expect, vi } from 'vitest';
import { RECIPES } from '../freshDocument';
import type { Engine } from '../../engine/protocol';
import type { DarklyInstance } from '../app.svelte';

/** Minimal fake engine exposing just the `api` methods the recipes call. */
function fakeEngine() {
    const api = {
        fillBackground: vi.fn(),
        fillBackgroundColor: vi.fn(),
        resize: vi.fn(),
    };
    return { engine: { api } as unknown as Engine, api };
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

        it('seeds the four hidden veils after resizing', () => {
            const { engine, api } = fakeEngine();
            const addVeil = vi.fn();
            const inst = { engine, addVeil } as unknown as DarklyInstance;
            RECIPES.demo.seedVeils(inst, 800, 600);
            expect(api.resize).toHaveBeenCalledWith({ width: 800, height: 600 });
            expect(addVeil).toHaveBeenCalledTimes(4);
            expect(addVeil.mock.calls.map((c) => c[0])).toEqual([
                'rainy_glass',
                'grain',
                'lens_blur',
                'vhs',
            ]);
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

        it('seeds no veils', () => {
            const addVeil = vi.fn();
            const { engine, api } = fakeEngine();
            const inst = { engine, addVeil } as unknown as DarklyInstance;
            RECIPES.app.seedVeils(inst, 800, 600);
            expect(addVeil).not.toHaveBeenCalled();
            expect(api.resize).not.toHaveBeenCalled();
        });
    });
});
