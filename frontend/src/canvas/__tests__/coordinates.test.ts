import { describe, it, expect, vi, beforeEach } from 'vitest';

// Minimal stand-in for the Svelte-runic `app` state. `coordinates.ts` now
// consumes `app.viewMatrices` — the screen↔plane affines built in Rust — so the
// test supplies KNOWN matrices and exercises only the matvec + DPR/rect handling
// (the construction math is covered by the Rust `gpu::view` tests). Packing:
// `[screen→plane (6), plane→screen (6)]`, each row-major `[m00,m01,m02,m10,m11,m12]`.
const { fakeApp } = vi.hoisted(() => {
    const fakeApp: { viewMatrices: Float32Array } = {
        viewMatrices: new Float32Array([1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 0]),
    };
    return { fakeApp };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));

import { screenToCanvas, canvasToScreen } from '../coordinates';

function fakeCanvas(cssW = 100, cssH = 100, dpr = 1, left = 0, top = 0): HTMLCanvasElement {
    return {
        width: cssW * dpr,
        height: cssH * dpr,
        getBoundingClientRect: () => ({ left, top, width: cssW, height: cssH }),
    } as unknown as HTMLCanvasElement;
}

/**
 * Build a `viewMatrices` pair for a pure scale+translate plane mapping:
 *   buffer = plane * s + t  (and its inverse). Mirrors the Rust packing so the
 *   test can assert round-trips and origin/scale behavior without re-deriving
 *   the full pan/zoom/rotate transform (that lives in Rust).
 */
function scaleTranslateMatrices(s: number, tx: number, ty: number): Float32Array {
    // plane→screen (buffer): bx = s*px + tx.  screen→plane: px = (bx - tx)/s.
    return new Float32Array([
        1 / s, 0, -tx / s, 0, 1 / s, -ty / s, // screen→plane
        s, 0, tx, 0, s, ty,                    // plane→screen
    ]);
}

beforeEach(() => {
    vi.stubGlobal('window', { devicePixelRatio: 1 });
    fakeApp.viewMatrices = new Float32Array([1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 0]);
});

describe('screen/canvas coordinate transforms (matvec over app.viewMatrices)', () => {
    it('identity matrices map screen pixels straight through to plane', () => {
        const el = fakeCanvas();
        const p = screenToCanvas(40, 60, el);
        expect(p.x).toBeCloseTo(40, 6);
        expect(p.y).toBeCloseTo(60, 6);
    });

    it('round-trips canvas→screen→canvas under scale + translate', () => {
        fakeApp.viewMatrices = scaleTranslateMatrices(2, 30, -20);
        const el = fakeCanvas();
        const plane = { x: 55, y: 65 };
        const screen = canvasToScreen(plane.x, plane.y, el);
        const back = screenToCanvas(screen.x, screen.y, el);
        expect(back.x).toBeCloseTo(plane.x, 4);
        expect(back.y).toBeCloseTo(plane.y, 4);
    });

    it('applies the screen→plane translate column (origin-like offset)', () => {
        // A pure +t translate in the plane→screen direction shows up as a -t/s
        // shift on screen→plane — verify the translate column is honored.
        fakeApp.viewMatrices = scaleTranslateMatrices(1, 40, 15);
        const el = fakeCanvas();
        const p = screenToCanvas(50, 50, el);
        expect(p.x).toBeCloseTo(10, 4); // (50 - 40)/1
        expect(p.y).toBeCloseTo(35, 4); // (50 - 15)/1
    });

    it('honors DPR and the element bounding-rect offset on screen→plane', () => {
        vi.stubGlobal('window', { devicePixelRatio: 2 });
        fakeApp.viewMatrices = new Float32Array([1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 0]);
        const el = fakeCanvas(100, 100, 2, 10, 5);
        // buffer = (client - rect) * dpr = (60-10, 45-5)*2 = (100, 80)
        const p = screenToCanvas(60, 45, el);
        expect(p.x).toBeCloseTo(100, 4);
        expect(p.y).toBeCloseTo(80, 4);
    });

    it('canvasToScreen divides buffer output by DPR back to CSS', () => {
        vi.stubGlobal('window', { devicePixelRatio: 2 });
        // plane→screen identity → buffer == plane; /dpr → CSS.
        fakeApp.viewMatrices = new Float32Array([1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 0]);
        const el = fakeCanvas(100, 100, 2);
        const s = canvasToScreen(100, 80, el);
        expect(s.x).toBeCloseTo(50, 4);
        expect(s.y).toBeCloseTo(40, 4);
    });
});
