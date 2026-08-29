import type { CatalogEntry } from '../../../engine/protocol_gen';

/**
 * One way of putting something new into the document.
 *
 * The add-layer modal's tab rail is derived from these — drop a file in this
 * directory and the rail grows a tab, with its label, icon and position coming
 * from the action it names. Nothing outside this directory enumerates the ways
 * to add a layer.
 *
 * A source either picks from a registry catalog (Filters, Veils, Voids) or is
 * the whole choice by itself (Normal, Group). The second kind contributes one
 * synthetic card built from its action's own metadata, so every tab is a grid
 * of cards and the modal never branches on which kind it is showing.
 */
export interface AddSource {
    /**
     * Action this source is bound to. Supplies the tab's icon and description,
     * its position in the rail (from the action's `menuPath` order), and — for
     * a source with no `spawn` — the thing that runs when a card is chosen.
     */
    action: string;
    /** Registry catalog to pick from, or `''` when choosing the kind is the whole choice. */
    catalog: string;
    /** Tab title. Falls back to the catalog's own title, then the action's name. */
    title?: string;
    /**
     * Create one from a chosen entry. Absent means "dispatch `action`", which is
     * what keeps `newLayer` and `newGroup` spawning through their existing
     * handlers rather than duplicating them here.
     */
    spawn?: (entry: CatalogEntry) => Promise<void>;
}
