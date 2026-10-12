import { describe, it, expect, vi, beforeEach } from 'vitest';

// Regression: a pointermove carries every device-rate sample the browser
// folded into it, and the brush must forward each one. Forwarding only the
// dispatched event drops most of a tablet's samples on platforms that
// coalesce motion to one event per frame, so fast strokes turn into long
// straight segments.

const { fakeInst, recorder, cloneMove } = vi.hoisted(() => ({
    fakeInst: {
        activeToolId: 'brush',
        session: undefined as unknown,
        canvasEl: {},
        toolCursor: null as string | null,
        zoom: 1,
        consumeForeground: () => ({ r: 255, g: 0, b: 0, a: 255 }),
        requestFrame: () => {},
    },
    recorder: { beginStroke: vi.fn(), addEvent: vi.fn(), endStroke: vi.fn() },
    cloneMove: vi.fn(),
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeInst, getActiveInstance: () => null }));
vi.mock('../../state/brush_graph.svelte', () => ({
    brushGraph: { activeBrush: null, graph: null, fullscreen: false, init: vi.fn() },
}));
vi.mock('../../lib/pressure', () => ({ effectivePressure: () => 1 }));
vi.mock('../../lib/strokeRecorder', () => ({
    strokeRecorder: recorder,
    currentCanvasDimensions: () => null,
}));
vi.mock('../clone_source_cursor', () => ({
    onCloneStrokeStart: vi.fn(),
    onCloneStrokeMove: cloneMove,
    onCloneStrokeEnd: vi.fn(),
    clearCloneSourceCursor: vi.fn(),
}));
vi.mock('../../canvas/coordinates', () => ({
    screenToCanvas: (sx: number, sy: number) => ({ x: sx, y: sy }),
    canvasToScreen: (cx: number, cy: number) => ({ x: cx, y: cy }),
}));
vi.mock('../../ui/BrushOptions.svelte', () => ({ default: {} }));
vi.mock('../../ui/BrushBuilderPanel.svelte', () => ({ default: {} }));

import { brushTool } from '../brush.svelte';

const tool = brushTool.create(fakeInst as never) as unknown as {
    onPointerMove(e: PointerEvent, cx: number, cy: number): void;
};

type Sample = { clientX: number; clientY: number; timeStamp: number };

function move(last: Sample, coalesced?: Sample[]): PointerEvent {
    const base = (s: Sample) => ({ ...s, buttons: 1, pointerType: 'pen', pressure: 1 });
    const e: Record<string, unknown> = base(last);
    if (coalesced) e.getCoalescedEvents = () => coalesced.map(base);
    return e as unknown as PointerEvent;
}

function strokeTo() {
    const spy = vi.fn();
    fakeInst.session = { api: { strokeTo: spy } };
    return spy;
}

const sent = (spy: ReturnType<typeof vi.fn>) =>
    spy.mock.calls.map(([req]) => [req.op.x, req.op.y, req.op.time_ms]);

describe('brush forwards coalesced pointer samples', () => {
    beforeEach(() => {
        recorder.addEvent.mockClear();
        cloneMove.mockClear();
    });

    it('sends every coalesced sample, oldest first', () => {
        const spy = strokeTo();
        const samples = [10, 20, 30].map((v) => ({ clientX: v, clientY: 5, timeStamp: v }));
        tool.onPointerMove(move(samples[2], samples), 30, 5);
        expect(sent(spy)).toEqual([
            [10, 5, 10],
            [20, 5, 20],
            [30, 5, 30],
        ]);
        expect(recorder.addEvent).toHaveBeenCalledTimes(3);
        expect(cloneMove).toHaveBeenCalledTimes(1);
        expect(cloneMove).toHaveBeenCalledWith(30, 5);
    });

    it('sends the dispatched event when the coalesced list is empty', () => {
        const spy = strokeTo();
        tool.onPointerMove(move({ clientX: 7, clientY: 8, timeStamp: 4 }, []), 70, 80);
        expect(sent(spy)).toEqual([[70, 80, 4]]);
    });

    it('sends the dispatched event when the browser has no coalesced events', () => {
        const spy = strokeTo();
        tool.onPointerMove(move({ clientX: 7, clientY: 8, timeStamp: 4 }), 70, 80);
        expect(sent(spy)).toEqual([[70, 80, 4]]);
    });
});
