import { describe, it, expect } from 'vitest';
import {
    parseBinding,
    buildChordIndex,
    resolveChord,
    type ChordEntry,
} from '../hotkey_resolve';

describe('parseBinding', () => {
    it('parses a bare chord as global (no site, no scope)', () => {
        expect(parseBinding('Delete')).toEqual({ site: null, scope: null, brush: null, chord: 'Delete' });
        expect(parseBinding('$mod+Shift+KeyZ')).toEqual({ site: null, scope: null, brush: null, chord: '$mod+Shift+KeyZ' });
    });

    it('parses a site-prefixed chord', () => {
        expect(parseBinding('layerPanel:Delete')).toEqual({
            site: 'layerPanel', scope: null, brush: null, chord: 'Delete',
        });
    });

    it('splits on the first colon only', () => {
        // Defensive: tinykeys notation has no colons today, but if a future
        // chord ever contained one, only the first should split off the site.
        expect(parseBinding('layerPanel:a:b')).toEqual({
            site: 'layerPanel', scope: null, brush: null, chord: 'a:b',
        });
    });

    it('parses a tool-scope on a sited binding', () => {
        // `<site>@<toolGroup>:<chord>` — site=canvas, scope=paint, chord=shift+drag.
        // This is the exact form `brushSizeAdjust` uses so its shift+drag scrub
        // only fires when a paint-group tool is active and not when selection
        // tools are using shift+drag for add-to-selection.
        expect(parseBinding('canvas@paint:shift+drag')).toEqual({
            site: 'canvas', scope: 'paint', brush: null, chord: 'shift+drag',
        });
    });

    it('parses a tool-scope with no site (global-but-tool-scoped)', () => {
        // `@<toolGroup>:<chord>` — no DOM site, only tool-scope. Useful for
        // future keyboard hotkeys that should only fire under a specific tool.
        expect(parseBinding('@paint:KeyB')).toEqual({
            site: null, scope: 'paint', brush: null, chord: 'KeyB',
        });
    });

    it('parses a brush dimension after the tool-scope', () => {
        // `<site>@<toolGroup>@<brush>:<chord>` — the clone brush's set-source
        // gesture. Brush is the third `@`-part.
        expect(parseBinding('canvas@paint@clone:$mod+drag')).toEqual({
            site: 'canvas', scope: 'paint', brush: 'clone', chord: '$mod+drag',
        });
        // Brush with no explicit site still parses (scope + brush only).
        expect(parseBinding('@paint@clone:$mod+drag')).toEqual({
            site: null, scope: 'paint', brush: 'clone', chord: '$mod+drag',
        });
    });

    it('treats `@` in the chord portion as literal', () => {
        // Defensive: only `@` *before the first `:`* splits scope/brush.
        // An `@` after the colon is part of the chord (none use it today,
        // but be explicit).
        expect(parseBinding('canvas:a@b')).toEqual({
            site: 'canvas', scope: null, brush: null, chord: 'a@b',
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

    it('sorts by specificity: site+scope > site > scope > global', () => {
        // Most specific entries should be tried first by the dispatcher so a
        // narrowly-targeted binding wins over a broader one on the same chord.
        const idx = buildChordIndex([
            { actionId: 'global',  bindings: ['shift+drag'] },
            { actionId: 'sited',   bindings: ['canvas:shift+drag'] },
            { actionId: 'scoped',  bindings: ['@paint:shift+drag'] },
            { actionId: 'both',    bindings: ['canvas@paint:shift+drag'] },
        ]);
        const list = idx.get('shift+drag')!;
        expect(list.map(e => e.actionId)).toEqual(['both', 'sited', 'scoped', 'global']);
    });

    it('sorts a brush-scoped entry ahead of the group-scoped one it shares a chord with', () => {
        // Clone's set-source (`canvas@paint@clone`) must out-rank the color
        // sampler (`canvas@paint`) so the shared `$mod+drag` resolves to the
        // brush gesture when clone is active.
        const idx = buildChordIndex([
            { actionId: 'sampleColor',   bindings: ['canvas@paint:$mod+drag'] },
            { actionId: 'setCloneSource', bindings: ['canvas@paint@clone:$mod+drag'] },
        ]);
        const list = idx.get('$mod+drag')!;
        expect(list.map(e => e.actionId)).toEqual(['setCloneSource', 'sampleColor']);
    });
});

describe('resolveChord', () => {
    // Regression: pin scoped-wins-when-active-otherwise-global so a
    // future "simplification" can't quietly drop site scoping.

    function entries(...es: [string | null, string][]): ChordEntry[] {
        return es.map(([site, actionId]) => ({ site, scope: null, brush: null, actionId }));
    }

    function scopedEntries(
        ...es: [string | null, string | null, string][]
    ): ChordEntry[] {
        return es.map(([site, scope, actionId]) => ({ site, scope, brush: null, actionId }));
    }

    function brushEntries(
        ...es: [string | null, string | null, string | null, string][]
    ): ChordEntry[] {
        return es.map(([site, scope, brush, actionId]) => ({ site, scope, brush, actionId }));
    }

    it('falls through to global when no scoped entry matches', () => {
        const e = entries(['layerPanel', 'deleteLayer'], [null, 'clearSelectionContents']);
        const r = resolveChord(e, [], null);
        expect(r?.entry.actionId).toBe('clearSelectionContents');
        expect(r?.site).toBeNull();
    });

    it('picks scoped when its site is active', () => {
        const e = entries(['layerPanel', 'deleteLayer'], [null, 'clearSelectionContents']);
        const r = resolveChord(e, [{ name: 'layerPanel' }], null);
        expect(r?.entry.actionId).toBe('deleteLayer');
        expect(r?.site?.name).toBe('layerPanel');
    });

    it('returns null when no entry matches and there is no global fallback', () => {
        const e = entries(['layerPanel', 'deleteLayer']);
        const r = resolveChord(e, [{ name: 'canvas' }], null);
        expect(r).toBeNull();
    });

    it('regression (bug: shift+drag w/ select tool fired brushSizeAdjust): tool-scoped entry skipped when active tool group differs', () => {
        // brushSizeAdjust binding becomes `canvas@paint:shift+drag`. When the
        // active tool is a select-group tool, the lookup must NOT resolve to
        // it — even though the click site (canvas) matches — because the tool
        // scope doesn't. The dispatcher then returns null and the pointer
        // falls through to rect_select.onPointerDown, where selectionMode(e)
        // reads e.shiftKey and commits add-to-selection.
        const e = scopedEntries(['canvas', 'paint', 'brushSizeAdjust']);
        // Active tool group is 'select' — does not match 'paint' scope.
        expect(resolveChord(e, [{ name: 'canvas' }], 'select')).toBeNull();
        // Active tool group is 'paint' — matches; action fires.
        const r = resolveChord(e, [{ name: 'canvas' }], 'paint');
        expect(r?.entry.actionId).toBe('brushSizeAdjust');
    });

    it('tool-scope alone (no site) fires regardless of site chain', () => {
        // `@paint:KeyB` — no DOM site, only tool-scope. Fires whenever paint
        // group is active, independent of focus.
        const e = scopedEntries([null, 'paint', 'brushPresetReset']);
        expect(resolveChord(e, [], 'paint')?.entry.actionId).toBe('brushPresetReset');
        expect(resolveChord(e, [{ name: 'layerPanel' }], 'paint')?.entry.actionId).toBe('brushPresetReset');
        expect(resolveChord(e, [], 'select')).toBeNull();
        expect(resolveChord(e, [], null)).toBeNull();
    });

    it('site+scope wins over site-only when both bind the same chord', () => {
        // Both compatible with current state; specificity decides.
        const e = scopedEntries(
            ['canvas', 'paint', 'specific'],
            ['canvas', null, 'broad'],
        );
        // Already sorted by buildChordIndex in production, but assert
        // resolveChord respects whatever order it's given and the
        // specific-first ordering is correct.
        const r = resolveChord(e, [{ name: 'canvas' }], 'paint');
        expect(r?.entry.actionId).toBe('specific');
    });

    it('brush-scoped set-source beats group-scoped sampler only when clone is active', () => {
        // Regression pin for the clone gesture: `canvas@paint@clone` and
        // `canvas@paint` share `$mod+drag`. The brush-scoped entry is sorted
        // first (see buildChordIndex test). With clone active it must win;
        // with any other brush it must fall through to the color sampler.
        const e = brushEntries(
            ['canvas', 'paint', 'clone', 'setCloneSource'],
            ['canvas', 'paint', null, 'sampleColor'],
        );
        // Clone active → set-source wins.
        expect(
            resolveChord(e, [{ name: 'canvas' }], 'paint', 'clone')?.entry.actionId,
        ).toBe('setCloneSource');
        // A different brush → the brush-scoped entry is skipped, sampler fires.
        expect(
            resolveChord(e, [{ name: 'canvas' }], 'paint', 'round')?.entry.actionId,
        ).toBe('sampleColor');
        // No named brush (Custom) → still falls through to the sampler.
        expect(
            resolveChord(e, [{ name: 'canvas' }], 'paint', null)?.entry.actionId,
        ).toBe('sampleColor');
    });
});
