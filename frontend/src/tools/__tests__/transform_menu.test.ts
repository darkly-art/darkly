import { describe, it, expect, vi, beforeEach } from 'vitest';
import { withApi } from '../../engine/testApi';

// A live void binding drives this test: the transform tool routes a 'live'
// capability to `voidTransformBinding`, whose read/update cross the engine
// transport (mocked here). The gizmo + mode registry are real. The typed `api`
// forwards to the `send`/`post` spies, so bare values come back off `send` and
// `update_void_transform` crosses as `{ id, transform: { mode, data } }`.
const { fakeApp } = vi.hoisted(() => {
    const engine = {
        post: vi.fn(),
        send: vi.fn((kind: string) => {
            switch (kind) {
                case 'layer_transform_capability':
                    return Promise.resolve('live');
                case 'void_transform_info':
                    return Promise.resolve({
                        ox: 0,
                        oy: 0,
                        w: 100,
                        h: 80,
                        mode: 0,
                        matrix: [1, 0, 0, 0, 1, 0],
                    });
                default:
                    return Promise.resolve({});
            }
        }),
    };
    return {
        fakeApp: {
            engine,
            session: null as unknown,
            canvasEl: {} as unknown,
            requestFrame: vi.fn(),
            toolCursor: null as string | null,
            activeLayerId: 7 as number | null,
            transformModeMenu: null as { x: number; y: number } | null,
        },
    };
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

import { transformTool } from '../transform.svelte';
import { SessionEngine } from '../tool_session';
import { mat3Apply, type Mat3 } from '../transform_projective';

withApi(fakeApp.engine);

/** The transform tool exposes mode queries as instance methods (not on the base
 *  `Tool` type); widen the created tool so the menu-facing methods are visible. */
type TransformToolLike = {
    onActivate?(): void;
    onPointerDown(e: PointerEvent, cx: number, cy: number): Promise<void> | void;
    onPointerUp?(e?: PointerEvent): void;
    availableModes(): { tag: number; label: string }[];
    activeModeTag(): number | null;
    setMode(tag: number): void;
    flip(axis: 'h' | 'v'): void;
};
const tool = transformTool.create(fakeApp as never) as unknown as TransformToolLike;

/** Drain all pending microtasks (one macrotask hop). `onActivate` fires a
 *  detached `void activate()` whose async `read()`→bbox rebuild outlives the
 *  awaited pointer handlers; production rebuilds the bbox every frame via
 *  `gizmo.frame()` long before any right-click, so the test must likewise let
 *  activation settle before exercising bbox-dependent behavior. */
const settle = () => new Promise((r) => setTimeout(r, 0));

async function activeTool() {
    tool.onActivate?.();
    // A left-click on an inactive gizmo runs activate() then claims the pointer.
    await tool.onPointerDown!({ button: 0 } as PointerEvent, 50, 40);
    tool.onPointerUp?.({} as PointerEvent);
    await settle();
}

describe('transform right-click mode menu', () => {
    beforeEach(() => {
        fakeApp.transformModeMenu = null;
        fakeApp.engine.post.mockClear();
        // Tool code reaches the engine through the instance's live session;
        // begin one over the fake engine.
        (fakeApp.session as SessionEngine | null)?.kill();
        fakeApp.session = new SessionEngine(fakeApp.engine as never);
    });

    it('right-click inside the bbox opens the mode menu at the cursor', async () => {
        await activeTool();
        await tool.onPointerDown!(
            { button: 2, clientX: 12, clientY: 34 } as PointerEvent,
            50,
            40,
        );
        expect(fakeApp.transformModeMenu).toEqual({ x: 12, y: 34 });
    });

    it('right-click outside the bbox does not open the menu', async () => {
        await activeTool();
        await tool.onPointerDown!(
            { button: 2, clientX: 12, clientY: 34 } as PointerEvent,
            5000,
            5000,
        );
        expect(fakeApp.transformModeMenu).toBeNull();
    });

    it('exposes the available modes and the active one to the menu', async () => {
        await activeTool();
        expect(tool.availableModes().map((m) => m.label)).toEqual(['Free transform', 'Perspective']);
        expect(tool.activeModeTag()).toBe(0);
    });

    it('selecting a mode pushes that mode tag through the void binding', async () => {
        await activeTool();
        tool.setMode(1);
        expect(tool.activeModeTag()).toBe(1);
        const persp = fakeApp.engine.post.mock.calls.find(
            (c) => c[0] === 'update_void_transform' && c[1]?.transform?.mode === 'Perspective',
        );
        expect(persp).toBeTruthy();
        // Perspective sends the full 9-float homography.
        expect(persp![1].transform.data.length).toBe(9);
    });
});

/** The matrix payload of the most recent transform push through the binding. */
function lastPushedMatrix(): { mode: string; data: number[] } {
    const calls = fakeApp.engine.post.mock.calls.filter((c) => c[0] === 'update_void_transform');
    expect(calls.length).toBeGreaterThan(0);
    return calls[calls.length - 1][1].transform;
}

describe('transform menu flips', () => {
    beforeEach(async () => {
        fakeApp.engine.post.mockClear();
        (fakeApp.session as SessionEngine | null)?.kill();
        fakeApp.session = new SessionEngine(fakeApp.engine as never);
        await activeTool();
        fakeApp.engine.post.mockClear();
    });

    // The fake void is a 100×80 rect at the origin under the identity matrix,
    // so a mirror about its centre is exactly `x → 100 - x` / `y → 80 - y`.
    it('flips horizontally about the source centre', () => {
        tool.flip('h');
        expect(lastPushedMatrix()).toEqual({ mode: 'Basic', data: [-1, 0, 100, 0, 1, 0] });
    });

    it('flips vertically about the source centre', () => {
        tool.flip('v');
        expect(lastPushedMatrix()).toEqual({ mode: 'Basic', data: [1, 0, 0, 0, -1, 80] });
    });

    it('flipping both axes is a 180° turn, and flipping twice restores the original', () => {
        tool.flip('h');
        tool.flip('v');
        expect(lastPushedMatrix().data).toEqual([-1, 0, 100, 0, -1, 80]);
        tool.flip('h');
        tool.flip('v');
        expect(lastPushedMatrix().data).toEqual([1, 0, 0, 0, 1, 0]);
    });

    it('mirrors a perspective quad in place, swapping its left and right edges', () => {
        tool.setMode(1);
        const before = lastPushedMatrix().data as Mat3;
        tool.flip('h');
        const after = lastPushedMatrix();
        expect(after.mode).toBe('Perspective');
        // Source TL now lands where TR did, and vice versa: same quad, mirrored
        // content.
        expect(mat3Apply(after.data as Mat3, 0, 0)).toEqual(mat3Apply(before, 100, 0));
        expect(mat3Apply(after.data as Mat3, 100, 80)).toEqual(mat3Apply(before, 0, 80));
    });
});
