import { describe, it, expect, beforeAll } from 'vitest';
import { registerActions } from '../index';
import { actions } from '../registry';

// Populate the real registry once. `registerActions` is idempotent enough for
// our purposes (re-registering overwrites by id), and tool actions are absent
// here because `tools/index` isn't imported — that's fine, we only assert on
// the menu/palette actions this feature owns.
beforeAll(() => {
    registerActions();
});

describe('menu action registrations', () => {
    it('registers the new commands; cheatsheet + about live under Help', () => {
        expect(actions.get('commandPalette')).toBeTruthy();
        // The command palette is surfaced as "Find", not as a submenu row.
        expect(actions.get('commandPalette')?.menuPath).toBeUndefined();
        expect(actions.get('openCheatsheet')?.menuPath).toEqual(['Help']);
        expect(actions.get('aboutDarkly')?.menuPath).toEqual(['Help']);
    });

    it('puts the selection commands under Select', () => {
        for (const id of ['selectAll', 'clearSelection', 'invertSelection', 'clearSelectionContents']) {
            expect(actions.get(id)?.menuPath, id).toEqual(['Select']);
        }
    });

    it('labels clearSelection "Deselect"', () => {
        expect(actions.get('clearSelection')?.displayName).toBe('Deselect');
    });

    it("save actions' enabled() follows canSave (false in this environment)", () => {
        // The test environment has no File System Access API, so canSave is
        // false and Save / Save As report disabled with a reason.
        const save = actions.get('saveDocument');
        expect(save?.enabled?.()).toBe(false);
        expect(save?.disabledReason?.()).toBeTruthy();
    });

    it('mirrorViewH exposes a checked() predicate', () => {
        expect(typeof actions.get('mirrorViewH')?.checked).toBe('function');
    });
});
