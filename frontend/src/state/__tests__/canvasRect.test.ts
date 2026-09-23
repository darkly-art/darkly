import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// `app.svelte.ts` imports the compiled wasm bundle at module load; stub it so
// the module graph loads in the node test env without instantiating wasm.
vi.mock('../../../wasm/pkg/darkly_wasm', () => ({
    compute_view_matrices: () => new Float32Array(16),
}));

import { DarklyInstance } from '../app.svelte';
import type { EngineState } from '../../engine/protocol';

/** The canvas-window mirror (`docW`/`docH`/`canvasOriginX`/`canvasOriginY`) has
 *  one runtime writer: the snapshot `render` returns each frame. It replaced
 *  twelve hand-written `syncCanvasRect()` calls, so what these pin is that an
 *  op which moves the window needs only to schedule a frame, and that the boot
 *  seed survives until the first one renders. */

function snapshot(over: Partial<EngineState> = {}): EngineState {
    return {
        frameCount: 1,
        thumbnailVersion: 0,
        dirty: false,
        hasSelection: false,
        canvasOriginX: 0,
        canvasOriginY: 0,
        canvasWidth: 64,
        canvasHeight: 64,
        ...over,
    };
}

/** An engine whose `render` hands back one prepared snapshot. */
function fakeEngine(state: EngineState) {
    return {
        render: vi.fn(() => ({ busy: false, needsMore: false, state })),
        api: {
            pollExportResult: vi.fn(),
        },
    } as unknown as DarklyInstance['engine'];
}

describe('the canvas-window mirror follows the frame snapshot', () => {
    let inst: DarklyInstance;

    beforeEach(() => {
        // Capture rAF callbacks without firing them, so each test drives frames
        // itself rather than racing the scheduler.
        vi.stubGlobal('requestAnimationFrame', () => 1);
        vi.stubGlobal('cancelAnimationFrame', () => {});
        inst = new DarklyInstance();
    });

    afterEach(() => {
        vi.unstubAllGlobals();
    });

    it('keeps the boot seed until the first frame renders', () => {
        // What `createInstance` does: seed the dimensions, then publish the
        // engine. `CanvasView` calls `fitZoom()` in between, so a mirror that
        // waited for the first snapshot would open every tab at the fallback
        // zoom of 1x1.
        inst.docW = 800;
        inst.docH = 600;
        inst.engine = fakeEngine(snapshot());

        expect(inst.docW).toBe(800);
        expect(inst.docH).toBe(600);
        // `fitZoom` is the consumer that makes this matter.
        inst.viewportW = 400;
        inst.viewportH = 400;
        expect(inst.fitZoom()).toBeCloseTo(0.5, 9);
    });

    it('adopts the rendered rect, origin included', () => {
        inst.engine = fakeEngine(
            snapshot({ canvasOriginX: 16, canvasOriginY: 8, canvasWidth: 32, canvasHeight: 24 }),
        );

        inst.renderNow(0);

        expect(inst.canvasOriginX).toBe(16);
        expect(inst.canvasOriginY).toBe(8);
        expect(inst.docW).toBe(32);
        expect(inst.docH).toBe(24);
    });

    it('overwrites the boot seed once a frame lands', () => {
        inst.docW = 800;
        inst.docH = 600;
        inst.engine = fakeEngine(snapshot({ canvasWidth: 64, canvasHeight: 64 }));

        inst.renderNow(0);

        expect([inst.docW, inst.docH]).toEqual([64, 64]);
    });

    it('a crop needs no sync call: the next frame carries the new window', () => {
        inst.engine = fakeEngine(snapshot());
        inst.renderNow(0);
        expect([inst.canvasOriginX, inst.docW]).toEqual([0, 64]);

        // The engine is now cropped. No caller tells the mirror; it just
        // renders again.
        inst.engine = fakeEngine(
            snapshot({ canvasOriginX: 10, canvasOriginY: 10, canvasWidth: 20, canvasHeight: 20 }),
        );
        inst.renderNow(1);

        expect([inst.canvasOriginX, inst.canvasOriginY, inst.docW, inst.docH]).toEqual([
            10, 10, 20, 20,
        ]);
    });

    it('leaves the mirror alone on a busy re-entrant render', () => {
        inst.docW = 800;
        inst.docH = 600;
        inst.engine = {
            render: vi.fn(() => ({ busy: true })),
            api: {},
        } as unknown as DarklyInstance['engine'];

        inst.renderNow(0);

        // A `busy` frame carries no snapshot, so there is nothing to adopt and
        // the seed must not be clobbered with zeroes.
        expect([inst.docW, inst.docH]).toEqual([800, 600]);
    });
});
