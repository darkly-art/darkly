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

import type { CatalogEntryView, CatalogView, GraphicContext, IconGlyph } from '../src/graphics/context';

export interface Raster {
    rgba: Uint8Array;
    width: number;
    height: number;
}

export interface LoadedGraphic {
    /** Absolute path of the component. */
    file: string;
    /** The SSR-compiled module: `default`, `catalog`, optional `category`,
     *  optional `instances`, `graphicProps`, `size`. */
    component: Record<string, unknown>;
    /** The component's source text, for compiling its scoped stylesheet. */
    source: string;
}

export interface OpenGraphics {
    graphics: LoadedGraphic[];
    render: (...args: unknown[]) => { body: string };
    close: () => Promise<void>;
}

/** What a sidecar records of the context a component rendered against: the
 *  catalog views by `id` or `id#category`, the entry views by `catalog/type`. */
export interface ContextRecord {
    catalogs: Record<string, CatalogView>;
    entries: Record<string, CatalogEntryView>;
}

/** Load every graphic through the runner's Vite server. Caller must `close()`. */
export function openGraphics(): Promise<OpenGraphics>;

/** A component's markup with the shared and its own stylesheets spliced in,
 *  for one of its instances (`entry`) or its single image. */
export function renderGraphic(
    component: Record<string, unknown>,
    source: string,
    filename: string,
    ssrRender: (...args: unknown[]) => { body: string },
    ctx: GraphicContext,
    entry?: string,
): string;

/** The file stem of a catalog graphic: the catalog id, plus the category or the
 *  entry when the graphic depicts one. */
export function graphicName(catalog: string, category?: string | null, entry?: string | null): string;

/** An SVG's identity, with the embedded JPEGs and Svelte's scope class normalized. */
export function normalizedHash(svg: string): string;

export function rasterize(svg: string): Raster;

export function encodeJpeg(raster: Raster): Uint8Array;

export function decodeJpeg(bytes: Uint8Array): Raster;

/** Worst tile RMSE between two rasters, 0 to 1. Returns 1 on a size mismatch. */
export function worstTileRmse(a: Raster, b: Raster, cols?: number, rows?: number): number;

/** A context over the real metadata export and the committed stills. */
export function diskContext(
    metadata: unknown,
    options?: { stillsRoot?: string; bannersRoot?: string },
): GraphicContext;

/** The banners under `bannersRoot` for the catalogs and entries in `known`,
 *  checked once; throws on an orphan file or directory. */
export function bannerLookup(
    bannersRoot: string,
    known: Map<string, string[]>,
): GraphicContext['banner'];

/** A glyph by Iconify name, or an error naming it. */
export function iconLookup(name: string): IconGlyph;

/** A context that records the catalog and entry views asked of `ctx`. */
export function recordingContext(ctx: GraphicContext): { record: ContextRecord; context: GraphicContext };

/** A context over a sidecar's recording plus the lookups it does not record. */
export function replayContext(
    record: ContextRecord,
    lookups: Pick<GraphicContext, 'still' | 'icon' | 'banner'>,
): GraphicContext;
