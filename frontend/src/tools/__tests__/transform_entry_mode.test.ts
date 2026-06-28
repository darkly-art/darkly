import { describe, it, expect, vi, beforeEach } from 'vitest';

// The transform cluster has two members sharing one gizmo: `transform` (free)
// and `transform_perspective` (enters perspective). This pins each variant's
// entry mode. Engine transport is mocked; a 'live' void (mode 0) is the target.
const { fakeApp } = vi.hoisted(() => {
    const engine = {
        post: vi.fn(),
        send: vi.fn((kind: string) => {
            switch (kind) {
                case 'layer_transform_capability':
                    return Promise.resolve({ value: 'live' });
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

import { transformTool, transformPerspectiveTool, transformActiveMode } from '../transform.svelte';

const ctx = { canvasEl: {} as HTMLCanvasElement } as never;

// onActivate fires `void activate()` (not awaited), so let its microtasks drain.
async function activate(tool: typeof transformTool) {
    tool.onActivate?.(ctx);
    await new Promise((r) => setTimeout(r, 0));
}

describe('transform cluster entry modes', () => {
    beforeEach(() => {
        fakeApp.engine.post.mockClear();
    });

    it('the free transform tool engages in mode 0 (adopts the document default)', async () => {
        await activate(transformTool);
        expect(transformActiveMode()).toBe(0);
        // Free entry never force-pushes a mode (no downgrade of existing state).
        const pushed = fakeApp.engine.post.mock.calls.find(
            (c) => c[0] === 'update_void_transform',
        );
        expect(pushed).toBeFalsy();
    });

    it('the perspective tool engages directly in perspective (mode 1)', async () => {
        await activate(transformPerspectiveTool);
        expect(transformActiveMode()).toBe(1);
        // Entering perspective seeds + pushes the 9-float homography.
        const persp = fakeApp.engine.post.mock.calls.find(
            (c) => c[0] === 'update_void_transform' && c[1]?.mode_tag === 1,
        );
        expect(persp).toBeTruthy();
        expect(persp![1].payload.length).toBe(9);
    });
});
