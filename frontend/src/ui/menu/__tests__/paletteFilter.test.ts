import { describe, it, expect } from 'vitest';
import { paletteActions, filterPalette } from '../paletteFilter';
import type { Action } from '../../../actions/registry';

function reg(
    id: string,
    displayName: string,
    extra: Partial<Action> = {},
): Action {
    return { id, displayName, category: 'edit', icon: 'fa6-solid:circle', handler: () => {}, ...extra };
}

describe('paletteActions', () => {
    it("excludes type:'hold' actions", () => {
        const out = paletteActions([
            reg('selectAll', 'Select All'),
            reg('sampleColor', 'Sample Color', { type: 'hold' }),
        ]);
        expect(out.map(r => r.id)).toEqual(['selectAll']);
    });
});

describe('filterPalette', () => {
    const regs = [
        reg('invertSelection', 'Invert Selection', { description: 'Invert the current selection.', menuPath: ['Select'] }),
        reg('selectAll', 'Select All', { description: 'Select the entire canvas.', menuPath: ['Select'] }),
        reg('undo', 'Undo', { description: 'Undo the last action.', menuPath: ['Edit'] }),
        reg('sampleColor', 'Sample Color', { type: 'hold' }),
    ];

    it('empty query returns all eligible actions (hold excluded)', () => {
        expect(filterPalette(regs, '').map(r => r.id)).toEqual([
            'invertSelection', 'selectAll', 'undo',
        ]);
    });

    it('matches substrings across the name', () => {
        expect(filterPalette(regs, 'invert').map(r => r.id)).toEqual(['invertSelection']);
    });

    it('matches against the description', () => {
        // "canvas" only appears in selectAll's description.
        expect(filterPalette(regs, 'canvas').map(r => r.id)).toEqual(['selectAll']);
    });

    it('matches against the menuPath / category', () => {
        const ids = filterPalette(regs, 'select').map(r => r.id);
        expect(ids).toContain('invertSelection');
        expect(ids).toContain('selectAll');
    });

    it('ranks name-prefix matches ahead of substring/other matches', () => {
        const ranked = filterPalette(
            [
                reg('invertSelection', 'Invert Selection'),
                reg('selectAll', 'Select All'),
            ],
            'select',
        );
        // "Select All" starts with the query; "Invert Selection" only contains it.
        expect(ranked[0].id).toBe('selectAll');
    });

    it('never returns hold actions even when they would match', () => {
        expect(filterPalette(regs, 'color').map(r => r.id)).not.toContain('sampleColor');
    });
});
