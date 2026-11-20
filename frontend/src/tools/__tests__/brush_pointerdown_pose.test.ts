import { describe, it, expect, vi, beforeEach } from 'vitest';

// The engine's stroke continues the hover the pen arrived with: its first
// dab measures its direction from the engine's last hover pose, which the
// stroke takes when it starts. Pointer-down must therefore leave that pose
// alone when the pointer going down is the one that hovered (clearing it
// would start every stroke from no direction), and clear it when another
// pointer goes down (a touch while a mouse rests over the canvas), whose
// stroke must not continue someone else's hover.

const { fakeInst } = vi.hoisted(() => ({
    fakeInst: {
        activeToolId: 'brush',
        activeLayerId: 1,
        session: undefined as unknown,
        canvasEl: {},
        toolCursor: null as string | null,
        zoom: 1,
        consumeForeground: () => ({ r: 0, g: 0, b: 0, a: 255 }),
        requestFrame: () => {},
    },
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeInst, getActiveInstance: () => null }));
vi.mock('../../state/brush_graph.svelte', () => ({
    brushGraph: {
        activeBrush: null,
        graph: null,
        fullscreen: false,
        init: vi.fn(),
        settleRecentCycle: vi.fn(),
    },
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

import { brushTool } from '../brush.svelte';

const tool = brushTool.create(fakeInst as never) as unknown as {
    onPointerMove(e: PointerEvent, cx: number, cy: number): void;
    onPointerDown(e: PointerEvent, cx: number, cy: number): void;
};

function pointer(pointerId: number, buttons: number): PointerEvent {
    return { pointerId, buttons, pointerType: 'pen', pressure: 1, timeStamp: 0 } as unknown as PointerEvent;
}

function engine() {
    const api = {
        refreshBrushCursorPreview: vi.fn(() => new Promise(() => {})),
        setOverlay: vi.fn(),
        clearOverlay: vi.fn(),
        clearBrushCursorPreviewPose: vi.fn(),
        beginStroke: vi.fn(() => Promise.resolve()),
        strokeTo: vi.fn(),
    };
    fakeInst.session = { api };
    return api;
}

describe('brush pointer-down and the hover pose', () => {
    beforeEach(() => vi.clearAllMocks());

    it('keeps the pose when the pointer that hovered goes down', () => {
        const api = engine();
        tool.onPointerMove(pointer(1, 0), 10, 10);
        tool.onPointerDown(pointer(1, 1), 10, 10);
        expect(api.strokeTo).toHaveBeenCalled();
        expect(api.clearBrushCursorPreviewPose).not.toHaveBeenCalled();
    });

    it('clears the pose before the stroke when another pointer goes down', () => {
        const api = engine();
        tool.onPointerMove(pointer(1, 0), 10, 10);
        tool.onPointerDown(pointer(2, 1), 40, 40);
        expect(api.clearBrushCursorPreviewPose).toHaveBeenCalled();
        expect(api.clearBrushCursorPreviewPose.mock.invocationCallOrder[0]).toBeLessThan(
            api.strokeTo.mock.invocationCallOrder[0],
        );
    });
});
