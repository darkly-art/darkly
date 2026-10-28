import { describe, it, expect, vi, beforeEach } from 'vitest';

// Regression: the on-canvas size drag (Shift+drag in the Krita preset) moved
// size 1% per pixel whatever the zoom, so the outline outran the pointer
// several times over and small sizes were as hard to hit as on the bar. The
// drag now grows the outline's on-screen diameter by exactly the pointer's
// travel, in CSS px, so a HiDPI screen must not halve it.
const { fakeApp, fakeBrushGraph, fakeTool } = vi.hoisted(() => {
    const fakeApp = {
        canvasEl: {} as HTMLCanvasElement,
        // `[screen→plane (6), plane→screen (6)]`; plane→screen is in buffer
        // px, so at zoom 1 on a `devicePixelRatio = 2` screen it scales by 2.
        viewMatrices: [0.5, 0, 0, 0, 0.5, 0, 2, 0, 0, 0, 2, 0],
    };
    const size = {
        key: 'brush_settings.size',
        nodeId: 'brush_settings',
        portName: 'size',
        label: 'Size',
        data: { kind: 'scalar', value: 1, min: 0.01, max: 400 },
    };
    const fakeBrushGraph = {
        exposedPorts: [size],
        setExposedPortValueLocal: vi.fn(),
        setExposedPortValue: vi.fn(),
    };
    // A brush whose outline is 5 canvas px across: 10 buffer px, 5 CSS px
    // (half-extent 2.5 canvas px).
    const fakeTool = {
        lastPreview: {
            cx: 40,
            cy: 30,
            info: { halfExtent: [2.5, 2.5], nominalHalfExtent: [2.5, 2.5] },
        } as unknown,
        pushHoverOverlay: vi.fn(() => Promise.resolve()),
        refreshHoverOverlay: vi.fn(),
    };
    return { fakeApp, fakeBrushGraph, fakeTool };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../state/brush_graph.svelte', () => ({ brushGraph: fakeBrushGraph }));
vi.mock('../../tools/brush.svelte', () => ({
    focusedBrushTool: () => fakeTool,
    cursorPose: () => ({ pressure: 1, tiltX: 0, tiltY: 0, twist: 0, tangentialPressure: 0 }),
}));
vi.mock('../../tools/tool_session', () => ({ runHook: (p: unknown) => p }));
vi.stubGlobal('window', { devicePixelRatio: 2 });

import { actions } from '../registry';
import { registerBrushParamActions } from '../brush_params';

registerBrushParamActions();
const drag = actions.get('brushSizeAdjust')!;
const event = {} as PointerEvent;

function lastCommitted(): number {
    const calls = fakeBrushGraph.setExposedPortValue.mock.calls;
    return calls[calls.length - 1][2] as number;
}

beforeEach(() => {
    fakeBrushGraph.setExposedPortValue.mockClear();
    fakeBrushGraph.exposedPorts[0].data.value = 1;
});

describe('brushSizeAdjust', () => {
    it('a drag as wide as the outline doubles the size', () => {
        drag.handler({ x: 40, y: 30 });
        drag.onMove!({ x: 40, y: 30 }, event, 5, 0);
        expect(lastCommitted()).toBeCloseTo(2, 5);
    });

    it('a drag back across most of the outline shrinks it in proportion', () => {
        drag.handler({ x: 40, y: 30 });
        drag.onMove!({ x: 40, y: 30 }, event, -4, 0);
        expect(lastCommitted()).toBeCloseTo(0.2, 5);
    });

    it('never drags below the floor', () => {
        drag.handler({ x: 40, y: 30 });
        drag.onMove!({ x: 40, y: 30 }, event, -500, 0);
        expect(lastCommitted()).toBe(0.01);
    });

    it('holds still until there is an outline to measure, then follows from there', () => {
        const held = fakeTool.lastPreview;
        fakeTool.lastPreview = null;
        drag.handler({ x: 40, y: 30 });
        drag.onMove!({ x: 40, y: 30 }, event, 3, 0);
        expect(fakeBrushGraph.setExposedPortValue).not.toHaveBeenCalled();

        fakeTool.lastPreview = held;
        drag.onMove!({ x: 40, y: 30 }, event, 4, 0);
        drag.onMove!({ x: 40, y: 30 }, event, 9, 0);
        expect(lastCommitted()).toBeCloseTo(2, 5);
    });

    it('lifts a size stored at 0 to the floor, so the drag can grow it', () => {
        fakeBrushGraph.exposedPorts[0].data.value = 0;
        drag.handler({ x: 40, y: 30 });
        expect(lastCommitted()).toBe(0.01);
    });
});
