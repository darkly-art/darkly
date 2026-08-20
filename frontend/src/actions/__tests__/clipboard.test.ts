import { describe, it, expect, vi, beforeEach } from 'vitest';
import { withApi } from '../../engine/testApi';

// Mock every dependency the clipboard actions import *before* importing the
// module under test, so its top-level `import { app }` / `config` / … chain
// resolves to our fakes (same pattern as sample_color.test.ts).
const { engine, fakeApp, fakeConfig, fakeBrushGraph } = vi.hoisted(() => {
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
    const fakeBrushGraph = { isOpen: false };
    return { engine, fakeApp, fakeConfig, fakeBrushGraph };
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
