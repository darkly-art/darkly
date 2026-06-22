import { describe, it, expect, vi, beforeEach } from 'vitest';

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
    fakeApp.activeLayerId = 42;
});

describe('clipboard action registration', () => {
    it('registers copy, cut, paste, and pasteInPlace under the edit category', () => {
        registerClipboardActions();
        for (const id of ['copy', 'cut', 'paste', 'pasteInPlace']) {
            const action = actions.get(id);
            expect(action, id).toBeDefined();
            expect(action!.category).toBe('edit');
        }
    });
});

// Regression: copy/cut sent `{ layer_id }`, but the Rust handler decodes the
// layer id from the `id` field (the `layer_id()` protocol helper). The
// mismatch surfaced as `bad_payload missing field \`id\`` on Ctrl+C and made
// copy/cut silently fail. These pin the field name so it can't drift back.
describe('copy/cut send the layer id under the `id` field (not `layer_id`)', () => {
    it('copy sends copy_layer_rich with { id }', async () => {
        registerClipboardActions();
        await actions.get('copy')!.handler({});
        expect(engine.send).toHaveBeenCalledWith('copy_layer_rich', { id: 42 });
    });

    it('cut sends cut with { id }', async () => {
        registerClipboardActions();
        await actions.get('cut')!.handler({});
        expect(engine.send).toHaveBeenCalledWith('cut', { id: 42 });
    });

    it('copy is a no-op when no layer is active', async () => {
        registerClipboardActions();
        fakeApp.activeLayerId = null;
        await actions.get('copy')!.handler({});
        expect(engine.send).not.toHaveBeenCalled();
    });
});
