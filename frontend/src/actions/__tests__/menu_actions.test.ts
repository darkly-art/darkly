import { describe, it, expect, beforeAll } from 'vitest';
import { registerActions } from '../index';
import { actions, actionEnablement } from '../registry';
import { app } from '../../state/app.svelte';

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

    it("save actions' enabled() follows canSave (disabled-with-reason here)", () => {
        // The test environment has no File System Access API, so canSave is
        // false; enabled() returns the disabled-reason string (not `true`).
        const save = actions.get('saveDocument');
        const e = save?.enabled?.();
        expect(e).not.toBe(true);
        expect(typeof e).toBe('string');
        expect(actionEnablement(save!)).toMatchObject({ enabled: false });
        expect(actionEnablement(save!).reason).toBeTruthy();
    });

    it('mirrorViewH exposes a status() indicator (icon class, not a bare bool)', () => {
        const status = actions.get('mirrorViewH')?.status;
        expect(typeof status).toBe('function');
        // No active engine instance here, so mirror is off → no status icon.
        expect(app.mirrorH).toBeFalsy();
        expect(status!()).toBeUndefined();
    });
});
