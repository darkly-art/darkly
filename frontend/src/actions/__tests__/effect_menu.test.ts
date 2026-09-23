// The destructive-apply actions the effect catalog generates, and where they
// land in the menu. Placement is derived from each effect's own declared
// category, so this seeds a catalog and reads the menu back out.
//
// Separate from `menu_actions.test.ts` because the effect actions only exist
// once the catalog is loaded, and that test deliberately asserts against the
// registrations that stand without one.
import { afterEach, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { DarklyInstance, setActiveInstance } from '../../state/app.svelte';
import { registerActions } from '../index';
import { actions, type Action } from '../registry';
import { catalogs } from '../../state/catalogs.svelte';
import { withApi } from '../../engine/testApi';
import { buildTopMenus, type MenuEntry } from '../../ui/menu/menuModel';
import { filterPalette } from '../../ui/menu/paletteFilter';
import type { Catalog, CatalogEntry, ParamInfo } from '../../engine/protocol_gen';
import { rustActionDocs } from './rust_action_docs';

function entry(type: string, displayName: string, category: string, params: ParamInfo[] = []): CatalogEntry {
    return {
        type,
        displayName,
        icon: 'tabler:circle',
        description: `${displayName} does something.`,
        category,
        hotkeyAction: `effect${type}`,
        params,
        supportsPreview: false,
        source: null,
    } as CatalogEntry;
}

// Two adjustments, two veils, and one category the core does not declare
// today: the third is what proves a new category needs no edit on this side.
const EFFECTS: Catalog = {
    id: 'effects',
    title: 'Effects',
    description: null,
    icon: null,
    order: null,
    entries: [
        entry('Invert', 'Invert', 'Filters'),
        entry('Levels', 'Levels', 'Filters', [{ name: 'gamma' } as ParamInfo]),
        entry('Grain', 'Grain', 'Veils'),
        entry('Vhs', 'VHS', 'Veils'),
        entry('Ripple', 'Ripple', 'Distorts'),
    ],
};

function filtersMenu() {
    return buildTopMenus(actions.all()).find(m => m.title === 'Filters')!;
}

function submenu(title: string): Extract<MenuEntry, { kind: 'submenu' }> {
    const found = filtersMenu().entries.find(
        (e): e is Extract<MenuEntry, { kind: 'submenu' }> => e.kind === 'submenu' && e.title === title,
    );
    expect(found, `a ${title} submenu`).toBeTruthy();
    return found!;
}

const idsOf = (entries: MenuEntry[]) =>
    entries.flatMap(e => (e.kind === 'action' ? [e.actionId] : []));

// The registries are process state, so the singleton is what `registerActions`
// reads; loading it once here is what a real session's bootstrap does.
beforeAll(async () => {
    await catalogs.load(withApi({ send: async (kind: string) => (kind === 'catalogs' ? [EFFECTS] : null) }));
});

beforeEach(() => {
    setActiveInstance(new DarklyInstance());
    actions.setDocs(rustActionDocs());
    registerActions();
});

afterEach(() => {
    setActiveInstance(null);
});

describe('effect menu placement', () => {
    it('puts the effects under Filters, and has no Colors menu', () => {
        const titles = buildTopMenus(actions.all()).map(m => m.title);
        expect(titles).toContain('Filters');
        expect(titles).not.toContain('Colors');
    });

    it('gives the menu-named category direct rows and every other one a submenu', () => {
        const entries = filtersMenu().entries;
        expect(idsOf(entries)).toEqual(['effectInvert', 'effectLevels']);
        expect(idsOf(submenu('Veils').entries)).toEqual(['effectGrain', 'effectVhs']);
    });

    it('grows a submenu for a category the frontend has never heard of', () => {
        expect(idsOf(submenu('Distorts').entries)).toEqual(['effectRipple']);
    });

    it('still marks a parametric effect as opening a dialog', () => {
        expect(actions.get('effectLevels')?.displayName).toBe('Levels…');
        expect(actions.get('effectInvert')?.displayName).toBe('Invert');
    });

    it('makes the submenu title a palette search term', () => {
        const hits = filterPalette(actions.all() as Action[], 'veils').map(a => a.id);
        expect(hits).toEqual(expect.arrayContaining(['effectGrain', 'effectVhs']));
        expect(hits).not.toContain('effectInvert');
    });
});
