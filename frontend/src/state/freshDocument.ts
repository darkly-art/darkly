import type { Engine } from '../engine/protocol';
import type { Color, DarklyInstance } from './app.svelte';

/** Which deploy flavor this build was compiled for. Selected at build time by
 *  Vite's `--mode` and injected as `__DARKLY_APP_MODE__` (see vite.config.ts). */
export type DeployMode = 'demo' | 'app';

export const deployMode: DeployMode = __DARKLY_APP_MODE__;

/** The starter content of a fresh document — everything that differs between the
 *  decorative `demo.darkly.art` build and the clean `app` build. Consumers call
 *  the two hooks without knowing which flavor they got; a new mode is a purely
 *  additive entry in {@link RECIPES}. */
interface FreshDocumentRecipe {
    /** The brush preset selected on boot, and the default this build's
     *  "reset colors" returns to. */
    defaultBrushName: string;
    /** The initial foreground paint color for this build. */
    foreground: Color;
    /** The initial background swatch color for this build — the other half of
     *  the foreground/background pair "reset colors" returns to and "swap"
     *  toggles into. */
    background: Color;
    /** Fill the freshly-created initial background layer. */
    fillInitialLayer(engine: Engine, layerId: number): void;
    /** Seed starter viewport effects / extras after the initial layer is
     *  filled. */
    seedViewportEffects(instance: DarklyInstance, docW: number, docH: number): void;
}

/** Per-flavor starter-content recipes. Exported for tests; consumers use
 *  {@link freshDocument}, the entry for this build's flavor. */
export const RECIPES: Record<DeployMode, FreshDocumentRecipe> = {
    // Demo: the night-sky background image plus the four hidden viewport
    // effects new users discover the feature through.
    demo: {
        defaultBrushName: 'Rough Watercolor',
        foreground: { r: 0, g: 0, b: 0, a: 255 },
        background: { r: 255, g: 255, b: 255, a: 255 },
        fillInitialLayer: (engine, id) => engine.api.fillBackground({ id }),
        seedViewportEffects: (instance, _w, _h) => {
            // Four effect layers, hidden, then the divider dragged over all
            // four so they are viewport-only. Order matters: each is added at
            // the top of the stack, and `setScreenSpaceBoundary` runs once at
            // the end because the run only grows when the user (or this) says
            // so — adding a layer never crosses the divider on its own.
            const api = instance.engine!.api;
            for (const [pipeline, params] of [
                ['rainy_glass', { direction: 135 }],
                ['grain', { speed: 0.05 }],
                ['lens_blur', { radius: 0.25 }],
                ['vhs', {}],
            ] as const) {
                api.addFilter({ pipeline, params, anchor: null });
            }
            api.setScreenSpaceBoundary({ count: 4 });
        },
    },
    // App: a clean editor — an opaque black layer painted with a white ink pen,
    // and no pre-seeded viewport effects. The feature still exists; it is
    // simply not pre-populated.
    app: {
        defaultBrushName: 'Ink Pen',
        foreground: { r: 255, g: 255, b: 255, a: 255 },
        background: { r: 0, g: 0, b: 0, a: 255 },
        fillInitialLayer: (engine, id) => engine.api.fillBackgroundColor({ id, rgba: [0, 0, 0, 255] }),
        seedViewportEffects: () => {},
    },
};

/** Starter-content recipe for this build's deploy flavor. */
export const freshDocument = RECIPES[deployMode];
