import { describe, it, expect, vi } from 'vitest';

// Regression for the in-flight hover race: `pushHoverOverlay` awaits an
// engine round-trip before drawing, so a hover started just before a
// modifier cursor engaged could land its `set_overlay` (and its
// `toolCursor = 'none'`) *after* the engage-time clear, resurrecting a
// ghost dab and stomping the engaged cursor. `brushTool.suspendHover` must
// bump the hover generation so the pending push bails on resume.

const { fakeApp } = vi.hoisted(() => ({
    fakeApp: {
        activeToolId: 'brush',
        session: undefined as unknown,
        toolCursor: null as string | null,
        zoom: 1,
        foreground: { r: 0, g: 0, b: 0, a: 255 },
        requestFrame: vi.fn(),
    },
}));
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
vi.mock('../../ui/BrushOptions.svelte', () => ({ default: {} }));
vi.mock('../../ui/BrushBuilderPanel.svelte', () => ({ default: {} }));

import { brushTool, type PenPose } from '../brush.svelte';

const POSE: PenPose = { pressure: 1, tiltX: 0, tiltY: 0, twist: 0, tangentialPressure: 0 };

// The brush tool is bound to the fake instance; it reads its engine off
// `inst.session` and its hover state (`pushHoverOverlay` / `suspendHover`) is
// per-instance.
const tool = brushTool.create(fakeApp as never) as unknown as {
    pushHoverOverlay(pose: PenPose, cx: number, cy: number): Promise<void>;
    suspendHover(): void;
};

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
        fakeApp.session = engine;

        const pending = tool.pushHoverOverlay(POSE, 10, 10);
        tool.suspendHover();
        expect(engine.api.clearOverlay).toHaveBeenCalled();
        expect(engine.api.clearBrushCursorPreviewPose).toHaveBeenCalled();

        resolvePreview({ halfExtent: [4, 4] });
        await pending;
        // The invalidated hover must neither draw nor steal the cursor.
        expect(engine.api.setOverlay).not.toHaveBeenCalled();
        expect(fakeApp.toolCursor).not.toBe('none');
    });
});
