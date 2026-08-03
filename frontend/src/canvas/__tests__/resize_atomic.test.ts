import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// `app.svelte.ts` imports the compiled wasm bundle at module load; stub it so
// the module graph loads in the node test env without instantiating wasm.
vi.mock('../../../wasm/pkg/darkly_wasm', () => ({
    compute_view_matrices: () => new Float32Array(16),
}));

import { DarklyInstance } from '../../state/app.svelte';

/** A minimal engine whose `render` returns `busy: true` so `runFrame` bails
 *  immediately after the render call. That isolates the property under test —
 *  "is the frame driven synchronously?" — from the heavy post-render pipeline
 *  (tool hooks, readback polls) which this test does not exercise. */
function fakeEngine(rec: string[]) {
    return {
        render: vi.fn(() => {
            rec.push('render');
            return { busy: true };
        }),
        api: {
            resize: vi.fn(() => rec.push('resize')),
            setViewTransform: vi.fn(() => rec.push('setViewTransform')),
        },
    } as unknown as DarklyInstance['engine'];
}

describe('atomic canvas resize scheduling', () => {
    let rafCbs: FrameRequestCallback[];
    let cancelled: number[];

    beforeEach(() => {
        rafCbs = [];
        cancelled = [];
        // Capture rAF callbacks WITHOUT firing them, so a deferred render is
        // observable as "render has not been called yet".
        vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
            rafCbs.push(cb);
            return rafCbs.length; // handle = 1-based index
        });
        vi.stubGlobal('cancelAnimationFrame', (h: number) => {
            cancelled.push(h);
        });
        vi.stubGlobal('performance', { now: () => 0 });
    });

    afterEach(() => vi.unstubAllGlobals());

    it('renderNow drives the frame synchronously, in the same task', () => {
        const rec: string[] = [];
        const inst = new DarklyInstance();
        inst.engine = fakeEngine(rec);

        inst.renderNow();

        // Rendered in THIS task — not deferred to a captured rAF. This is the
        // atomicity the Firefox resize fix depends on: the present runs in the
        // same JS task as the canvas resize, never straddling a browser turn.
        expect(rec).toEqual(['render']);
        expect(rafCbs).toHaveLength(0);
    });

    it('requestFrame defers to a rAF — the split the old resize path relied on', () => {
        const rec: string[] = [];
        const inst = new DarklyInstance();
        inst.engine = fakeEngine(rec);

        inst.requestFrame();

        // Nothing rendered synchronously; it waits for the browser turn. Driving
        // the resize present this way is exactly what let a present straddle the
        // swapchain reconfigure and freeze Firefox.
        expect(rec).toEqual([]);
        expect(rafCbs).toHaveLength(1);
        rafCbs[0](0);
        expect(rec).toEqual(['render']);
    });

    it('renderNow cancels a pending rAF and clears the pending flag', () => {
        const rec: string[] = [];
        const inst = new DarklyInstance();
        inst.engine = fakeEngine(rec);

        inst.requestFrame(); // schedules an rAF (handle = 1)
        expect(rafCbs).toHaveLength(1);

        inst.renderNow(); // must cancel the pending rAF and render now
        expect(cancelled).toEqual([1]);
        expect(rec).toEqual(['render']);

        // The pending flag must not be left stuck — a later requestFrame must
        // still schedule (a stuck flag would silently freeze all rendering).
        inst.requestFrame();
        expect(rafCbs).toHaveLength(2);
    });
});
