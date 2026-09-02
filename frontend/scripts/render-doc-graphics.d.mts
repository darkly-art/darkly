/**
 * Types for the graphics runner, which is plain `.mjs` because it imports
 * `@resvg/resvg-js`, whose own types open with `/// <reference types="node" />`
 * and would drag `@types/node` into a project that deliberately omits it (see
 * `vite.config.ts`).
 *
 * Hand-written rather than generated so the test that imports the runner is
 * type-checked by `tsc --noEmit` like everything else under `src/`, instead of
 * silently arriving as `any` behind a `@ts-ignore`.
 */

import type { GraphicContext } from '../src/graphics/context';

export interface Raster {
    rgba: Uint8Array;
    width: number;
    height: number;
}

export interface LoadedGraphic {
    /** Absolute path of the component. */
    file: string;
    /** The SSR-compiled module: `default`, `catalog`, `graphicProps`, `size`. */
    component: Record<string, unknown>;
    /** The component's source text, for compiling its scoped stylesheet. */
    source: string;
}

export interface OpenGraphics {
    graphics: LoadedGraphic[];
    render: (...args: unknown[]) => { body: string };
    close: () => Promise<void>;
}

/** Load every graphic through the runner's Vite server. Caller must `close()`. */
export function openGraphics(): Promise<OpenGraphics>;

/** A component's markup with its compiled scoped stylesheet spliced in. */
export function renderGraphic(
    component: Record<string, unknown>,
    source: string,
    filename: string,
    ssrRender: (...args: unknown[]) => { body: string },
    ctx: GraphicContext,
): string;

/** An SVG's identity, with the stills and Svelte's scope class normalized. */
export function normalizedHash(svg: string): string;

export function rasterize(svg: string): Raster;

export function encodeJpeg(raster: Raster): Uint8Array;

export function decodeJpeg(bytes: Uint8Array): Raster;

/** Worst tile RMSE between two rasters, 0 to 1. Returns 1 on a size mismatch. */
export function worstTileRmse(a: Raster, b: Raster, cols?: number, rows?: number): number;

/** A context over the real metadata export and the committed stills. */
export function diskContext(
    metadata: unknown,
    options?: { stillsRoot?: string },
): GraphicContext;
