import { describe, it, expect, vi, beforeEach } from 'vitest';
import { withApi } from '../../engine/testApi';

// Stand-ins for the Svelte state proxy + GPU overlay builder so the gizmo's
// lifecycle runs without the Svelte/GPU/DOM runtime. The transform-mode
// registry and projective math are NOT mocked — this exercises the real
// mode-switching path.
const { fakeApp } = vi.hoisted(() => {
    const engine = { post: vi.fn(), send: vi.fn(() => Promise.resolve({})) };
    return { fakeApp: { engine, requestFrame: vi.fn(), toolCursor: null as string | null } };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../canvas/gpu_overlay', () => ({
    OverlayBuilder: vi.fn(function (this: Record<string, unknown>) {
        this.line = () => this;
        this.handle = () => this;
        this.hitTest = () => null;
        this.push = () => {};
    }),
}));

import { TransformGizmo } from '../transform_gizmo';
import { mat3Apply, type Mat3 } from '../transform_projective';
import { SessionEngine } from '../tool_session';

const IDENTITY_MAT3: Mat3 = [1, 0, 0, 0, 1, 0, 0, 0, 1];

// The gizmo pushes/clears its overlay through a live session accessor supplied
// at construction; establish one over the fake engine so it resolves and the
// overlay (and thus the bbox that `isInside` reads) is built.
withApi(fakeApp.engine);
let session: SessionEngine | null = null;
const sess = () => session;
beforeEach(() => {
    session = new SessionEngine(fakeApp.engine as never);
});

function makeBinding(live = false) {
    return {
        live,
        read: vi.fn(() =>
            Promise.resolve({
                origin: [0, 0] as [number, number],
                w: 100,
                h: 80,
                mode: 0,
                matrix: [...IDENTITY_MAT3] as Mat3,
            }),
        ),
        update: vi.fn(),
        commit: vi.fn(),
        cancel: vi.fn(),
    };
}

describe('mode switching via setMode', () => {
    it('a fresh attach reports mode 0 (free transform), not perspective', async () => {
        const gizmo = new TransformGizmo({} as never, sess);
        await gizmo.attach(makeBinding() as never);
        expect(gizmo.active).toBe(true);
        // The bug was right-click force-entering perspective; default must be free.
        expect(gizmo.modeTag).toBe(0);
    });

    it('setMode(1) pushes a mode-1 homography reproducing the bbox', async () => {
        const gizmo = new TransformGizmo({} as never, sess);
        const binding = makeBinding();
        await gizmo.attach(binding as never);

        gizmo.setMode(1);

        expect(binding.update).toHaveBeenCalledTimes(1);
        const [matrix, modeTag] = binding.update.mock.calls[0];
        expect(modeTag).toBe(1);
        expect((matrix as Mat3).length).toBe(9);
        const near = (p: [number, number], x: number, y: number) => {
            expect(p[0]).toBeCloseTo(x, 2);
            expect(p[1]).toBeCloseTo(y, 2);
        };
        near(mat3Apply(matrix as Mat3, 0, 0), 0, 0);
        near(mat3Apply(matrix as Mat3, 100, 0), 100, 0);
        near(mat3Apply(matrix as Mat3, 100, 80), 100, 80);
        expect(gizmo.modeTag).toBe(1);
    });

    it('is two-way: setMode(1) then setMode(0) returns to free transform', async () => {
        const gizmo = new TransformGizmo({} as never, sess);
        const binding = makeBinding();
        await gizmo.attach(binding as never);

        gizmo.setMode(1);
        gizmo.setMode(0);

        expect(binding.update).toHaveBeenCalledTimes(2);
        const [matrix0, modeTag0] = binding.update.mock.calls[1];
        expect(modeTag0).toBe(0);
        // The basic seedMatrix is a least-squares affine fit; for an unrotated
        // identity quad it reproduces the rect exactly.
        const near = (p: [number, number], x: number, y: number) => {
            expect(p[0]).toBeCloseTo(x, 2);
            expect(p[1]).toBeCloseTo(y, 2);
        };
        near(mat3Apply(matrix0 as Mat3, 100, 80), 100, 80);
        expect(gizmo.modeTag).toBe(0);
    });

    it('setMode is a no-op when already in the target mode', async () => {
        const gizmo = new TransformGizmo({} as never, sess);
        const binding = makeBinding();
        await gizmo.attach(binding as never);
        gizmo.setMode(1);
        gizmo.setMode(1);
        expect(binding.update).toHaveBeenCalledTimes(1);
    });

    it('availableModes respects binding.live + liveCapable', async () => {
        // Both basic and perspective are liveCapable today, so a live binding
        // offers the same set as a one-shot binding.
        const live = new TransformGizmo({} as never, sess);
        await live.attach(makeBinding(true) as never);
        expect(live.availableModes().map((m) => m.tag)).toEqual([0, 1]);
        expect(live.availableModes().map((m) => m.label)).toEqual([
            'Free transform',
            'Perspective',
        ]);

        const oneShot = new TransformGizmo({} as never, sess);
        await oneShot.attach(makeBinding(false) as never);
        expect(oneShot.availableModes().map((m) => m.tag)).toEqual([0, 1]);
    });

    it('isInside reflects the current bbox', async () => {
        const gizmo = new TransformGizmo({} as never, sess);
        await gizmo.attach(makeBinding() as never);
        expect(gizmo.isInside(50, 40)).toBe(true);
        expect(gizmo.isInside(1000, 1000)).toBe(false);
    });
});
