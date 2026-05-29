# Svelte 5 Lessons Learned

## 1. `$effect` does not detect deep proxy mutations

**Problem:** Svelte 5's `$effect` only subscribes to the exact property paths read inside it. Reading `obj.graph` subscribes to reassignment of `graph`, not to mutations of nested properties like `obj.graph.nodes[id].position[0]`.

This means an `$effect` like:

```typescript
$effect(() => {
    brushGraph.graph;       // subscribes to graph being reassigned
    renderer.markDirty();   // never fires on deep mutations
});
```

will NOT re-run when code elsewhere does:

```typescript
node.position[0] = x;  // deep mutation — $effect never sees it
```

**Why it's subtle:** Svelte 5 markets deep reactivity, and it does work — but only when the consumer reads the deep path too. A Svelte component template that renders `{node.position[0]}` will re-render on mutation because it read the deep path. But an `$effect` that only reads the top-level reference doesn't traverse deep enough to subscribe.

**Impact:** The node graph editor appeared to lag by 10+ seconds during drag. In reality, `draw()` was never called — the dirty flag was never set because the `$effect` never fired. The node only visually updated when an unrelated state change (like pointer up) happened to trigger the effect.

**Rule:** When bridging Svelte 5 reactive state to imperative code (Canvas 2D, WebGL, WebGPU), don't rely on `$effect` for high-frequency mutations. Call the imperative invalidation function directly from the event handler:

```typescript
// WRONG: rely on $effect to detect the mutation
brushGraph.moveNode(id, x, y);

// RIGHT: explicitly invalidate after mutation
brushGraph.moveNode(id, x, y);
renderer.markDirty();
```

Keep `$effect` for structural changes (add/remove/connect) where the top-level reference is reassigned and direct invalidation isn't practical.

Subscription granularity reference:

| Code in `$effect`               | Subscribes to                        |
|---------------------------------|--------------------------------------|
| `obj.graph`                     | `graph` being reassigned             |
| `obj.graph.nodes`               | `graph` reassigned OR `nodes` mutated|
| `obj.graph.nodes[id].position`  | all of the above + `position` mutated|
| `obj.graph.nodes[id].position[0]` | all of the above + element 0 set   |

An `$effect` that reads `obj.graph` will fire when you do `obj.graph = newGraph` but NOT when you do `obj.graph.nodes[id].position[0] = 42`. This is by design — Svelte tracks what you read, not what exists.
