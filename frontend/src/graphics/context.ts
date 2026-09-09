/**
 * What a documentation graphic is allowed to ask for.
 *
 * A graphic never reaches for a file or a registry itself: it asks this context
 * by name, and the runner (`frontend/scripts/render-doc-graphics.mjs`) or a test
 * decides where the answer comes from. That is what lets the same component
 * render from the real metadata export on disk and from a fixture in a unit
 * test, and what keeps a graphic that depicts two catalogs (brush packs, whose
 * entries carry no previews of their own) from needing a different contract.
 *
 * Both methods throw rather than returning empty. A graphic with a missing
 * still would otherwise render a hole, and a hole is exactly the kind of thing
 * that gets committed without anyone noticing.
 */

/** One entry of a catalog, as `metadata.json` exports it. */
export interface CatalogEntryView {
    type: string;
    displayName: string;
    description?: string;
    icon?: string;
}

/** One catalog of the metadata export, narrowed to what a graphic reads. */
export interface CatalogView {
    id: string;
    title: string;
    entries: CatalogEntryView[];
}

export interface GraphicContext {
    /** A catalog from the metadata export. Throws if it is not there. */
    catalog(id: string): CatalogView;
    /** One entry's committed still, as a data URI. Throws if it is not on disk. */
    still(catalogId: string, typeId: string): string;
}
