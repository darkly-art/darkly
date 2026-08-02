import { describe, it, expect, vi, beforeEach } from 'vitest';
import { withApi } from '../../engine/testApi';

// Mock `app` and `config` before importing the tool so we never pull in the
// real engine/wasm. The regression guard: the text tool must CREATE the object
// itself on pointer-up — not merely stash a `placement` for a properties panel
// to consume. The old design created text only inside a `$effect` in
// `TextProperties.svelte`, so when that panel was tabbed behind another (thus
// unmounted), the gesture was silently dropped. This test has NO panel mounted
// (node env, no components) and asserts an `add_text` request is issued anyway.
const { fakeApp } = vi.hoisted(() => ({
    fakeApp: {
        engine: null as unknown,
        // `null` so the create-band overlay draw (which needs `window`) is a
        // no-op in the DOM-free test env; the tool doesn't need a canvas to
        // create text.
        canvasEl: null as unknown,
        selectLayer: vi.fn(),
        requestFrame: vi.fn(),
        refreshLayerTree: vi.fn(() => Promise.resolve()),
        activeLayerId: null as number | null,
        activeNode: null as { id: number; type: string } | null,
        foreground: { r: 0, g: 0, b: 0, a: 255 },
        toolCursor: null as string | null,
    },
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeApp, getActiveInstance: () => fakeApp }));
vi.mock('../../config/store.svelte', () => ({ config: { get: () => undefined } }));

import { textTool } from '../text.svelte';

type TextToolLike = {
    onActivate(): Promise<void>;
    onPointerDown(e: PointerEvent, cx: number, cy: number): Promise<void>;
    onPointerMove(e: PointerEvent, cx: number, cy: number): void;
    onPointerUp(e: PointerEvent): void | Promise<void>;
    editing: { layerId: number; objectId: number } | null;
    focusObject: number | null;
};
const tool = textTool.create(fakeApp as never) as unknown as TextToolLike;

const ev = (button = 0) => ({ button }) as unknown as PointerEvent;

function mockEngine() {
    return withApi({
        send: vi.fn((kind: string) => {
            if (kind === 'add_text') return Promise.resolve({ id: 7, object: 3 });
            if (kind === 'add_text_object') return Promise.resolve({ object: 9 });
            if (kind === 'text_objects') return Promise.resolve({ objects: [] });
            if (kind === 'hit_test_vector_object') return Promise.resolve({ object: -1 });
            return Promise.resolve(null);
        }),
        post: vi.fn(),
    });
}

beforeEach(() => {
    fakeApp.selectLayer.mockClear();
    fakeApp.requestFrame.mockClear();
    fakeApp.refreshLayerTree.mockClear();
    fakeApp.engine = mockEngine();
    fakeApp.canvasEl = null;
    fakeApp.activeLayerId = null;
    fakeApp.activeNode = null;
    tool.editing = null;
    tool.focusObject = null;
});

describe('text tool warms the vector renderer on activation', () => {
    it('activating the tool compiles the Vello pipelines ahead of first use', async () => {
        // Selecting the text tool must warm the renderer so the first box doesn't
        // stall on Vello's >1s one-time shader compile.
        fakeApp.canvasEl = {};
        await tool.onActivate();
        const engine = fakeApp.engine as ReturnType<typeof mockEngine>;
        expect(engine.post).toHaveBeenCalledWith('warm_vector_renderer');
    });
});

describe('text tool creates the object itself (panel-independent)', () => {
    it('a drag creates a new text layer with NO properties panel mounted', async () => {
        await tool.onPointerDown(ev(), 20, 20);
        tool.onPointerMove(ev(), 220, 140);
        await tool.onPointerUp(ev());
        const engine = fakeApp.engine as ReturnType<typeof mockEngine>;
        // Observable outcome — not another module's internal wire shape (no
        // `anchor` assertion; that's `createTextFromPending`'s new-layer detail).
        expect(engine.send).toHaveBeenCalledWith(
            'add_text',
            expect.objectContaining({ x: 20, y: 20, box: [200, 120] }),
        );
        expect(fakeApp.selectLayer).toHaveBeenCalledWith(7);
        expect(tool.editing).toEqual({ layerId: 7, objectId: 3 });
        expect(tool.focusObject).toBe(3);
    });

    it('a click on empty canvas creates point text (no box)', async () => {
        await tool.onPointerDown(ev(), 100, 60);
        await tool.onPointerUp(ev());
        const engine = fakeApp.engine as ReturnType<typeof mockEngine>;
        expect(engine.send).toHaveBeenCalledWith(
            'add_text',
            expect.objectContaining({ x: 100, y: 60, box: null }),
        );
    });

    it('a drag on an active vector layer adds an object, no new layer', async () => {
        fakeApp.activeLayerId = 42;
        fakeApp.activeNode = { id: 42, type: 'vector' };
        await tool.onPointerDown(ev(), 20, 20);
        tool.onPointerMove(ev(), 220, 140);
        await tool.onPointerUp(ev());
        const engine = fakeApp.engine as ReturnType<typeof mockEngine>;
        expect(engine.send).toHaveBeenCalledWith(
            'add_text_object',
            expect.objectContaining({ id: 42, x: 20, y: 20, box: [200, 120] }),
        );
        // The vector layer is already active; adding an object must not reselect.
        expect(fakeApp.selectLayer).not.toHaveBeenCalled();
    });
});
