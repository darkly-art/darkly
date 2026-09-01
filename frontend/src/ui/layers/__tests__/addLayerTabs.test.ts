import { describe, it, expect } from 'vitest';
import { buildTabs, filterTabs, type TabDeps } from '../addLayerTabs';
import type { AddSource } from '../addSources/types';
import type { Catalog, CatalogEntry } from '../../../engine/protocol_gen';

function entry(type: string, over: Partial<CatalogEntry> = {}): CatalogEntry {
    return {
        type,
        displayName: type,
        icon: null,
        description: null,
        category: null,
        hotkeyAction: null,
        params: [],
        supportsPreview: false,
        source: null,
        addable: true,
        ...over,
    } as CatalogEntry;
}

function catalog(id: string, title: string, entries: CatalogEntry[]): Catalog {
    return { id, title, description: null, icon: null, order: null, entries };
}

/** Deps with a rail whose actions declare menu order, matching the real one. */
function deps(over: Partial<TabDeps> = {}): TabDeps {
    const sources: AddSource[] = [
        { action: 'newLayer', catalog: '', title: 'Normal' },
        { action: 'newFilterLayer', catalog: 'filters' },
        { action: 'newVeil', catalog: 'veils' },
        { action: 'newVoid', catalog: 'voids' },
        { action: 'newGroup', catalog: '', title: 'Normal' },
    ];
    const catalogs: Record<string, Catalog> = {
        filters: catalog('filters', 'Filters', [entry('invert'), entry('curves')]),
        veils: catalog('veils', 'Veils', [entry('grain'), entry('vhs')]),
        voids: catalog('voids', 'Voids', [entry('noise')]),
    };
    const menus: Record<string, string> = {
        newLayer: 'Layer:10',
        newFilterLayer: 'Layer:12',
        newVeil: 'Layer:14',
        newVoid: 'Layer:16',
        newGroup: 'Layer:20',
    };
    const names: Record<string, string> = {
        newLayer: 'New Layer',
        newGroup: 'New Group',
    };
    return {
        sources,
        catalog: id => catalogs[id],
        action: id =>
            menus[id]
                ? {
                      displayName: names[id] ?? id,
                      description: undefined,
                      icon: 'fa6-solid:square-plus',
                      menuPath: [menus[id]],
                  }
                : undefined,
        ...over,
    };
}

