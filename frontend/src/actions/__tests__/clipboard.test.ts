import { describe, it, expect, vi, beforeEach } from 'vitest';
import { withApi } from '../../engine/testApi';

// Mock every dependency the clipboard actions import *before* importing the
// module under test, so its top-level `import { app }` / `config` / … chain
// resolves to our fakes (same pattern as sample_color.test.ts).
const { engine, fakeApp, fakeConfig, fakeBrushGraph, fakeToast } = vi.hoisted(() => {
    const engine = {
        post: vi.fn(),
        send: vi.fn().mockResolvedValue(null),
    };
    const fakeApp = {
        engine,
        activeLayerId: 42 as number | null,
        canvasEl: {} as HTMLCanvasElement,
        activeToolId: 'brush',
        docW: 100,
        docH: 100,
        onCopyResult: vi.fn(),
        requestFrame: vi.fn(),
        selectLayer: vi.fn(),
        refreshLayerTree: vi.fn().mockResolvedValue(undefined),
    };
    const fakeConfig = { get: vi.fn(() => false) };
    const fakeToast = { show: vi.fn() };
    // Mirrors the real accessor: expanded AND the brush tool active. The
    // distinction is the whole point of the regression below.
    const fakeBrushGraph = {
        isOpen: false,
        selectedNode: null as string | null,
        graph: null as unknown,
        nodeList: [] as unknown[],
        addNode: vi.fn(async () => 'node-1'),
        uploadImageToNode: vi.fn(async () => {}),
        get isVisible() {
            return this.isOpen && fakeApp.activeToolId === 'brush';
        },
    };
    return { engine, fakeApp, fakeConfig, fakeBrushGraph, fakeToast };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../config/store.svelte', () => ({ config: fakeConfig }));
vi.mock('../../state/brush_graph.svelte', () => ({ brushGraph: fakeBrushGraph }));
vi.mock('../../clipboard', () => ({
    copyToSystemClipboard: vi.fn(),
    readImageFromClipboard: vi.fn().mockResolvedValue(null),
    readLayerFromClipboard: vi.fn().mockResolvedValue(null),
}));
vi.mock('../../tools/registry', () => ({ toolRegistry: { get: vi.fn() } }));
vi.mock('../../canvas/coordinates', () => ({ screenToCanvas: vi.fn() }));
vi.mock('../../state/toast.svelte', () => ({ toast: fakeToast }));
vi.mock('./place_smart_object', () => ({ placeSmartObjectFromBlob: vi.fn() }));

import { actions } from '../registry';
import { registerClipboardActions } from '../clipboard';

beforeEach(() => {
    engine.post.mockClear();
    engine.send.mockClear();
    fakeApp.onCopyResult.mockClear();
    fakeApp.activeLayerId = 42;
    fakeConfig.get.mockReturnValue(false);
    // `mockClear` keeps the implementation, so restore the default response
    // rather than letting one test's stub leak into the next.
    engine.send.mockResolvedValue(null);
    fakeApp.activeToolId = 'brush';
    fakeBrushGraph.isOpen = false;
    fakeBrushGraph.selectedNode = null;
    fakeBrushGraph.addNode.mockClear();
    fakeBrushGraph.uploadImageToNode.mockClear();
    fakeBrushGraph.uploadImageToNode.mockResolvedValue(undefined);
    fakeToast.show.mockClear();
});

// Give the fake engine a real typed `api` over its send/post spies.
withApi(engine);

describe('clipboard action registration', () => {
    it('registers copy, cut, paste, and pasteInPlace', () => {
        registerClipboardActions();
        for (const id of ['copy', 'cut', 'paste', 'pasteInPlace']) {
            expect(actions.get(id), id).toBeDefined();
        }
    });
});

// Regression: copy/cut sent `{ layer_id }`, but the Rust handler decodes the
// layer id from the `id` field (the `layer_id()` protocol helper). The
// mismatch surfaced as `bad_payload missing field \`id\`` on Ctrl+C and made
// copy/cut silently fail. These pin the field name so it can't drift back.
describe('copy/cut send the layer id under the `id` field (not `layer_id`)', () => {
    it('copy posts copy_layer_rich with { id }', () => {
        registerClipboardActions();
        actions.get('copy')!.handler({});
        expect(engine.post).toHaveBeenCalledWith('copy_layer_rich', { id: 42 });
    });

    it('cut sends cut with { id }', async () => {
        registerClipboardActions();
        await actions.get('cut')!.handler({});
        expect(engine.send).toHaveBeenCalledWith('cut', { id: 42 });
    });

    it('copy is a no-op when no layer is active', () => {
        registerClipboardActions();
        fakeApp.activeLayerId = null;
        actions.get('copy')!.handler({});
        expect(engine.post).not.toHaveBeenCalled();
    });
});

// Regression: paste-in-place special-cased a mask target onto the committed
// verb, so pasting into a mask entered no transform session and overwrote the
// mask the instant the key was pressed — no preview, no reposition, no cancel.
// The target's kind must not divert the routing: with transform-after-paste on,
// every target floats, and the transform tool is what commits it.
describe('paste-in-place routing', () => {
    it('floats with transform-after-paste on, whatever the target kind', async () => {
        registerClipboardActions();
        fakeConfig.get.mockReturnValue(true);

        await actions.get('pasteInPlace')!.handler({});

        expect(engine.send).toHaveBeenCalledWith('paste_in_place_floating', { id: 42 });
        expect(engine.send).not.toHaveBeenCalledWith('paste_in_place', expect.anything());
    });

    it('commits on arrival only when transform-after-paste is off', async () => {
        registerClipboardActions();
        fakeConfig.get.mockReturnValue(false);
        // The committed verb answers with the id it wrote into.
        engine.send.mockResolvedValue({ id: 42 });

        await actions.get('pasteInPlace')!.handler({});

        expect(engine.send).toHaveBeenCalledWith('paste_in_place', { active_layer_id: 42 });
    });
});

// Regression: the brush builder is the brush tool's `panelComponent`, so it
// only mounts while that tool is active — but `isOpen` survives a tool switch.
// Paste gated on `isOpen`, so expanding the builder once and then switching
// tools left an *invisible* node editor swallowing every paste: the image
// never reached the canvas, and the engine's rejection ("stamp accepts
// AlphaMask only") surfaced only as an unhandled promise rejection after a
// multi-second freeze. `isVisible` is the honest question.
describe('paste is not intercepted by an invisible brush builder', () => {
    beforeEach(async () => {
        const { readImageFromClipboard } = await import('../../clipboard');
        vi.mocked(readImageFromClipboard).mockResolvedValue({
            rgba: new Uint8Array(4),
            width: 1,
            height: 1,
            blob: new Blob(),
        });
        // The canvas paste destructures an id off the response.
        engine.send.mockResolvedValue({ id: 7 });
    });

    it('routes to the canvas when the builder is expanded but the brush tool is not active', async () => {
        fakeBrushGraph.isOpen = true;
        fakeApp.activeToolId = 'eraser';
        registerClipboardActions();

        await actions.get('paste')!.handler({});

        expect(fakeBrushGraph.uploadImageToNode).not.toHaveBeenCalled();
        const kinds = engine.send.mock.calls.map((c) => c[0]);
        expect(kinds.some((k: string) => k.startsWith('paste_image'))).toBe(true);
    });

    it('still routes to the builder when it is genuinely on screen', async () => {
        fakeBrushGraph.isOpen = true;
        fakeApp.activeToolId = 'brush';
        registerClipboardActions();

        await actions.get('paste')!.handler({});

        expect(fakeBrushGraph.uploadImageToNode).toHaveBeenCalledTimes(1);
    });

    it('reports an engine rejection as a toast rather than an unhandled rejection', async () => {
        fakeBrushGraph.isOpen = true;
        fakeApp.activeToolId = 'brush';
        fakeBrushGraph.uploadImageToNode.mockRejectedValue({
            kind: 'engine_error',
            message: 'image-stamp brushes are unsupported — stamp accepts AlphaMask only',
        });
        registerClipboardActions();

        // The assertion is as much that this resolves at all: an un-awaited,
        // un-caught upload rejects outside the handler and takes the paste with
        // it.
        await expect(actions.get('paste')!.handler({})).resolves.toBeUndefined();
        expect(fakeToast.show).toHaveBeenCalledWith(
            'error',
            'image-stamp brushes are unsupported — stamp accepts AlphaMask only',
        );
    });
});
