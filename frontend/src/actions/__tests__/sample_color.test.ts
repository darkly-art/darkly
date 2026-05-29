import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock app/config/coordinates *before* importing the module under test so
// the action's `import { app }` / `startPick` chain resolves to our fakes.
const { handle, fakeApp, fakeConfig } = vi.hoisted(() => {
    const handle = {
        pick_color: vi.fn(),
        has_pending_color_pick: vi.fn().mockReturnValue(false),
        last_picked_color: vi.fn().mockReturnValue(new Uint8Array([0, 0, 0, 0])),
    };
    const fakeApp = {
        handle,
        activeLayerId: null as number | null,
        canvasEl: {} as HTMLCanvasElement, // present-but-empty stub
        foreground: { r: 0, g: 0, b: 0, a: 255 },
    };
    const fakeConfig = {
        _mode: 'merged',
        get: vi.fn((_key: string) => fakeConfig._mode),
    };
    return { handle, fakeApp, fakeConfig };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../config/store.svelte', () => ({ config: fakeConfig }));
// `onMove` calls screenToCanvas — stub it to return a known canvas point.
vi.mock('../../canvas/coordinates', () => ({
    screenToCanvas: vi.fn(() => ({ x: 50, y: 60 })),
}));

import { actions } from '../registry';
import { registerSampleColorAction } from '../sample_color';
import { buildChordIndex, resolveChord } from '../hotkey_resolve';

beforeEach(() => {
    handle.pick_color.mockClear();
    handle.has_pending_color_pick.mockClear();
    handle.last_picked_color.mockClear();
});

describe('sampleColor action registration', () => {
    it('registers under the id "sampleColor" with the colors category', () => {
        registerSampleColorAction();
        const action = actions.get('sampleColor');
        expect(action).toBeDefined();
        expect(action!.category).toBe('colors');
        expect(action!.type).toBe('hold');
    });

    it('handler queues a pick at the ctx canvas coordinates', () => {
        registerSampleColorAction();
        const action = actions.get('sampleColor')!;
        action.handler({ x: 123, y: 456 });
        // `merged` mode + null activeLayerId → layer_id sentinel -1.
        expect(handle.pick_color).toHaveBeenCalledWith(123, 456, -1);
    });

    it('onMove queues a pick at the screenToCanvas-converted coordinates', () => {
        registerSampleColorAction();
        const action = actions.get('sampleColor')!;
        const fakeEvent = { clientX: 999, clientY: 888 } as unknown as PointerEvent;
        action.onMove!({ x: 0, y: 0 }, fakeEvent, 0, 0);
        // screenToCanvas stub returns { x: 50, y: 60 }.
        expect(handle.pick_color).toHaveBeenCalledWith(50, 60, -1);
    });
});

describe('sampleColor chord resolution (canvas@paint)', () => {
    it('resolves under the paint scope at site=canvas with chord=ctrl+drag', () => {
        // Defensive integration — proves that the registered binding string
        // parses into a chord-index entry that the dispatcher would match
        // when a paint-group tool is active.
        const idx = buildChordIndex([
            { actionId: 'sampleColor', bindings: ['canvas@paint:ctrl+drag'] },
        ]);
        const entries = idx.get('ctrl+drag');
        expect(entries).toBeDefined();
        const resolved = resolveChord(entries!, [{ name: 'canvas' }], 'paint');
        expect(resolved).not.toBeNull();
        expect(resolved!.entry.actionId).toBe('sampleColor');
    });

    it('does NOT resolve when a non-paint tool is active', () => {
        // `select` is the group on rect/ellipse/lasso/polygon select tools —
        // they have their own ctrl-modified gestures and must not steal
        // sampleColor's chord.
        const idx = buildChordIndex([
            { actionId: 'sampleColor', bindings: ['canvas@paint:ctrl+drag'] },
        ]);
        const entries = idx.get('ctrl+drag');
        expect(entries).toBeDefined();
        const resolved = resolveChord(entries!, [{ name: 'canvas' }], 'select');
        expect(resolved).toBeNull();
    });
});

// `mods.ts` caches the platform at module load, so each test re-imports
// the module after re-mocking. Mirrors what `rebuildClickIndex` does at
// runtime: substitute `$mod` in the binding, then build the index.
describe('sampleColor chord resolution with the preset $mod+drag binding', () => {
    async function loadModsAs(os: 'linux' | 'windows' | 'macos') {
        vi.resetModules();
        vi.doMock('../../platform', () => ({
            detectPlatform: () => ({ os, browser: 'chromium' }),
        }));
        return await import('../mods');
    }

    it('resolves on a literal ctrl+drag event on Linux (Krita/GIMP preset)', async () => {
        const { substituteModInBinding } = await loadModsAs('linux');
        const { buildChordIndex, resolveChord } = await import('../hotkey_resolve');
        const idx = buildChordIndex([
            { actionId: 'sampleColor', bindings: [substituteModInBinding('canvas@paint:$mod+drag')] },
        ]);
        // On Linux, $mod → ctrl, so the matcher looks up under 'ctrl+drag'.
        const entries = idx.get('ctrl+drag');
        expect(entries).toBeDefined();
        const resolved = resolveChord(entries!, [{ name: 'canvas' }], 'paint');
        expect(resolved?.entry.actionId).toBe('sampleColor');
        // And NOT under 'meta+drag' (the Super key on Linux).
        expect(idx.get('meta+drag')).toBeUndefined();
    });

    it('resolves on a literal meta+drag event on macOS (Cmd+drag)', async () => {
        const { substituteModInBinding } = await loadModsAs('macos');
        const { buildChordIndex, resolveChord } = await import('../hotkey_resolve');
        const idx = buildChordIndex([
            { actionId: 'sampleColor', bindings: [substituteModInBinding('canvas@paint:$mod+drag')] },
        ]);
        const entries = idx.get('meta+drag');
        expect(entries).toBeDefined();
        const resolved = resolveChord(entries!, [{ name: 'canvas' }], 'paint');
        expect(resolved?.entry.actionId).toBe('sampleColor');
        // And NOT under 'ctrl+drag' (a literal Ctrl press on Mac).
        expect(idx.get('ctrl+drag')).toBeUndefined();
    });
});
