import { describe, it, expect, beforeEach } from 'vitest';
import { BrushGraphState, type BrushGraph, type NodeInstance } from '../brush_graph.svelte';

// Auto-layout is a one-shot for a *freshly loaded* graph (all nodes
// unpositioned). It must never fire while a single new node is awaiting
// placement mid-`addNode`, or it would relayout — and thus move — every
// already-positioned node. `needsInitialLayout` encodes that distinction.

function node(id: number): NodeInstance {
    return { id, type_id: 'test', ports: [], params: [] };
}

function graphWith(...ids: number[]): BrushGraph {
    const nodes: Record<string, NodeInstance> = {};
    for (const id of ids) nodes[String(id)] = node(id);
    return { nodes, connections: [], next_id: Math.max(0, ...ids) + 1 };
}

let state: BrushGraphState;
beforeEach(() => {
    state = new BrushGraphState();
});

describe('needsInitialLayout', () => {
    it('is true for a fresh graph where no node has a position', () => {
        state.graph = graphWith(0, 1);
        state.nodePositions = {};
        expect(state.needsInitialLayout).toBe(true);
    });

    it('is false when a node is already placed and a new one awaits placement', () => {
        // Regression: this is the transient mid-`addNode` state. Spawning a
        // node must not trigger a full relayout that moves the existing one.
        state.graph = graphWith(0, 1);
        state.nodePositions = { 0: [10, 20] };
        expect(state.needsInitialLayout).toBe(false);
    });

    it('is false when every node is positioned', () => {
        state.graph = graphWith(0, 1);
        state.nodePositions = { 0: [10, 20], 1: [30, 40] };
        expect(state.needsInitialLayout).toBe(false);
    });

    it('is false for an empty graph', () => {
        state.graph = graphWith();
        state.nodePositions = {};
        expect(state.needsInitialLayout).toBe(false);
    });
});
