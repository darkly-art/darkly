import { describe, it, expect, beforeAll } from 'vitest';
import { registerActions } from '../index';
import { actions } from '../registry';
import { rustActionDocs } from './rust_action_docs';
import { buildTopMenus } from '../../ui/menu/menuModel';
import { PACK_EXTENSION } from '../pack_actions';

beforeAll(() => {
    actions.setDocs(rustActionDocs());
    registerActions();
});

describe('brush pack actions', () => {
    it('import_and_export_actions_are_registered_with_menu_items', () => {
        for (const id of ['importBrushPack', 'exportBrushPack']) {
            const action = actions.all().find(a => a.id === id);
            expect(action, `${id} is registered`).toBeDefined();
            expect(action!.menuPath, `${id} has a menu path`).toBeDefined();
            // Docs come from the Rust `actions` catalog: an action without
            // them would render a blank menu label.
            expect(action!.displayName, `${id} has a display name`).toBeTruthy();
            expect(action!.icon, `${id} has an icon`).toBeTruthy();
        }
    });

    it('both_land_in_the_file_menu_after_export_timelapse', () => {
        const file = buildTopMenus(actions.all()).find(m => m.title === 'File');
        const ids = file!.entries
            .filter(e => e.kind === 'action')
            .map(e => (e as { actionId: string }).actionId);

        expect(ids).toContain('importBrushPack');
        expect(ids).toContain('exportBrushPack');
        expect(ids.indexOf('importBrushPack')).toBeGreaterThan(ids.indexOf('exportTimelapse'));
        expect(ids.indexOf('exportBrushPack')).toBeGreaterThan(ids.indexOf('importBrushPack'));
    });

    it('both_labels_say_pack_not_brush', () => {
        // The extension names a container, not a count: one `.darkly-brush`
        // may hold twenty brushes, so the artist-facing wording must not imply
        // one.
        for (const id of ['importBrushPack', 'exportBrushPack']) {
            const action = actions.all().find(a => a.id === id)!;
            expect(action.displayName.toLowerCase()).toContain('pack');
        }
    });

    it('the_extension_is_unchanged', () => {
        // One format, and it kept its name.
        expect(PACK_EXTENSION).toBe('.darkly-brush');
    });
});