describe('buildTabs', () => {
    it('orders the rail by each action’s menu position', () => {
        const titles = buildTabs(deps()).map(t => t.title);
        expect(titles).toEqual(['Normal', 'Filters', 'Veils', 'Voids']);
    });

    it('merges two sources that name the same tab, in rail order', () => {
        // A group is document structure rather than a kind of effect, so it
        // sits beside the plain layer instead of in a rail entry of its own.
        const normal = buildTabs(deps()).find(t => t.title === 'Normal')!;
        expect(normal.cards.map(c => c.entry.displayName)).toEqual(['New Layer', 'New Group']);
        expect(normal.cards.map(c => c.source.action)).toEqual(['newLayer', 'newGroup']);
    });

    it('gives a catalog-less source one synthetic card from its action', () => {
        const normal = buildTabs(deps()).find(t => t.title === 'Normal')!;
        expect(normal.cards[0].entry.displayName).toBe('New Layer');
        expect(normal.cards[0].entry.icon).toBe('fa6-solid:square-plus');
        expect(normal.cards[0].entry.supportsPreview).toBe(false);
        // No catalog to request a preview from.
        expect(normal.cards[0].catalog).toBe('');
    });

    it('titles a catalog-less tab by the kind, not the command that adds it', () => {
        // Left to itself the tab would read "New Layer", the command's name.
        expect(buildTabs(deps()).map(t => t.title)).toContain('Normal');
    });

    it('yields one tab per catalog when no entry declares a category', () => {
        const tabs = buildTabs(deps());
        expect(tabs.filter(t => t.title === 'Voids')).toHaveLength(1);
        expect(tabs.find(t => t.title === 'Voids')!.cards.map(c => c.entry.type)).toEqual(['noise']);
    });

    it('splits one source into two tabs when its entries declare two categories', () => {
        // The shape the rail takes once the veil and filter registries merge
        // into one `effects` catalog — no change to this function.
        const merged = catalog('effects', 'Effects', [
            entry('invert', { category: 'Filters' }),
            entry('grain', { category: 'Veils' }),
            entry('curves', { category: 'Filters' }),
        ]);
        const tabs = buildTabs(
            deps({
                sources: [{ action: 'newVeil', catalog: 'effects' }],
                catalog: id => (id === 'effects' ? merged : undefined),
            }),
        );
        expect(tabs.map(t => t.title)).toEqual(['Filters', 'Veils']);
        // Grouping is map-based, so a category interleaved by `type_id` sort
        // order still collects into one tab rather than repeating.
        expect(tabs[0].cards.map(c => c.entry.type)).toEqual(['invert', 'curves']);
    });

    it('offers an effect registered in two catalogs exactly once', () => {
        // The real duplication: `black_and_white` and `chromatic_aberration`
        // are registered as both a veil and a filter. Each declares which
        // registration owns its add path.
        const filters = catalog('filters', 'Filters', [
            entry('black_and_white', { displayName: 'Black and White' }),
            entry('chromatic_aberration', { displayName: 'Chromatic Aberration', addable: false }),
        ]);
        const veils = catalog('veils', 'Veils', [
            entry('black_and_white', { displayName: 'Black and White', addable: false }),
            entry('chromatic_aberration', { displayName: 'Chromatic Aberration' }),
        ]);
        const tabs = buildTabs(
            deps({
                sources: [
                    { action: 'newFilterLayer', catalog: 'filters' },
                    { action: 'newVeil', catalog: 'veils' },
                ],
                catalog: id => (id === 'filters' ? filters : id === 'veils' ? veils : undefined),
            }),
        );

        const all = tabs.flatMap(t => t.cards.map(c => c.entry.type));
        expect(all.filter(t => t === 'black_and_white')).toHaveLength(1);
        expect(all.filter(t => t === 'chromatic_aberration')).toHaveLength(1);

        // ...and each lands under the registration that owns it, which is what
        // makes its preview and its spawn path correct.
        const bw = tabs.find(t => t.cards.some(c => c.entry.type === 'black_and_white'))!;
        expect(bw.title).toBe('Filters');
        expect(bw.cards.find(c => c.entry.type === 'black_and_white')!.catalog).toBe('filters');

        const ca = tabs.find(t => t.cards.some(c => c.entry.type === 'chromatic_aberration'))!;
        expect(ca.title).toBe('Veils');
        expect(ca.cards.find(c => c.entry.type === 'chromatic_aberration')!.source.action).toBe('newVeil');
    });

    it('drops a tab whose catalog has not arrived yet', () => {
        const tabs = buildTabs(deps({ catalog: () => undefined }));
        expect(tabs.map(t => t.title)).toEqual(['Normal']);
    });
});

describe('filterTabs', () => {
    it('spans tabs and drops those left empty', () => {
        const tabs = filterTabs(buildTabs(deps()), 'grain');
        expect(tabs.map(t => t.title)).toEqual(['Veils']);
        expect(tabs[0].cards.map(c => c.entry.type)).toEqual(['grain']);
    });

    it('returns every tab for an empty query', () => {
        expect(filterTabs(buildTabs(deps()), '   ')).toHaveLength(4);
    });

    it('never resurrects an entry the addability gate removed', () => {
        const veils = catalog('veils', 'Veils', [
            entry('black_and_white', { displayName: 'Black and White', addable: false }),
        ]);
        const tabs = buildTabs(
            deps({
                sources: [{ action: 'newVeil', catalog: 'veils' }],
                catalog: id => (id === 'veils' ? veils : undefined),
            }),
        );
        expect(filterTabs(tabs, 'black')).toEqual([]);
    });
});
