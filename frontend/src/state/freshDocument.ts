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
    /** Seed default veils / extras after the initial layer is filled. */
    seedVeils(instance: DarklyInstance, docW: number, docH: number): void;
}

/** Per-flavor starter-content recipes. Exported for tests; consumers use
 *  {@link freshDocument}, the entry for this build's flavor. */
export const RECIPES: Record<DeployMode, FreshDocumentRecipe> = {
    // Demo: the night-sky background image plus the four hidden veils new users
    // discover the feature through.
    demo: {
        defaultBrushName: 'Rough Watercolor',
        foreground: { r: 0, g: 0, b: 0, a: 255 },
        background: { r: 255, g: 255, b: 255, a: 255 },
        fillInitialLayer: (engine, id) => engine.api.fillBackground({ id }),
        seedVeils: (instance, w, h) => {
            // The veil chain needs a non-zero viewport before `add_veil` will
            // allocate textures; without this `ensure_textures` no-ops and the
            // next call would unwrap on `views`. CanvasView issues its own
            // resize to the surface dims right after, so the only cost is one
            // GPU realloc that's immediately replaced.
            instance.engine!.api.resize({ width: w, height: h });
            instance.addVeil('rainy_glass', { direction: 135, visible: false });
            instance.addVeil('grain', { speed: 0.05, visible: false });
            instance.addVeil('lens_blur', { radius: 0.25, visible: false });
            instance.addVeil('vhs', { visible: false });
        },
    },
    // App: a clean editor — an opaque black layer painted with a white ink pen,
    // and no pre-seeded veils. The veil feature still exists; it is simply not
    // pre-populated.
    app: {
        defaultBrushName: 'Ink Pen',
        foreground: { r: 255, g: 255, b: 255, a: 255 },
        background: { r: 0, g: 0, b: 0, a: 255 },
        fillInitialLayer: (engine, id) => engine.api.fillBackgroundColor({ id, rgba: [0, 0, 0, 255] }),
        seedVeils: () => {},
    },
};

/** Starter-content recipe for this build's deploy flavor. */
export const freshDocument = RECIPES[deployMode];
