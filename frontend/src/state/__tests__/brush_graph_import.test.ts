import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { app, DarklyInstance, setActiveInstance } from '../app.svelte';
import { BrushGraphState, type BrushGraph } from '../brush_graph.svelte';

// Regression: a null-on-success handler (`brush_graph_import_yaml`) crosses the
// wasm boundary as a nullish value. The importer must read that as success and
// never dereference it as an `{ error }` envelope: the crash was
// `Cannot read properties of undefined (reading 'error')` when the boundary
// delivered `undefined` instead of the declared `null`.

const emptyGraph: BrushGraph = { nodes: {}, connections: [] };

// Minimal engine stub covering `importYaml`'s success refresh chain.
function fakeEngine(importResult: unknown) {
    return {
        api: {
            brushGraphImportYaml: async () => importResult,
            brushGraphActive: async () => emptyGraph,
            brushExposedPorts: async () => [],
            brushActiveCapabilities: async () => ({}),
            brushTopologyVersion: async () => ({ value: 0 }),
        },
    } as unknown as NonNullable<typeof app.engine>;
}

let state: BrushGraphState;

beforeEach(() => {
    setActiveInstance(new DarklyInstance());
    state = new BrushGraphState();
});
afterEach(() => {
    setActiveInstance(null);
});

describe('importYaml success sentinel', () => {
    it('treats a `null` result as success', async () => {
        app.engine = fakeEngine(null);
        expect(await state.importYaml('nodes: {}')).toBeNull();
        expect(state.error).toBeNull();
    });

    it('treats an `undefined` result as success (boundary null → undefined)', async () => {
        // Without the nullish guard this threw reading `.error` off `undefined`.
        app.engine = fakeEngine(undefined);
        expect(await state.importYaml('nodes: {}')).toBeNull();
        expect(state.error).toBeNull();
    });

    it('surfaces an `{ error }` envelope as the failure string', async () => {
        app.engine = fakeEngine({ error: 'bad node' });
        expect(await state.importYaml('nonsense')).toBe('bad node');
        expect(state.error).toBe('bad node');
    });
});
