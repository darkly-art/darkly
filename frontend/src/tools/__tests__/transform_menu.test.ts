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
import {
    transformModes,
    transformActiveMode,
    setTransformMode,
} from '../transform.svelte';

withApi(fakeApp.engine);

const ctx = { canvasEl: {} as HTMLCanvasElement } as never;

async function activeTool() {
    transformTool.onActivate?.(ctx);
    // A left-click on an inactive gizmo runs activate() then claims the pointer.
    await transformTool.onPointerDown(ctx, { button: 0 } as PointerEvent, 50, 40);
    transformTool.onPointerUp?.(ctx, {} as PointerEvent);
}

describe('transform right-click mode menu', () => {
    beforeEach(() => {
        fakeApp.transformModeMenu = null;
        fakeApp.engine.post.mockClear();
    });

    it('right-click inside the bbox opens the mode menu at the cursor', async () => {
        await activeTool();
        await transformTool.onPointerDown(
            ctx,
            { button: 2, clientX: 12, clientY: 34 } as PointerEvent,
            50,
            40,
        );
        expect(fakeApp.transformModeMenu).toEqual({ x: 12, y: 34 });
    });

    it('right-click outside the bbox does not open the menu', async () => {
        await activeTool();
        await transformTool.onPointerDown(
            ctx,
            { button: 2, clientX: 12, clientY: 34 } as PointerEvent,
            5000,
            5000,
        );
        expect(fakeApp.transformModeMenu).toBeNull();
    });

    it('exposes the available modes and the active one to the menu', async () => {
        await activeTool();
        expect(transformModes().map((m) => m.label)).toEqual(['Free transform', 'Perspective']);
        expect(transformActiveMode()).toBe(0);
    });

    it('selecting a mode pushes that mode tag through the void binding', async () => {
        await activeTool();
        setTransformMode(1);
        expect(transformActiveMode()).toBe(1);
        const persp = fakeApp.engine.post.mock.calls.find(
            (c) => c[0] === 'update_void_transform' && c[1]?.transform?.mode === 'Perspective',
        );
        expect(persp).toBeTruthy();
        // Perspective sends the full 9-float homography.
        expect(persp![1].transform.data.length).toBe(9);
    });
});
