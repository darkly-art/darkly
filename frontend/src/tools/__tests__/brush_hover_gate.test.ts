import { describe, it, expect, vi } from 'vitest';

// Regression for the frozen-dab / stomped-cursor bug: `pushHoverOverlay` is
// the choke point every hover entry path funnels through (CanvasView's hover
// dispatch, the shift+drag size scrub, the `[` / `]` hotkey refresh), and the
// size scrub calls it directly, bypassing CanvasView's suppression check.
// While a modifier cursor was engaged (picker dropper, clone crosshair), such
// a call rendered a dab that nothing could later clear (the suppressed hover
// dispatch never refreshes it) and wrote `app.toolCursor = 'none'` over the
// engaged cursor. The suppression gate inside `pushHoverOverlay` must make
// the call a no-op.

const { fakeApp } = vi.hoisted(() => ({
    fakeApp: {
        activeToolId: 'brush',
        engine: undefined,
        session: undefined as unknown,
        canvasEl: null,
        toolCursor: null as string | null,
        zoom: 1,
        foreground: { r: 0, g: 0, b: 0, a: 255 },
        requestFrame: vi.fn(),
    },
}));
// No focused instance in these unit tests: the modifier-cursor machinery
// (real) treats a null focus as "no tool to suspend/restore".
vi.mock('../../state/app.svelte', () => ({ app: fakeApp, getActiveInstance: () => null }));
vi.mock('../../state/brush_graph.svelte', () => ({
    brushGraph: { activeBrush: null, graph: null, fullscreen: false, init: vi.fn() },
}));
vi.mock('../../lib/pressure', () => ({ effectivePressure: () => 1 }));
vi.mock('../../lib/strokeRecorder', () => ({
    strokeRecorder: { beginStroke: vi.fn(), addEvent: vi.fn(), endStroke: vi.fn() },
    currentCanvasDimensions: () => null,
}));
vi.mock('../clone_source_cursor', () => ({
    onCloneStrokeStart: vi.fn(),
    onCloneStrokeMove: vi.fn(),
    onCloneStrokeEnd: vi.fn(),
    clearCloneSourceCursor: vi.fn(),
}));
vi.mock('../../canvas/coordinates', () => ({
    screenToCanvas: (sx: number, sy: number) => ({ x: sx, y: sy }),
    canvasToScreen: (cx: number, cy: number) => ({ x: cx, y: cy }),
}));
vi.mock('../../ui/BrushOptions.svelte', () => ({ default: {} }));
vi.mock('../../ui/BrushBuilderPanel.svelte', () => ({ default: {} }));

import { brushTool, type PenPose } from '../brush.svelte';
import { engageModifierCursor, disengageModifierCursor } from '../modifier_cursor';

const POSE: PenPose = { pressure: 1, tiltX: 0, tiltY: 0, twist: 0, tangentialPressure: 0 };

// `pushHoverOverlay` is now an instance method reading `this.inst.session`; the
// tool is bound to the fake instance, and each test points its session at a
// fresh fake engine.
const tool = brushTool.create(fakeApp as never) as unknown as {
    pushHoverOverlay(pose: PenPose, cx: number, cy: number): Promise<void>;
};

function makeEngine() {
    return {
        api: {
            refreshBrushCursorPreview: vi.fn(async () => ({ halfExtent: [4, 4] })),
            setOverlay: vi.fn(),
            clearOverlay: vi.fn(),
            clearBrushCursorPreviewPose: vi.fn(),
        },
    };
}

describe('pushHoverOverlay while a modifier cursor is engaged', () => {
    it('is a no-op: no preview refresh, no overlay, cursor slot untouched', async () => {
        const engine = makeEngine();
        fakeApp.session = engine;
        engageModifierCursor('picker-like', 'crosshair');
        expect(fakeApp.toolCursor).toBe('crosshair');

        await tool.pushHoverOverlay(POSE, 10, 10);
        expect(engine.api.refreshBrushCursorPreview).not.toHaveBeenCalled();
        expect(engine.api.setOverlay).not.toHaveBeenCalled();
        expect(fakeApp.toolCursor).toBe('crosshair');
        disengageModifierCursor('picker-like');
    });

    it('renders again once the last engager disengages', async () => {
        const engine = makeEngine();
        fakeApp.session = engine;
        engageModifierCursor('picker-like', 'crosshair');
        disengageModifierCursor('picker-like');

        await tool.pushHoverOverlay(POSE, 10, 10);
        expect(engine.api.setOverlay).toHaveBeenCalledTimes(1);
        expect(fakeApp.toolCursor).toBe('none');
    });
});
