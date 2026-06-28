import { describe, it, expect, vi } from 'vitest';

// Stand-ins for the Svelte state proxy + GPU overlay builder so the gizmo's
// lifecycle runs without the Svelte/GPU/DOM runtime. The transform-mode
// registry and projective math are NOT mocked — this exercises the real
// perspective entry path.
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

const IDENTITY_MAT3: Mat3 = [1, 0, 0, 0, 1, 0, 0, 0, 1];

function makeBinding() {
    return {
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

describe('right-click → perspective entry', () => {
    it('pushes a mode-1 homography through the binding that reproduces the bbox', async () => {
        const gizmo = new TransformGizmo({} as never);
        const binding = makeBinding();
        await gizmo.attach(binding as never);
        expect(gizmo.active).toBe(true);

        gizmo.enterPerspective();

        // The binding received a perspective update (modeTag 1) with a full
        // 9-float homography reproducing the current (identity) shape.
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
    });

    it('isInside reflects the current bbox', async () => {
        const gizmo = new TransformGizmo({} as never);
        await gizmo.attach(makeBinding() as never);
        expect(gizmo.isInside(50, 40)).toBe(true);
        expect(gizmo.isInside(1000, 1000)).toBe(false);
    });

    it('is one-way: a second call while already perspective is a no-op', async () => {
        const gizmo = new TransformGizmo({} as never);
        const binding = makeBinding();
        await gizmo.attach(binding as never);
        gizmo.enterPerspective();
        // The mode is now perspective locally; re-entering must not re-push.
        gizmo.enterPerspective();
        expect(binding.update).toHaveBeenCalledTimes(1);
    });
});
