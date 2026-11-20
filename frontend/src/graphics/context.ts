/**
 * What a documentation graphic is allowed to ask for.
 *
 * A graphic never reaches for a file or a registry itself: it asks this context
 * by name, and the runner (`frontend/scripts/render-doc-graphics.mjs`) or a test
 * decides where the answer comes from. That is what lets the same component
 * render from the real metadata export on disk and from a fixture in a unit
 * test, and what keeps a graphic that depicts two catalogs (a brush pack and
 * the brushes it groups) from needing a different contract.
 *
 * Every method but `banner` throws rather than returning empty. A graphic with
 * a missing still would otherwise render a hole, and a hole is exactly the kind
 * of thing that gets committed without anyone noticing. A banner is optional by
 * design, so its absence is an answer and not an error.
 */

import type { Membership, PackPalette } from '../engine/protocol_gen';

/** One entry of a catalog, as `metadata.json` exports it, narrowed to what a
 *  graphic may read. Also the shape a sidecar records, so a recording is
 *  exactly a view and not a copy of the export. */
export interface CatalogEntryView {
    type: string;
    displayName: string;
    description?: string;
    icon?: string;
    /** Identity colours; entries that are brush packs. */
    palette?: PackPalette;
    /** The entries of another catalog this entry groups; brush packs. */
    members?: Membership;
}

/** One catalog of the metadata export, narrowed to what a graphic reads. */
export interface CatalogView {
    id: string;
    title: string;
    entries: CatalogEntryView[];
}

/** An Iconify glyph as `@iconify/utils` resolves it: the inner SVG markup and
 *  the viewBox it was drawn in. */
export interface IconGlyph {
    body: string;
    left: number;
    top: number;
    width: number;
    height: number;
}

export interface GraphicContext {
    /** A catalog from the metadata export, narrowed to one of its grouping
     *  labels when `category` is given (the view's `title` is then the category,
     *  since that is what its entries have in common). Throws if the catalog is
     *  not there, or if the category selects nothing. */
    catalog(id: string, category?: string): CatalogView;
    /** One entry of a catalog, for a graphic that shows part of one and should
     *  ask for exactly what it draws. Throws if either id is unknown. */
    entry(catalogId: string, typeId: string): CatalogEntryView;
    /** One entry's committed still, as a data URI. Throws if it is not on disk. */
    still(catalogId: string, typeId: string): string;
    /** A glyph by Iconify name. Throws if no installed icon set has it. */
    icon(name: string): IconGlyph;
    /** One entry's hand-made banner, as a data URI, or `undefined` when none
     *  has been committed. `slotWidth` is the width the graphic will draw it
     *  at; a banner narrower than that would be upscaled, and is an error. */
    banner(catalogId: string, typeId: string, slotWidth: number): string | undefined;
}
