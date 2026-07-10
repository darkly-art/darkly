import { describe, it, expect, vi } from 'vitest';

// Regression for the in-flight hover race: `pushHoverOverlay` awaits an
// engine round-trip before drawing, so a hover started just before a
// modifier cursor engaged could land its `set_overlay` (and its
// `toolCursor = 'none'`) *after* the engage-time clear — resurrecting a
// ghost dab and stomping the engaged cursor. `brushTool.suspendHover` must
// bump the hover generation so the pending push bails on resume.

const { fakeApp } = vi.hoisted(() => ({
    fakeApp: {
        activeToolId: 'brush',
        toolCursor: null as string | null,
        zoom: 1,
        foreground: { r: 0, g: 0, b: 0, a: 255 },
        requestFrame: vi.fn(),
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
vi.mock('../../ui/BrushOptions.svelte', () => ({ default: {} }));
vi.mock('../../ui/BrushBuilderPanel.svelte', () => ({ default: {} }));

import { brushTool, pushHoverOverlay, type PenPose } from '../brush.svelte';

const POSE: PenPose = { pressure: 1, tiltX: 0, tiltY: 0, twist: 0, tangentialPressure: 0 };

describe('brushTool.suspendHover', () => {
    it('invalidates an in-flight hover push so it cannot land after the clear', async () => {
        let resolvePreview!: (v: unknown) => void;
        const engine = {
            api: {
                refreshBrushCursorPreview: vi.fn(
                    () => new Promise((r) => { resolvePreview = r; })),
                setOverlay: vi.fn(),
                clearOverlay: vi.fn(),
                clearBrushCursorPreviewPose: vi.fn(),
            },
        };
        const ctx = {
            engine,
            canvasEl: {} as HTMLCanvasElement,
            screenToCanvas: (x: number, y: number) => ({ x, y }),
        };

        const pending = pushHoverOverlay(engine as any, POSE, 10, 10);
        brushTool.suspendHover!(ctx as any);
        expect(engine.api.clearOverlay).toHaveBeenCalled();
        expect(engine.api.clearBrushCursorPreviewPose).toHaveBeenCalled();

        resolvePreview({ halfExtent: [4, 4] });
        await pending;
        // The invalidated hover must neither draw nor steal the cursor.
        expect(engine.api.setOverlay).not.toHaveBeenCalled();
        expect(fakeApp.toolCursor).not.toBe('none');
    });
});
