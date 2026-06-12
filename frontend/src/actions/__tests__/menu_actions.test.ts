import { describe, it, expect, beforeAll } from 'vitest';
import { registerActions } from '../index';
import { actions, actionEnablement, parseMenuSegment, type ActionRegistration } from '../registry';
import { buildTopMenus } from '../../ui/menu/menuModel';
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
        expect(actions.get('openCheatsheet')?.menuPath).toEqual(['Help:10']);
        expect(actions.get('aboutDarkly')?.menuPath).toEqual(['Help:20']);
    });

    it('puts the selection commands under Select', () => {
        for (const id of ['selectAll', 'clearSelection', 'invertSelection', 'clearSelectionContents']) {
            const seg = actions.get(id)?.menuPath?.[0];
            expect(parseMenuSegment(seg ?? '').title, id).toBe('Select');
        }
    });

    it('labels clearSelection "Deselect"', () => {
        expect(actions.get('clearSelection')?.displayName).toBe('Deselect');
    });

    it('puts canvas resize + crop under the Image menu', () => {
        expect(actions.get('resizeCanvas')?.menuPath).toEqual(['Image:10']);
        expect(actions.get('cropToSelection')?.menuPath).toEqual(['Image:20']);
    });

    it('disables cropToSelection with a reason when no selection is active', () => {
        // No WASM handle in this environment → no active selection.
        const crop = actions.get('cropToSelection')!;
        expect(crop.enabled?.()).not.toBe(true);
        expect(actionEnablement(crop)).toMatchObject({ enabled: false });
        expect(actionEnablement(crop).reason).toBe('No active selection');
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

    it('orders items within a menu by the menuPath order suffix, not registration sequence', () => {
        const file = buildTopMenus(actions.all()).find(m => m.title === 'File');
        const ids = file!.entries
            .filter(e => e.kind === 'action')
            .map(e => (e as { actionId: string }).actionId);
        expect(ids).toEqual(['newDocument', 'open', 'saveDocument', 'saveDocumentAs', 'exportImage']);
    });

    it('sorts actions without an order suffix to the end, keeping registration order', () => {
        const regs = [
            { id: 'b', displayName: 'B', category: 'file', icon: 'fa-circle', menuPath: ['X:20'], handler() {} },
            { id: 'noOrder1', displayName: 'N1', category: 'file', icon: 'fa-circle', menuPath: ['X'], handler() {} },
            { id: 'a', displayName: 'A', category: 'file', icon: 'fa-circle', menuPath: ['X:10'], handler() {} },
            { id: 'noOrder2', displayName: 'N2', category: 'file', icon: 'fa-circle', menuPath: ['X'], handler() {} },
        ] as ActionRegistration[];
        const x = buildTopMenus(regs).find(m => m.title === 'X');
        const ids = x!.entries.map(e => (e as { actionId: string }).actionId);
        expect(ids).toEqual(['a', 'b', 'noOrder1', 'noOrder2']);
    });

    it('parseMenuSegment splits the title from the optional order suffix', () => {
        expect(parseMenuSegment('Help')).toEqual({ title: 'Help' });
        expect(parseMenuSegment('Help:10')).toEqual({ title: 'Help', order: 10 });
        // Non-numeric suffix is not an order — treat the whole thing as a title.
        expect(parseMenuSegment('A:B')).toEqual({ title: 'A:B' });
    });

    it('gives every registered action a non-empty Font Awesome icon', () => {
        const missing = actions.all().filter(a => !a.icon || !a.icon.startsWith('fa-'));
        expect(missing.map(a => a.id)).toEqual([]);
    });

    it('mirrorViewH exposes a status() indicator (icon class, not a bare bool)', () => {
        const status = actions.get('mirrorViewH')?.status;
        expect(typeof status).toBe('function');
        // No active engine instance here, so mirror is off → no status icon.
        expect(app.mirrorH).toBeFalsy();
        expect(status!()).toBeUndefined();
    });
});
