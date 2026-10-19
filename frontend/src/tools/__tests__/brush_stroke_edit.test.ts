import { describe, it, expect, vi, beforeEach } from 'vitest';

// Stroke edit's frontend half: a toggle that enters or leaves the engine's
// mode, read back from `engineState`, and the brush tool ending the edit when
// the artist leaves it.

const { fakeInst, toastShow } = vi.hoisted(() => ({
    fakeInst: {
        activeToolId: 'brush',
        engine: null as unknown,
        session: undefined as unknown,
        engineState: null as { strokeEditActive: boolean } | null,
        canvasEl: {},
        toolCursor: null as string | null,
        zoom: 1,
        consumeForeground: () => ({ r: 0, g: 0, b: 0, a: 255 }),
        requestFrame: vi.fn(),
    },
    toastShow: vi.fn(),
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeInst, getActiveInstance: () => fakeInst }));
vi.mock('../../state/toast.svelte', () => ({ toast: { show: toastShow } }));
vi.mock('../../state/brush_graph.svelte', () => ({
    brushGraph: { activeBrush: null, graph: null, fullscreen: false, init: vi.fn() },
}));
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

import { brushTool, toggleStrokeEdit } from '../brush.svelte';

function fakeApi(beginResult: boolean) {
    return {
        strokeEditBegin: vi.fn(async () => beginResult),
        strokeEditEnd: vi.fn(),
        clearOverlay: vi.fn(),
        setBrushBlendMode: vi.fn(),
    };
}

describe('stroke edit toggle', () => {
    beforeEach(() => {
        fakeInst.activeToolId = 'brush';
        fakeInst.engineState = { strokeEditActive: false };
        fakeInst.requestFrame.mockClear();
        toastShow.mockClear();
    });

    it('enters the edit when there is a stroke', async () => {
        const api = fakeApi(true);
        fakeInst.engine = { api };
        await toggleStrokeEdit();
        expect(api.strokeEditBegin).toHaveBeenCalledOnce();
        expect(toastShow).not.toHaveBeenCalled();
        expect(fakeInst.requestFrame).toHaveBeenCalled();
    });

    it('says so when there is no stroke to edit', async () => {
        const api = fakeApi(false);
        fakeInst.engine = { api };
        await toggleStrokeEdit();
        expect(toastShow).toHaveBeenCalledWith('info', 'No stroke to edit');
    });

    it('leaves the edit when the engine reports one', async () => {
        const api = fakeApi(true);
        fakeInst.engine = { api };
        fakeInst.engineState = { strokeEditActive: true };
        await toggleStrokeEdit();
        expect(api.strokeEditEnd).toHaveBeenCalledOnce();
        expect(api.strokeEditBegin).not.toHaveBeenCalled();
    });

    it('does nothing from another tool', async () => {
        const api = fakeApi(true);
        fakeInst.engine = { api };
        fakeInst.activeToolId = 'fill';
        await toggleStrokeEdit();
        expect(api.strokeEditBegin).not.toHaveBeenCalled();
        expect(api.strokeEditEnd).not.toHaveBeenCalled();
    });

    it('ends when the artist leaves the brush tool', () => {
        const api = fakeApi(true);
        fakeInst.session = { api };
        const tool = brushTool.create(fakeInst as never);
        tool.onDeactivate?.();
        expect(api.strokeEditEnd).toHaveBeenCalledOnce();
    });
});
