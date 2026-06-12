import { describe, it, expect, vi, beforeEach } from 'vitest';

// Minimal stand-in for the Svelte-runic `app` state so the coordinate math
// can be tested without the Svelte runtime. Mutated per-test.
const { fakeApp } = vi.hoisted(() => ({
    fakeApp: {
        rotation: 0,
        zoom: 1,
        panX: 0,
        panY: 0,
        mirrorH: false,
        docW: 100,
        docH: 100,
        canvasOriginX: 0,
        canvasOriginY: 0,
    },
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));

import { screenToCanvas, canvasToScreen } from '../coordinates';

function fakeCanvas(cssW = 100, cssH = 100, dpr = 1): HTMLCanvasElement {
    return {
        width: cssW * dpr,
        height: cssH * dpr,
        getBoundingClientRect: () => ({ left: 0, top: 0, width: cssW, height: cssH }),
    } as unknown as HTMLCanvasElement;
}

beforeEach(() => {
    vi.stubGlobal('window', { devicePixelRatio: 1 });
    Object.assign(fakeApp, {
        rotation: 0,
        zoom: 1,
        panX: 0,
        panY: 0,
        mirrorH: false,
        docW: 100,
        docH: 100,
        canvasOriginX: 0,
        canvasOriginY: 0,
    });
});

describe('screen/canvas coordinate transforms with canvas_origin', () => {
    it('round-trips a point with a non-zero canvas_origin', () => {
        fakeApp.canvasOriginX = 30;
        fakeApp.canvasOriginY = 20;
        const el = fakeCanvas();
        const plane = { x: 55, y: 65 };
        const screen = canvasToScreen(plane.x, plane.y, el);
        const back = screenToCanvas(screen.x, screen.y, el);
        expect(back.x).toBeCloseTo(plane.x, 4);
        expect(back.y).toBeCloseTo(plane.y, 4);
    });

    it('screenToCanvas returns plane coords — shifting the origin shifts the result', () => {
        const el = fakeCanvas();
        const at0 = screenToCanvas(50, 50, el);
        fakeApp.canvasOriginX = 40;
        fakeApp.canvasOriginY = 15;
        const shifted = screenToCanvas(50, 50, el);
        expect(shifted.x - at0.x).toBeCloseTo(40, 4);
        expect(shifted.y - at0.y).toBeCloseTo(15, 4);
    });

    it('round-trips under zoom + rotation + pan + non-zero origin', () => {
        Object.assign(fakeApp, {
            zoom: 1.7,
            rotation: 0.4,
            panX: 12,
            panY: -8,
            canvasOriginX: 25,
            canvasOriginY: -10,
        });
        const el = fakeCanvas(120, 90, 1);
        const plane = { x: 70, y: 40 };
        const screen = canvasToScreen(plane.x, plane.y, el);
        const back = screenToCanvas(screen.x, screen.y, el);
        expect(back.x).toBeCloseTo(plane.x, 3);
        expect(back.y).toBeCloseTo(plane.y, 3);
    });

    it('round-trips with a mirrored view and non-zero origin', () => {
        fakeApp.mirrorH = true;
        fakeApp.canvasOriginX = 18;
        const el = fakeCanvas();
        const plane = { x: 33, y: 77 };
        const screen = canvasToScreen(plane.x, plane.y, el);
        const back = screenToCanvas(screen.x, screen.y, el);
        expect(back.x).toBeCloseTo(plane.x, 4);
        expect(back.y).toBeCloseTo(plane.y, 4);
    });
});
