import { describe, it, expect, vi, beforeEach } from 'vitest';

// Regression tests for the clone set-source cursor's engagement machinery.
// The crosshair is a *transient* sample-mode indicator — engaged only while
// the set-source chord is actually held, exactly like the color picker's
// dropper. Source presence is irrelevant to arming.
//
// Bug 1 ("no dab preview until a source is set"): a clone brush with no
// source used to engage as a persistent prompt — holding the cursor slot and
// suppressing the brush hover indefinitely, so the neutral-grey dab preview
// never rendered on a fresh load. No chord held must mean no engagement.
//
// Bug 2 ("crosshair fails to disappear on ctrl release"): disarming nulled
// `app.toolCursor`, and the canvas fell back to nav's idle cursor — which is
// also 'crosshair' — until the next pointermove. Disarming must restore the
// tool hover immediately (which re-asserts the brush's own cursor).
//
// Binding seed mirrors `colorpicker_cursor.test.ts`: `setCloneSource` on
// `canvas@paint@clone:ctrl+drag`.

const { fakeApp, fakeConfig, fakeBrushGraph, fakeActions, paintTool, heldState } = vi.hoisted(() => {
    const bindings: Record<string, string> = {
        'mouseclicks.setCloneSource': 'canvas@paint@clone:ctrl+drag',
        'mouseclicks.sampleColor': 'canvas@paint:ctrl+drag',
    };
    const api = {
        activeBrushNeedsSource: vi.fn(async () => true),
        cloneSourceAnchored: vi.fn(async () => false),
        setCloneSource: vi.fn(),
        setCloneOverlay: vi.fn(),
        clearCloneOverlay: vi.fn(),
        clearOverlay: vi.fn(),
    };
    const fakeApp = {
        activeToolId: 'brush',
        activeLayerId: null as number | null,
        engine: { api },
        // The focused instance's tool, looked up by the modifier-cursor
        // machinery. `paintTool` is defined later in this scope; the arrow
        // defers the reference until call time (past its TDZ).
        tool: (_id: string) => paintTool,
        canvasEl: {
            getBoundingClientRect: () => ({ left: 0, top: 0, right: 100, bottom: 100 }),
        } as unknown as HTMLCanvasElement | null,
        toolCursor: null as string | null,
        requestFrame: vi.fn(),
    };
    const fakeConfig = {
        get: vi.fn((key: string) => bindings[key]),
        onChange: vi.fn(() => () => undefined),
    };
    const fakeBrushGraph = { activeBrush: 'Clone' as string | null };
    const fakeActions = {
        all: () => [{ id: 'setCloneSource' }, { id: 'sampleColor' }],
        dispatch: vi.fn(),
        get: vi.fn(),
        release: vi.fn(),
    };
    const paintTool = {
        group: 'paint',
        suspendHover: vi.fn(),
        restoreHover: vi.fn(),
    };
    const heldState = { value: '', listeners: [] as Array<() => void> };
    return { fakeApp, fakeConfig, fakeBrushGraph, fakeActions, paintTool, heldState };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp, getActiveInstance: () => fakeApp }));
vi.mock('../../config/store.svelte', () => ({ config: fakeConfig }));
vi.mock('../../state/brush_graph.svelte', () => ({ brushGraph: fakeBrushGraph }));
vi.mock('../registry', () => ({ toolRegistry: { get: () => paintTool } }));
vi.mock('../../actions/registry', () => ({ actions: fakeActions }));
vi.mock('../tool_session', () => ({ toolEngine: () => fakeApp.engine }));
vi.mock('../../canvas/coordinates', () => ({
    screenToCanvas: (sx: number, sy: number) => ({ x: sx, y: sy }),
    canvasToScreen: (cx: number, cy: number) => ({ x: cx, y: cy }),
}));
vi.mock('../../actions/held_mods', () => ({
    heldMods: () => heldState.value,
    onHeldModsChange: (cb: () => void) => {
        heldState.listeners.push(cb);
        return () => undefined;
    },
}));

// The overlay builder reads `window.devicePixelRatio` when pushing marker
// primitives; vitest's node env has no window.
vi.stubGlobal('window', { devicePixelRatio: 1 });

let clone: typeof import('../clone_source_cursor');
let mc: typeof import('../modifier_cursor');

function setHeld(v: string) {
    heldState.value = v;
    for (const cb of heldState.listeners) cb();
}

/** Flush the needs-source engine round-trip cached by the clone module. */
async function primeNeedsSource() {
    clone.tickCloneSourceCursor();
    await new Promise((r) => setTimeout(r, 0));
    clone.tickCloneSourceCursor();
}

