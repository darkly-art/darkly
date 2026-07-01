import { describe, it, expect, vi, beforeEach } from 'vitest';
import { withApi } from '../../engine/testApi';

// Stand-ins for the Svelte-runic state proxy, the GPU overlay builder, and the
// transform-mode strategy, so the gizmo's async lifecycle can be driven without
// the Svelte/GPU/DOM runtime.
const { engine, fakeApp, pushSpy } = vi.hoisted(() => {
    const engine = { post: vi.fn(), send: vi.fn(() => Promise.resolve({})) };
    const fakeApp = { engine, requestFrame: vi.fn(), toolCursor: null as string | null };
    const pushSpy = vi.fn();
    return { engine, fakeApp, pushSpy };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../canvas/gpu_overlay', () => ({
    OverlayBuilder: vi.fn(function (this: Record<string, unknown>) {
        this.line = () => this;
        this.handle = () => this;
        this.push = pushSpy;
    }),
}));
vi.mock('../transform_modes', () => ({
    modeForTag: () => ({
        tag: 0,
        buildOverlay: () => null,
        resolveHandle: () => ({ id: null, cursor: 'default' }),
        beginDrag: () => ({}),
        updateDrag: () => [],
    }),
}));

import { TransformGizmo } from '../transform_gizmo';

function deferred<T>() {
    let resolve!: (v: T) => void;
    const promise = new Promise<T>((r) => (resolve = r));
    return { promise, resolve };
}

const INFO = { origin: [0, 0] as [number, number], w: 10, h: 10, mode: 0, affine: [1, 0, 0, 1, 0, 0] };

beforeEach(() => {
    pushSpy.mockClear();
    engine.post.mockClear();
});

/**
 * Regression: a live void keeps the render loop running every frame, so
 * `onFrame -> gizmo.frame() -> adopt()` almost always has an async
 * `void_transform_info` read in flight. Pressing Enter (commit) clears the
 * gizmo synchronously, but the read that was issued *before* the commit then
 * resolves — and its continuation must NOT rebuild the overlay. If it does, the
 * gizmo handles reappear and the user has to press Enter a second time.
 */
// Give the fake engine a real typed `api` over its send/post spies.
withApi(engine);

describe('transform gizmo commit during in-flight frame read', () => {
    it('does not resurrect the overlay when committed mid-read', async () => {
        const gizmo = new TransformGizmo({} as never);

        let gate = deferred<typeof INFO | null>();
        const binding = {
            read: vi.fn(() => gate.promise),
            update: vi.fn(),
            commit: vi.fn(), // void commit is a no-op
            cancel: vi.fn(),
        };

        // Attach: resolve the first read so the gizmo becomes active.
        const attachP = gizmo.attach(binding as never);
        gate.resolve(INFO);
        await attachP;
        expect(gizmo.active).toBe(true);
        pushSpy.mockClear();
        engine.post.mockClear();

        // A frame fires and issues a read that is still in flight...
        gate = deferred<typeof INFO | null>();
        const frameP = gizmo.frame();

        // ...the user presses Enter mid-read: commit clears the gizmo.
        gizmo.commit();
        expect(gizmo.active).toBe(false);
        expect(engine.post).toHaveBeenCalledWith('clear_overlay');

        // The stale read resolves. The overlay must stay cleared.
        gate.resolve(INFO);
        await frameP;

        expect(gizmo.active).toBe(false);
        expect(pushSpy).not.toHaveBeenCalled();
    });
});
