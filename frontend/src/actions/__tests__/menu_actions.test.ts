import { describe, it, expect, beforeAll } from 'vitest';
import { registerActions, NEW_LAYER_ACTION_IDS } from '../index';
import { actions, actionEnablement, parseMenuSegment, type Action } from '../registry';
import { buildTopMenus } from '../../ui/menu/menuModel';
import { filterPalette } from '../../ui/menu/paletteFilter';
import { app } from '../../state/app.svelte';
import { rustActionDocs } from './rust_action_docs';

// Populate the real registry once. `registerActions` is idempotent enough for
// our purposes (re-registering overwrites by id), and tool actions are absent
// here because `tools/index` isn't imported — that's fine, we only assert on
// the menu/palette actions this feature owns. Documentation comes from the Rust
// tables, standing in for the `actions` catalog the editor is handed at init.
beforeAll(() => {
    actions.setDocs(rustActionDocs());
    registerActions();
});

describe('menu action registrations', () => {
    it('registers the new commands; cheatsheet + about live under Help', () => {
        expect(actions.get('commandPalette')).toBeTruthy();
        // The command palette is surfaced as "Find", not as a submenu row.
        expect(actions.get('commandPalette')?.menuPath).toBeUndefined();
        expect(actions.get('openCheatsheet')?.menuPath).toEqual(['Help:10']);
        expect(actions.get('aboutDarkly')?.menuPath).toEqual(['Help:50']);
    });

    it('puts docs, website, and github links under Help, before about', () => {
        expect(actions.get('openDocs')?.menuPath).toEqual(['Help:20']);
        expect(actions.get('openWebsite')?.menuPath).toEqual(['Help:30']);
        expect(actions.get('openGithub')?.menuPath).toEqual(['Help:40']);

        const help = buildTopMenus(actions.all()).find(m => m.title === 'Help');
        const ids = help!.entries
            .filter(e => e.kind === 'action')
            .map(e => (e as { actionId: string }).actionId);
        expect(ids).toEqual([
            'openCheatsheet',
            'openDocs',
            'openWebsite',
            'openGithub',
            'aboutDarkly',
        ]);
    });

    it('puts the selection commands under Select', () => {
        for (const id of ['selectAll', 'clearSelection', 'invertSelection', 'clearSelectionContents', 'maskToSelection', 'alphaToSelection']) {
            const seg = actions.get(id)?.menuPath?.[0];
            expect(parseMenuSegment(seg ?? '').title, id).toBe('Select');
        }
    });

    it('slots maskToSelection into Select right after Invert', () => {
        expect(actions.get('maskToSelection')?.menuPath).toEqual(['Select:35']);
        expect(actions.get('maskToSelection')?.category).toBe('selection');
    });

    it('slots alphaToSelection into Select right after maskToSelection', () => {
        expect(actions.get('alphaToSelection')?.menuPath).toEqual(['Select:36']);
        expect(actions.get('alphaToSelection')?.category).toBe('selection');
    });

    it('disables alphaToSelection with a reason when the active node has no pixels', () => {
        // No layer tree in this environment → activeNode is null, so the
        // action can't know of any pixels to load.
        const a2s = actions.get('alphaToSelection')!;
        expect(a2s.enabled?.()).not.toBe(true);
        expect(actionEnablement(a2s)).toMatchObject({ enabled: false });
        expect(actionEnablement(a2s).reason).toBe('Active layer has no pixels');
    });

    it('disables maskToSelection with a reason when the active layer has no mask', () => {
        // No layer tree / active mask in this environment → activeMaskId is null.
        const m2s = actions.get('maskToSelection')!;
        expect(m2s.enabled?.()).not.toBe(true);
        expect(actionEnablement(m2s)).toMatchObject({ enabled: false });
        expect(actionEnablement(m2s).reason).toBe('No mask on the active layer');
    });

    it('labels clearSelection "Deselect"', () => {
        expect(actions.get('clearSelection')?.displayName).toBe('Deselect');
    });

    it('puts canvas resize + crop under the Image menu', () => {
        expect(actions.get('resizeCanvas')?.menuPath).toEqual(['Image:10']);
        expect(actions.get('cropToSelection')?.menuPath).toEqual(['Image:20']);
    });

    it('puts canvas flip + rotate under the Image menu, in order after crop', () => {
        expect(actions.get('flipCanvasH')?.menuPath).toEqual(['Image:30']);
        expect(actions.get('flipCanvasV')?.menuPath).toEqual(['Image:31']);
        expect(actions.get('rotateCanvasCW')?.menuPath).toEqual(['Image:40']);
        expect(actions.get('rotateCanvasCCW')?.menuPath).toEqual(['Image:41']);
        expect(actions.get('rotateCanvas180')?.menuPath).toEqual(['Image:42']);

        const image = buildTopMenus(actions.all()).find(m => m.title === 'Image');
        const ids = image!.entries
            .filter(e => e.kind === 'action')
            .map(e => (e as { actionId: string }).actionId);
        expect(ids).toEqual([
            'resizeCanvas',
            'rescaleImage',
            'cropToSelection',
            'flipCanvasH',
            'flipCanvasV',
            'rotateCanvasCW',
            'rotateCanvasCCW',
            'rotateCanvas180',
        ]);
    });

    it('puts layer flips under the Layer menu, grouped after Duplicate (no order collision)', () => {
        expect(actions.get('flipLayerH')?.menuPath).toEqual(['Layer:40']);
        expect(actions.get('flipLayerV')?.menuPath).toEqual(['Layer:50']);

        // Assert the whole Layer-menu order so a future duplicate suffix (the
        // bug that put Add Mask between the two flips) can't slip back in.
        const layer = buildTopMenus(actions.all()).find(m => m.title === 'Layer');
        const ids = layer!.entries
            .filter(e => e.kind === 'action')
            .map(e => (e as { actionId: string }).actionId);
        expect(ids).toEqual([
            'newLayer',
            'newFilterLayer',
            'newVeil',
            'newVoid',
            'newGroup',
            'duplicateLayer',
            'flipLayerH',
            'flipLayerV',
            'deleteLayer',
            'toggleVisibility',
            'toggleLock',
            'isolateLayer',
            'addMask',
            'mergeDown',
            'flatten',
        ]);
    });

    it('makes every layer kind the new-layer menu can add reachable from the palette', () => {
        // Searching the palette for a layer kind used to come up empty for
        // veils, voids and filter layers — those existed only as local state
        // inside the layer panel's dropdown.
        const hit = (query: string) => filterPalette(actions.all(), query).map(r => r.id);
        expect(hit('veil')).toContain('newVeil');
        expect(hit('void')).toContain('newVoid');
        expect(hit('filter layer')).toContain('newFilterLayer');
        expect(hit('group')).toContain('newGroup');
    });

    it('backs every new-layer dropdown entry with a registered action', () => {
        // The dropdown renders label + icon straight from these registrations.
        const missing = NEW_LAYER_ACTION_IDS.filter(id => !actions.get(id));
        expect(missing).toEqual([]);
    });

    it('disables cropToSelection with a reason when no selection is active', () => {
        // No WASM handle in this environment → no active selection.
        const crop = actions.get('cropToSelection')!;
        expect(crop.enabled?.()).not.toBe(true);
        expect(actionEnablement(crop)).toMatchObject({ enabled: false });
        expect(actionEnablement(crop).reason).toBe('No active selection');
    });

    it('leaves save actions always enabled (download fallback works everywhere)', () => {
        // Save no longer gates on the File System Access API — browsers without
        // it (Firefox/Safari) fall back to a download, so there's no `enabled`
        // gate at all.
        for (const id of ['saveDocument', 'saveDocumentAs']) {
            const save = actions.get(id)!;
            expect(save.enabled).toBeUndefined();
            expect(actionEnablement(save)).toMatchObject({ enabled: true });
        }
    });

    it('orders items within a menu by the menuPath order suffix, not registration sequence', () => {
        const file = buildTopMenus(actions.all()).find(m => m.title === 'File');
        const ids = file!.entries
            .filter(e => e.kind === 'action')
            .map(e => (e as { actionId: string }).actionId);
        expect(ids).toEqual([
            'newDocument',
            'open',
            'placeSmartObject',
            'saveDocument',
            'saveDocumentAs',
            'exportTimelapse',
        ]);
    });

    it('sorts actions without an order suffix to the end, keeping registration order', () => {
        const regs = [
            { id: 'b', displayName: 'B', category: 'file', icon: 'fa6-solid:circle', menuPath: ['X:20'], handler() {} },
            { id: 'noOrder1', displayName: 'N1', category: 'file', icon: 'fa6-solid:circle', menuPath: ['X'], handler() {} },
            { id: 'a', displayName: 'A', category: 'file', icon: 'fa6-solid:circle', menuPath: ['X:10'], handler() {} },
            { id: 'noOrder2', displayName: 'N2', category: 'file', icon: 'fa6-solid:circle', menuPath: ['X'], handler() {} },
        ] as Action[];
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

    it('gives every registered action a non-empty Iconify icon name', () => {
        const missing = actions.all().filter(a => !a.icon || !a.icon.includes(':'));
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