beforeEach(async () => {
    vi.resetModules();
    vi.clearAllMocks();
    heldState.value = '';
    heldState.listeners.length = 0;
    fakeApp.toolCursor = null;
    fakeApp.activeLayerId = null;
    fakeBrushGraph.activeBrush = 'Clone';
    const triggers = await import('../../actions/triggers');
    mc = await import('../modifier_cursor');
    clone = await import('../clone_source_cursor');
    triggers.rebuildClickIndex();
    clone.setupCloneSourceModifierTracking();
});

describe('bug 1 regression: no chord means no engagement', () => {
    it('a clone brush with no source and nothing held leaves the hover alone', async () => {
        await primeNeedsSource();
        expect(mc.isToolHoverSuppressed()).toBe(false);
        expect(fakeApp.toolCursor).toBe(null);
        expect(paintTool.suspendHover).not.toHaveBeenCalled();
    });
});

describe('chord engagement (sample-mode indicator)', () => {
    it('holding the set-source chord engages: hover suppressed, crosshair cursor', async () => {
        await primeNeedsSource();
        clone.setCloneSourceAnchor(5, 5);
        setHeld('');
        expect(mc.isToolHoverSuppressed()).toBe(false);

        paintTool.suspendHover.mockClear();
        setHeld('ctrl');
        expect(mc.isToolHoverSuppressed()).toBe(true);
        expect(paintTool.suspendHover).toHaveBeenCalledTimes(1);
        expect(fakeApp.toolCursor).toBe('crosshair');
    });

    it('engages with no source set too — source presence is irrelevant to arming', async () => {
        await primeNeedsSource();
        setHeld('ctrl');
        expect(mc.isToolHoverSuppressed()).toBe(true);
        expect(fakeApp.toolCursor).toBe('crosshair');
    });
});

describe('bug 2 regression: disarming restores the brush hover', () => {
    it('releasing the modifier disengages: cursor released, hover restored at pointer', async () => {
        await primeNeedsSource();
        clone.setCloneSourceAnchor(5, 5);
        mc.trackPointer({ clientX: 30, clientY: 40 });
        setHeld('ctrl');
        expect(mc.isToolHoverSuppressed()).toBe(true);

        paintTool.restoreHover.mockClear();
        setHeld('');
        expect(mc.isToolHoverSuppressed()).toBe(false);
        expect(fakeApp.toolCursor).toBe(null);
        expect(paintTool.restoreHover).toHaveBeenCalledTimes(1);
        const [cx, cy] = paintTool.restoreHover.mock.calls[0];
        expect(cx).toBe(30);
        expect(cy).toBe(40);
    });
});

describe('mid-stroke refusal', () => {
    it('does not first-engage while a pointer is down; engages on release', async () => {
        await primeNeedsSource();
        clone.setCloneSourceAnchor(5, 5);
        setHeld('');
        mc.notePointerDown();
        setHeld('ctrl');
        expect(mc.isToolHoverSuppressed()).toBe(false); // stroke keeps painting
        mc.notePointerUp(); // clone re-evaluates on release
        expect(mc.isToolHoverSuppressed()).toBe(true);
    });
});

describe('set-source pins the active layer', () => {
    it('sends the active layer id with the anchor', () => {
        fakeApp.activeLayerId = 42;
        clone.setCloneSourceAnchor(7, 9);
        expect(fakeApp.engine.api.setCloneSource).toHaveBeenCalledWith({ x: 7, y: 9, layer: 42 });
    });

    it('sends null when no layer is active (same-layer clone)', () => {
        fakeApp.activeLayerId = null;
        clone.setCloneSourceAnchor(7, 9);
        expect(fakeApp.engine.api.setCloneSource).toHaveBeenCalledWith({
            x: 7,
            y: 9,
            layer: null,
        });
    });
});

describe('cloneEngages decision', () => {
    it('requires the chord, needs-source, a paint tool, and no pointer down', () => {
        const { cloneEngages } = clone;
        const chord = new Set(['setCloneSource']);
        const none = new Set<string>();
        expect(cloneEngages(chord, true, true, false)).toBe(true);
        // No chord → no engagement; there is no persistent no-source prompt.
        expect(cloneEngages(none, true, true, false)).toBe(false);
        // Gates: brush doesn't need a source / non-paint tool / pointer down.
        expect(cloneEngages(chord, true, false, false)).toBe(false);
        expect(cloneEngages(chord, false, true, false)).toBe(false);
        expect(cloneEngages(chord, true, true, true)).toBe(false);
    });
});
