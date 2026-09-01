import { describe, it, expect, vi } from 'vitest';
import { validateOverrides } from '../validate';
import type { Catalog, ParamInfo } from '../../engine/protocol_gen';

/** A pref as `settings_catalogs()` projects it. Only the fields
 *  `validateOverrides` reads are meaningful; the rest carry schema-shaped
 *  filler. */
function pref(name: string, kind: string, widget = 'auto'): ParamInfo {
    return {
        kind,
        name,
        label: null,
        description: null,
        widget,
        unit: 'none',
        min: null,
        max: null,
        default: false,
        value: null,
        options: null,
        display: 'normal',
    } as unknown as ParamInfo;
}

/** One `Catalog` per section holding a single entry, matching the shape
 *  `config_schema()` returns. */
function schema(...prefs: ParamInfo[]): Catalog[] {
    return [
        {
            id: 'settings.ui',
            title: 'UI',
            description: null,
            icon: null,
            order: 0,
            entries: [
                {
                    typeId: 'ui',
                    displayName: 'UI',
                    icon: null,
                    description: null,
                    category: null,
                    hotkeyAction: null,
                    params: prefs,
                    supportsPreview: false,
                    captureKind: null,
                },
            ],
        } as unknown as Catalog,
    ];
}

describe('validateOverrides', () => {
    it('a_hidden_pref_survives_validation', () => {
        // Regression: prefs marked `Hidden` were filtered out of the projected
        // schema, so `validateOverrides` saw them as unknown keys, dropped
        // them, and the store wrote the cleaned set back, silently erasing
        // the brush-builder pane state on every reload.
        const sections = schema(
            pref('ui.theme', 'enum'),
            pref('ui.brushBuilder.previewVisible', 'bool', 'hidden'),
        );

        const { cleaned, changed } = validateOverrides(sections, {
            'ui.brushBuilder.previewVisible': false,
        });

        expect(cleaned).toHaveProperty('ui.brushBuilder.previewVisible', false);
        expect(changed).toBe(false);
    });

    it('a_hidden_pref_is_still_not_offered_as_a_setting', () => {
        // The invariant that moved out of Rust: hiding is the renderer's job.
        // This mirrors `SettingsModal.svelte`'s `visiblePrefs` derivation.
        const sections = schema(
            pref('ui.theme', 'enum'),
            pref('ui.brushBuilder.previewVisible', 'bool', 'hidden'),
        );

        const visible = sections
            .flatMap(s => s.entries[0]?.params ?? [])
            .filter(p => p.widget !== 'hidden')
            .map(p => p.name);

        expect(visible).toEqual(['ui.theme']);
    });

    it('an_unknown_key_is_still_dropped', () => {
        // The projection widened to include hidden prefs; it did not stop
        // rejecting keys the schema never declared.
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const { cleaned, changed } = validateOverrides(schema(pref('ui.theme', 'enum')), {
            'ui.nonexistent': 1,
        });
        expect(cleaned).toEqual({});
        expect(changed).toBe(true);
        warn.mockRestore();
    });
});
