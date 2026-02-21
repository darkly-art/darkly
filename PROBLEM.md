# Performance Problem: Unconditional Per-Frame GPU Recompositing

## Symptom
100% CPU on all cores, extremely laggy interface — even when idle.

## Root Cause

The render loop calls `compositor.render()` on every `requestAnimationFrame` (~60fps). Inside `render()`, the compositor does the **full compositing pipeline every frame regardless of whether anything changed**:

1. Creates a command encoder
2. Clears the accumulator texture to transparent
3. For **every** visible raster layer:
   - Allocates a **new** `wgpu::Buffer` (uniform buffer) — `compositor.rs:254`
   - Allocates a **new** `wgpu::BindGroup` — `compositor.rs:260`
   - Runs a fullscreen composite render pass
4. For **every** filter layer:
   - Allocates **two** new uniform buffers — `filter.rs:149` (one per blur direction)
   - Allocates **two** new bind groups — `filter.rs:155`
   - Runs two fullscreen blur render passes
5. Allocates another bind group for the present pass — `compositor.rs:338`
6. Submits the entire command buffer to the GPU

For the 3-layer demo (bg raster + blur filter + paint raster), this means **per frame**:
- 4 uniform buffer allocations
- 4 bind group allocations
- 5 render passes (clear + 2 composite + 2 blur)
- 1 command encoder + submission

All of this happens **60 times per second while the user does nothing**.

## Why dirty tracking doesn't help

Dirty tracking exists (`doc.dirty` / `DirtyRegion`) but is only used for tile uploads (step 1 — lines 190-203). The compositing pipeline (steps 2-6) runs unconditionally — there is no check for "has anything actually changed since last composite?"

## The two-part flaw

1. **No early-exit**: The compositor has no concept of "nothing changed, skip compositing and re-present the last frame."

2. **GPU objects created in the render loop instead of up-front**: Uniform buffers, bind groups, and the command encoder are allocated inside `render()` every frame. But none of the inputs to these objects change frame-to-frame:
   - **Uniform buffers** hold `{opacity, blend_mode}` — set once at layer creation, rarely mutated
   - **Bind groups** reference accumulator views, layer views, sampler, and uniform buffer — all are stable objects created once in `Compositor::new()` or `ensure_layer_texture()` and never recreated
   - **Blur uniform buffers** hold `{radius, direction}` — fixed at filter creation
   - **Blur bind groups** reference the same stable accumulator views + sampler
   - **Present bind groups** reference a stable accumulator view + sampler

   There is **zero reason** to allocate any of these per-frame. They should all be created once (at layer/filter creation time or compositor init) and reused. The only time they'd need recreation is on canvas resize or layer structure changes. `queue.write_buffer()` handles the rare case where a uniform value changes.

## Comparison with Graphite

Graphite's render loop also uses `requestAnimationFrame`, but it gates actual GPU work on change detection:
- Image data hashing to skip texture uploads when content is unchanged
- The rAF loop exists for animation/polling, but rendering only happens when the backend detects a change
- Frontend subscribes to `UpdateDocumentArtwork` messages — only updates DOM when the backend says something changed
