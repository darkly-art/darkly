import { describe, it, expect } from 'vitest';
import {
    parseBinding,
    buildChordIndex,
    resolveChord,
    type ChordEntry,
} from '../hotkey_resolve';

describe('parseBinding', () => {
    it('parses a bare chord as global (no site)', () => {
        expect(parseBinding('Delete')).toEqual({ site: null, chord: 'Delete' });
        expect(parseBinding('$mod+Shift+KeyZ')).toEqual({ site: null, chord: '$mod+Shift+KeyZ' });
    });

    it('parses a site-prefixed chord', () => {
        expect(parseBinding('layerPanel:Delete')).toEqual({
            site: 'layerPanel', chord: 'Delete',
        });
    });

    it('splits on the first colon only', () => {
        // Defensive: tinykeys notation has no colons today, but if a future
        // chord ever contained one, only the first should split off the site.
        expect(parseBinding('layerPanel:a:b')).toEqual({
            site: 'layerPanel', chord: 'a:b',
        });
    });
});

describe('buildChordIndex', () => {
    it('regression: scoped entries sort before global on the same chord', () => {
        // The previous flat-global keymap couldn't distinguish two actions
        // bound to `Delete`; last-writer won. The new index must put the
        // scoped entry first so the dispatcher tries it before falling
        // back to global. This is the exact arrangement that the
        // Photoshop preset's deleteLayer creates against the existing
        // global Delete → clearSelectionContents binding.
        const idx = buildChordIndex([
            { actionId: 'clearSelectionContents', bindings: ['Delete'] },
            { actionId: 'deleteLayer',            bindings: ['layerPanel:Delete'] },
        ]);
        const list = idx.get('Delete');
        expect(list).toBeDefined();
        expect(list!.map(e => e.actionId)).toEqual(['deleteLayer', 'clearSelectionContents']);
        expect(list!.map(e => e.site)).toEqual(['layerPanel', null]);
    });

    it('drops empty-chord bindings (e.g. preset unset)', () => {
        // Photoshop's preset sets `isolateLayer = ""` to unbind the default.
        // effectiveHotkeys returns [] for those; this guards against an
        // empty chord sneaking into the index if a malformed value ever
        // reached buildChordIndex.
        const idx = buildChordIndex([
            { actionId: 'a', bindings: [''] },
            { actionId: 'b', bindings: ['layerPanel:'] },
        ]);
        expect(idx.size).toBe(0);
    });
});

describe('resolveChord', () => {
    // Regression: pin scoped-wins-when-active-otherwise-global so a
    // future "simplification" can't quietly drop site scoping.

    function entries(...es: [string | null, string][]): ChordEntry[] {
        return es.map(([site, actionId]) => ({ site, actionId }));
    }

    it('falls through to global when no scoped entry matches', () => {
        const e = entries(['layerPanel', 'deleteLayer'], [null, 'clearSelectionContents']);
        const r = resolveChord(e, []);
        expect(r?.entry.actionId).toBe('clearSelectionContents');
        expect(r?.site).toBeNull();
    });

    it('picks scoped when its site is active', () => {
        const e = entries(['layerPanel', 'deleteLayer'], [null, 'clearSelectionContents']);
        const r = resolveChord(e, [{ name: 'layerPanel' }]);
        expect(r?.entry.actionId).toBe('deleteLayer');
        expect(r?.site?.name).toBe('layerPanel');
    });

    it('returns null when no entry matches and there is no global fallback', () => {
        const e = entries(['layerPanel', 'deleteLayer']);
        const r = resolveChord(e, [{ name: 'canvas' }]);
        expect(r).toBeNull();
    });
});
