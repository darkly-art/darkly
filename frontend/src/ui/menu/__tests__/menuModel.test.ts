import { describe, it, expect } from 'vitest';
import { buildTopMenus, buildHamburgerEntries, type MenuEntry } from '../menuModel';
import type { ActionRegistration } from '../../../actions/registry';

function reg(id: string, displayName: string, menuPath?: string[]): ActionRegistration {
    return { id, displayName, category: 'edit', menuPath, handler: () => {} };
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
