import { describe, it, expect } from 'vitest';
import { buildTopMenus, buildHamburgerEntries, type MenuEntry } from '../menuModel';
import type { Action } from '../../../actions/registry';

function reg(id: string, displayName: string, menuPath?: string[]): Action {
    return { id, displayName, category: 'edit', icon: 'fa6-solid:circle', menuPath, handler: () => {} };
}

const SAMPLE = [
    reg('selectAll', 'Select All', ['Select']),
    reg('undo', 'Undo', ['Edit']),
    reg('invertSelection', 'Invert Selection', ['Select']),
    reg('mirrorViewH', 'Mirror View', ['View']),
    reg('openCheatsheet', 'Hotkey Cheat Sheet', ['Help']),
    reg('newDoc', 'New', ['File']),
    reg('sampleColor', 'Sample Color'), // no menuPath → excluded
];

describe('buildTopMenus', () => {
    it('groups actions by menuPath[0]', () => {
        const menus = buildTopMenus(SAMPLE);
        const select = menus.find(m => m.title === 'Select');
        const ids = select?.entries.flatMap(e => (e.kind === 'action' ? [e.actionId] : []));
        expect(ids).toEqual(['selectAll', 'invertSelection']);
    });

    it('orders the top-level menus by the fixed list (Help last)', () => {
        const menus = buildTopMenus(SAMPLE);
        expect(menus.map(m => m.title)).toEqual(['File', 'Edit', 'Select', 'View', 'Help']);
    });

    it('appends the theme widget to the View menu', () => {
        const view = buildTopMenus(SAMPLE).find(m => m.title === 'View')!;
        const last = view.entries[view.entries.length - 1];
        expect(last).toEqual({ kind: 'widget', widget: 'theme' });
    });

    it('excludes actions without a menuPath', () => {
        const menus = buildTopMenus(SAMPLE);
        const allActionIds = menus.flatMap(m =>
            m.entries.flatMap(e => (e.kind === 'action' ? [e.actionId] : [])),
        );
        expect(allActionIds).not.toContain('sampleColor');
    });

    it('appends unknown groups after the known ones', () => {
        const menus = buildTopMenus([reg('z', 'Z', ['Zebra']), reg('n', 'New', ['File'])]);
        expect(menus.map(m => m.title)).toEqual(['File', 'Zebra']);
    });
});

describe('nested menuPath', () => {
    // Two adjustment rows and two veils under one submenu: the shape the
    // Filters menu actually takes.
    const NESTED = [
        reg('bw', 'Black and White', ['Filters:10']),
        reg('invert', 'Invert', ['Filters:10']),
        reg('grain', 'Grain', ['Filters:20', 'Veils']),
        reg('vhs', 'VHS', ['Filters:20', 'Veils']),
    ];

    it('collects a deeper segment into a submenu entry', () => {
        const filters = buildTopMenus(NESTED).find(m => m.title === 'Filters')!;
        expect(filters.entries.map(e => e.kind)).toEqual(['action', 'action', 'submenu']);
        const veils = filters.entries[2] as Extract<MenuEntry, { kind: 'submenu' }>;
        expect(veils.title).toBe('Veils');
        expect(veils.entries.flatMap(e => (e.kind === 'action' ? [e.actionId] : []))).toEqual([
            'grain',
            'vhs',
        ]);
    });

    it('positions the submenu among the rows by the order naming their shared menu', () => {
        const hoisted = [
            reg('bw', 'Black and White', ['Filters:10']),
            reg('grain', 'Grain', ['Filters:5', 'Veils']),
        ];
        const filters = buildTopMenus(hoisted).find(m => m.title === 'Filters')!;
        expect(filters.entries.map(e => e.kind)).toEqual(['submenu', 'action']);
    });

    it('orders rows within a submenu by the order on its own segment', () => {
        const inner = [
            reg('vhs', 'VHS', ['Filters:20', 'Veils:20']),
            reg('grain', 'Grain', ['Filters:20', 'Veils:10']),
        ];
        const filters = buildTopMenus(inner).find(m => m.title === 'Filters')!;
        const veils = filters.entries[0] as Extract<MenuEntry, { kind: 'submenu' }>;
        expect(veils.entries.flatMap(e => (e.kind === 'action' ? [e.actionId] : []))).toEqual([
            'grain',
            'vhs',
        ]);
    });

    it('nests arbitrarily deep', () => {
        const deep = buildTopMenus([reg('x', 'X', ['File:10', 'B', 'C'])]);
        const b = deep[0].entries[0] as Extract<MenuEntry, { kind: 'submenu' }>;
        expect(b.title).toBe('B');
        const c = b.entries[0] as Extract<MenuEntry, { kind: 'submenu' }>;
        expect(c.kind).toBe('submenu');
        expect(c.title).toBe('C');
        expect(c.entries).toEqual([{ kind: 'action', actionId: 'x' }]);
    });

    it('places Filters between Layer and View', () => {
        const menus = buildTopMenus([
            reg('v', 'V', ['View']),
            reg('grain', 'Grain', ['Filters:20', 'Veils']),
            reg('l', 'L', ['Layer']),
        ]);
        expect(menus.map(m => m.title)).toEqual(['Layer', 'Filters', 'View']);
    });

    it('carries the nesting into the hamburger', () => {
        const root = buildHamburgerEntries(NESTED);
        const filters = root.find(
            (e): e is Extract<MenuEntry, { kind: 'submenu' }> =>
                e.kind === 'submenu' && e.title === 'Filters',
        )!;
        expect(filters.entries.some(e => e.kind === 'submenu' && e.title === 'Veils')).toBe(true);
    });
});

describe('buildHamburgerEntries', () => {
    const entries = buildHamburgerEntries(SAMPLE);

    it('leads with a Find item bound to the command palette', () => {
        const first = entries[0] as Extract<MenuEntry, { kind: 'action' }>;
        expect(first.kind).toBe('action');
        expect(first.actionId).toBe('commandPalette');
        expect(first.label).toBe('Find');
        expect(first.icon).toContain('magnifying-glass');
    });

    it('renders the top-level menus as submenu entries', () => {
        const submenuTitles = entries.flatMap(e => (e.kind === 'submenu' ? [e.title] : []));
        expect(submenuTitles).toEqual(['File', 'Edit', 'Select', 'View', 'Help']);
    });

    it('duplicates settings / cheatsheet / about at the root', () => {
        const rootActionIds = entries.flatMap(e => (e.kind === 'action' ? [e.actionId] : []));
        expect(rootActionIds).toContain('openSettings');
        expect(rootActionIds).toContain('openCheatsheet');
        expect(rootActionIds).toContain('aboutDarkly');
    });

    it('gives the root Settings item a gear icon', () => {
        const settings = entries.find(
            (e): e is Extract<MenuEntry, { kind: 'action' }> =>
                e.kind === 'action' && e.actionId === 'openSettings',
        );
        expect(settings?.icon).toContain('gear');
    });

    it('does NOT duplicate the theme widget at the root (it lives in View)', () => {
        expect(entries.some(e => e.kind === 'widget' && e.widget === 'theme')).toBe(false);
    });
});
