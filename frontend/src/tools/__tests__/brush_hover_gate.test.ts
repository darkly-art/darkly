import { describe, it, expect, vi } from 'vitest';

// Regression for the frozen-dab / stomped-cursor bug: `pushHoverOverlay` is
// the choke point every hover entry path funnels through (CanvasView's hover
// dispatch, the shift+drag size scrub, the `[` / `]` hotkey refresh), and the
// size scrub calls it directly — bypassing CanvasView's suppression check.
// While a modifier cursor was engaged (picker dropper, clone crosshair), such
// a call rendered a dab that nothing could later clear (the suppressed hover
// dispatch never refreshes it) and wrote `app.toolCursor = 'none'` over the
// engaged cursor. The suppression gate inside `pushHoverOverlay` must make
// the call a no-op.

const { fakeApp, paintTool } = vi.hoisted(() => ({
    fakeApp: {
        activeToolId: 'brush',
        engine: undefined,
        canvasEl: null,
        toolCursor: null as string | null,
        zoom: 1,
        foreground: { r: 0, g: 0, b: 0, a: 255 },
        requestFrame: vi.fn(),
    },
    paintTool: {
        group: 'paint',
        suspendHover: vi.fn(),
        restoreHover: vi.fn(),
    },
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
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
vi.mock('../registry', () => ({ toolRegistry: { get: () => paintTool } }));
vi.mock('../tool_session', () => ({ toolEngine: () => null }));
vi.mock('../../canvas/coordinates', () => ({
    screenToCanvas: (sx: number, sy: number) => ({ x: sx, y: sy }),
    canvasToScreen: (cx: number, cy: number) => ({ x: cx, y: cy }),
}));
vi.mock('../../ui/BrushOptions.svelte', () => ({ default: {} }));
vi.mock('../../ui/BrushBuilderPanel.svelte', () => ({ default: {} }));

import { pushHoverOverlay, type PenPose } from '../brush.svelte';
import { engageModifierCursor, disengageModifierCursor } from '../modifier_cursor';

const POSE: PenPose = { pressure: 1, tiltX: 0, tiltY: 0, twist: 0, tangentialPressure: 0 };

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
        engageModifierCursor('picker-like', 'crosshair');
        expect(fakeApp.toolCursor).toBe('crosshair');

        await pushHoverOverlay(engine as any, POSE, 10, 10);
        expect(engine.api.refreshBrushCursorPreview).not.toHaveBeenCalled();
        expect(engine.api.setOverlay).not.toHaveBeenCalled();
        expect(fakeApp.toolCursor).toBe('crosshair');
        disengageModifierCursor('picker-like');
    });

    it('renders again once the last engager disengages', async () => {
        const engine = makeEngine();
        engageModifierCursor('picker-like', 'crosshair');
        disengageModifierCursor('picker-like');

        await pushHoverOverlay(engine as any, POSE, 10, 10);
        expect(engine.api.setOverlay).toHaveBeenCalledTimes(1);
        expect(fakeApp.toolCursor).toBe('none');
    });
});
