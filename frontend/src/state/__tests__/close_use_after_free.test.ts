import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Engine } from '../../engine/protocol';
import { DarklyInstance } from '../app.svelte';

// Regression: closing a tab (or a failed `.darkly` open, which closes the
// just-opened tab) used to free the WASM handle while leaving `instance.engine`
// non-null. The self-rescheduling `requestFrame` rAF loop would then call
// `engine.render` on the freed handle, and the wasm-bindgen wrapper throws
// "Attempt to use a moved value": an uncaught error surfacing as
// `DarklyHandle.render → Engine.render → app.svelte.ts`'s rAF callback.
//
// `dispose()` must null the engine reference so the loop's `if (!engine) return`
// guard short-circuits a frame that was already queued when the tab closed.
describe('closing a tab mid-frame does not render on a freed handle', () => {
    let rafCb: FrameRequestCallback | null;

    beforeEach(() => {
        rafCb = null;
        // Capture the scheduled callback instead of running it, so the test
        // decides exactly when the queued frame fires (after dispose).
        vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
            rafCb = cb;
            return 1;
        });
    });

    it('a rAF queued before dispose() renders nothing and does not throw', () => {
        // Model the wasm-bindgen "moved value" throw: once the handle is freed,
        // any later `render` throws, exactly what the real `DarklyHandle` does.
        let freed = false;
        const render = vi.fn(() => {
            if (freed) throw new Error('Attempt to use a moved value');
            return { busy: false, needsMore: false };
        });
        const free = vi.fn(() => {
            freed = true;
        });

        const inst = new DarklyInstance();
        inst.engine = {
            render,
            free,
            api: {
                pollCopyResult: vi.fn(),
                pollExportResult: vi.fn(),
                pollSaveResult: vi.fn(),
            },
        } as unknown as Engine;

        // A frame is in flight: the open-success path calls requestFrame(), and
        // the loop self-reschedules, so one is essentially always queued.
        inst.requestFrame();
        expect(rafCb).toBeInstanceOf(Function);

        // The load fails / the user closes the tab: the instance is disposed,
        // which frees the handle.
        inst.dispose();
        expect(inst.engine).toBeNull();

        // The already-queued rAF now fires. It must bail on the null-engine
        // guard rather than render on the freed handle.
        expect(() => rafCb!(16)).not.toThrow();
        expect(render).not.toHaveBeenCalled();
        expect(free).toHaveBeenCalledTimes(1);
    });
});
