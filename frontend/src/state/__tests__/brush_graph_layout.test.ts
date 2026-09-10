import { describe, it, expect, beforeEach, vi } from 'vitest';

// The brush-builder auto-layout is a one-shot per *graph load*. Positions and
// the `needsInitialLayout` guard are scoped to `layoutGeneration` (bumped by
// `beginLayoutGeneration` on every fresh load/reset/import/tab-sync), so a
// newly-loaded brush is always treated as un-laid-out even when it reuses the
// previous brush's node ids, and a stale async layout write for a superseded
// generation is discarded. These tests exercise that at the class level with
// no DOM.

// Controllable stub for the WASM auto-layout round-trip. `autoLayout`'s commit
// path calls only `brushGraphAutoLayout`, so this is the entire engine surface
// these tests need.
const engine = vi.hoisted(() => {
    let resolve: ((v: Record<string, [number, number]>) => void) | null = null;
    const brushGraphAutoLayout = vi.fn(
        () => new Promise<Record<string, [number, number]>>((r) => { resolve = r; }),
    );
    return {
        brushGraphAutoLayout,
        resolveLayout: (v: Record<string, [number, number]>) => resolve!(v),
    };
});

vi.mock('../app.svelte', () => ({
    app: { engine: { api: { brushGraphAutoLayout: engine.brushGraphAutoLayout } } },
}));

import { BrushGraphState, type BrushGraph, type NodeInstance } from '../brush_graph.svelte';

function node(id: string): NodeInstance {
    return { id, type_id: 'test', ports: [] };
}

function graphWith(...ids: string[]): BrushGraph {
    const nodes: Record<string, NodeInstance> = {};
    for (const id of ids) nodes[id] = node(id);
    return { nodes, connections: [] };
}

/** Drive a full auto-layout commit through the stubbed engine, exercising the
 *  real generation-scoped commit path. */
async function commitLayout(state: BrushGraphState, positions: Record<string, [number, number]>) {
    const p = state.autoLayout({});
    engine.resolveLayout(positions);
    await p;
}

let state: BrushGraphState;
beforeEach(() => {
    state = new BrushGraphState();
});

describe('needsInitialLayout', () => {
    it('is true for a freshly-loaded graph', () => {
        state.beginLayoutGeneration();
        state.graph = graphWith('0', '1');
        expect(state.needsInitialLayout).toBe(true);
    });

    it('is false once the current generation has been laid out', async () => {
        state.beginLayoutGeneration();
        state.graph = graphWith('0', '1');
        await commitLayout(state, { 0: [10, 20], 1: [30, 40] });
        expect(state.needsInitialLayout).toBe(false);
    });

    it('is false when a new node awaits placement after layout (addNode invariant)', async () => {
        // Spawning a node updates the graph but must not bump the generation,
        // so the one-shot stays quiet and never relayouts the existing nodes.
        state.beginLayoutGeneration();
        state.graph = graphWith('0');
        await commitLayout(state, { 0: [10, 20] });
        state.graph = graphWith('0', '1');
        expect(state.needsInitialLayout).toBe(false);
    });

    it('is false for an empty graph', () => {
        state.beginLayoutGeneration();
        state.graph = graphWith();
        expect(state.needsInitialLayout).toBe(false);
    });

    it('is true again after a fresh load even when node ids are reused', async () => {
        // Brush A: ids 1..3, laid out.
        state.beginLayoutGeneration();
        state.graph = graphWith('1', '2', '3');
        await commitLayout(state, { 1: [0, 0], 2: [100, 0], 3: [200, 0] });
        expect(state.needsInitialLayout).toBe(false);

        // Brush B reuses ids 1..3. Keying on id-presence would see them all
        // "positioned" and skip layout; keying on generation does not.
        state.beginLayoutGeneration();
        state.graph = graphWith('1', '2', '3', '4', '5');
        expect(state.needsInitialLayout).toBe(true);
    });
});

describe('autoLayout generation guard (regression)', () => {
    // Regression for the graph-identity race: an auto-layout computed for one
    // brush must never land on the graph after a different brush has loaded.
    // Before the fix, `autoLayout` wrote unconditionally, so the stale layout
    // for the reused ids stuck and the new graph never re-laid-out.
    it('discards a layout computed for a superseded generation', async () => {
        // Brush A loads and requests layout.
        state.beginLayoutGeneration();
        state.graph = graphWith('1', '2', '3');
        const aWrite = state.autoLayout({}); // A's engine call is now in flight

        // Before A's layout lands, brush B loads: a fresh generation.
        state.beginLayoutGeneration();
        state.graph = graphWith('1', '2', '3', '4', '5');

        // A's response resolves late, for the now-superseded generation.
        engine.resolveLayout({ 1: [10, 10], 2: [20, 20], 3: [30, 30] });
        await aWrite;

        // The stale positions (for reused ids 1..3) must NOT be applied to B,
        // and B must still be recognized as needing its own layout.
        expect(state.nodePositions).toEqual({});
        expect(state.needsInitialLayout).toBe(true);
    });
});

describe('beginLayoutGeneration invalidates node-id-keyed caches', () => {
    // Image thumbnails are keyed by `image_${nodeId}`, and node ids restart per
    // brush, so a stale bitmap would alias onto a reused id under the next
    // graph. ImageBitmap is GPU-backed and must be closed, not just dropped.
    it('closes and clears image thumbnails on a fresh graph', () => {
        const close = vi.fn();
        state.imageThumbnails.set('image_1', { close } as unknown as ImageBitmap);

        state.beginLayoutGeneration();

        expect(close).toHaveBeenCalledTimes(1);
        expect(state.imageThumbnails.size).toBe(0);
    });
});
