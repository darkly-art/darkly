import { describe, it, expect, beforeEach, vi } from 'vitest';

// Per-node author comments are ordinary graph state. `setNodeCommentLocal` is
// the pure, engine-free local-feedback path the node editor calls while the
// artist types; the engine commit is exercised end-to-end by the Rust
// round-trip test. These class-level tests need no DOM and no real engine.

vi.mock('../app.svelte', () => ({ app: { engine: null } }));

import { BrushGraphState, type BrushGraph, type NodeInstance } from '../brush_graph.svelte';

function node(id: string): NodeInstance {
    return { id, type_id: 'test', ports: [] };
}

function graphWith(...ids: string[]): BrushGraph {
    const nodes: Record<string, NodeInstance> = {};
    for (const id of ids) nodes[id] = node(id);
    return { nodes, connections: [] };
}

let state: BrushGraphState;
beforeEach(() => {
    state = new BrushGraphState();
});

describe('setNodeCommentLocal', () => {
    it('updates the target node comment', () => {
        state.graph = graphWith('shape', 'stamp');
        state.setNodeCommentLocal('shape', 'words of wisdom');
        expect(state.graph.nodes['shape'].comment).toBe('words of wisdom');
        // Siblings are untouched.
        expect(state.graph.nodes['stamp'].comment).toBeUndefined();
    });

    it('clears the comment with an empty string', () => {
        state.graph = graphWith('shape');
        state.setNodeCommentLocal('shape', 'note');
        state.setNodeCommentLocal('shape', '');
        expect(state.graph.nodes['shape'].comment).toBe('');
    });

    it('is a no-op for an unknown node or missing graph', () => {
        expect(() => state.setNodeCommentLocal('ghost', 'x')).not.toThrow();
        state.graph = graphWith('shape');
        state.setNodeCommentLocal('ghost', 'x');
        expect(state.graph.nodes['shape'].comment).toBeUndefined();
    });
});
