import { describe, it, expect } from 'vitest';
import { buildMenu } from '../menuModel';
import type { ActionRegistration } from '../../../actions/registry';

function reg(id: string, displayName: string, menuPath?: string[]): ActionRegistration {
    return { id, displayName, category: 'edit', menuPath, handler: () => {} };
}

describe('buildMenu', () => {
    it('groups actions by menuPath[0]', () => {
        const groups = buildMenu([
            reg('selectAll', 'Select All', ['Select']),
            reg('undo', 'Undo', ['Edit']),
            reg('invertSelection', 'Invert Selection', ['Select']),
        ]);
        const select = groups.find(g => g.title === 'Select');
        expect(select?.items.map(i => i.actionId)).toEqual(['selectAll', 'invertSelection']);
    });

    it('orders top-level menus by the fixed list', () => {
        const groups = buildMenu([
            reg('mirror', 'Mirror', ['View']),
            reg('reset', 'Reset', ['Colors']),
            reg('new', 'New', ['File']),
            reg('undo', 'Undo', ['Edit']),
        ]);
        expect(groups.map(g => g.title)).toEqual(['File', 'Edit', 'Colors', 'View']);
    });

    it('appends unknown groups after the known ones, in first-seen order', () => {
        const groups = buildMenu([
            reg('z', 'Z', ['Zebra']),
            reg('n', 'New', ['File']),
            reg('a', 'A', ['Aardvark']),
        ]);
        expect(groups.map(g => g.title)).toEqual(['File', 'Zebra', 'Aardvark']);
    });

    it('sets each leaf label to displayName', () => {
        const groups = buildMenu([reg('clearSelection', 'Deselect', ['Select'])]);
        expect(groups[0].items[0].label).toBe('Deselect');
    });

    it('excludes actions without a menuPath', () => {
        const groups = buildMenu([
            reg('sampleColor', 'Sample Color'),
            reg('undo', 'Undo', ['Edit']),
        ]);
        expect(groups).toHaveLength(1);
        expect(groups[0].title).toBe('Edit');
    });

    it('the Select group carries the three selection commands', () => {
        const groups = buildMenu([
            reg('selectAll', 'Select All', ['Select']),
            reg('clearSelection', 'Deselect', ['Select']),
            reg('invertSelection', 'Invert Selection', ['Select']),
            reg('clearSelectionContents', 'Clear Selection Contents', ['Select']),
        ]);
        const ids = groups.find(g => g.title === 'Select')?.items.map(i => i.actionId);
        expect(ids).toContain('selectAll');
        expect(ids).toContain('clearSelection');
        expect(ids).toContain('invertSelection');
    });
});
