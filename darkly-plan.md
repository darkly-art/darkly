# Darkly - The Anti-AI Tool for Artists

Darkly is an art creation tool that runs in the browser.

Its goal is not to compete with Photoshop by focusing on intricate and precise 4K detail. It is to explore wildly and freely, brainstorming with entropy, trying out different compositions, and unlocking secrets latent in your imagination. It solves artist's block by avoiding the dreaded "white canvas".

Darkly stimulates your imagination by obscuring 2D or 3D scenes (or pure entropy) behind "Veils" - layers that blur, deform, or otherwise obfuscate your project.

This frees you from rulers, sharp lines, and your own inner critic by enabling you to play with objects and paint freely and without judgement, behind the veil. 

Examples of Veils might include static noise, smart blur, stain-glass-like crystal, or a noise-and-denoise algorithm.

Accessible warp functions let you morph and shape the underlying art, a process which feels less like painting and more like sculpting.

The goal is to enable artists to better compete with AI by speeding up their ideation process and unearthing artistic expressions unique to them. What will *YOU* see in the entropy? It may surprise you!

## The Implementation

Conceptually this is simple, but implementation will need to be handled carefully.

### Performance

Performance is paramount. Veils especially must be extremely efficient, since they are continually updated in realtime to reflect what's beneath them. 

GPU is the only option.

Every layer along with the entire compositor should run on the GPU. Since we are implementing this in the browser, that means WebGPU.

### Similar Projects

Thankfully there are projects like Graphite, which have done something very similar, and will save us from having to write this in C++.

Graphite is a browser-based vector editor written in Rust + Svelte. It uses WASM and WebGPU to composite its canvas entirely on the GPU. The entire layer hierarchy lives in rust, with functions exported to JS only for interactivity, i.e. transformations, opacity, hiding/showing layers, etc.

### Darkly Phase 1 - Layers + Compositor

I expect the majority of the code will be in interface and user experience; however since the compositor is the most critical piece, it's the first piece we'll write.

My goal for phase 1 is to establish a solid, DRY architecture for layer system. We'll write the 2D layer, layer group, and filter layer types, while leaving accomodation for future types like 3D (e.g. three.JS).

Additionally, the initial version should establish a good system for writing and maintaining veil modules. This will most likely mean a solid bridge between rust and GPU shaders.

UI will be nonexistent; canvas only. We will expose the necessary functions to JS for creating/manipulating layers, and for now will hardcode preinstantiated layers in whatever configuration we need at the moment.

Because performance is so important, even the basic Phase 1 version of Darkly will need to have the proper mechanisms in place for efficient painting and compositing. Similar to Krita, all painting will be done on the CPU, while filter layers and compositing will use the GPU. This will require specific [optimizations](./krita-plus-graphite-perf-lessons.md).

Graphite's vector-specific performance optimizations aren't helpful to us. However, its working implementation of wasm and wgpu will be an extremely useful reference.

WASM64 is pretty much ready, so we'll use that, and not implement any annoying memory optimizations like compression (ick!)

### Phase 1 — First Attempt (Failed)

The first implementation of Phase 1 was functionally complete — tiles, layers, dirty tracking, COW undo, GPU compositing with blend modes, a blur filter, and a full WASM+Svelte frontend. It ran, the demo worked visually, and 15 unit tests passed. But the performance was catastrophic: 100% CPU on all cores, extremely laggy even when idle.

**What went wrong:**

1. **The compositor re-composited everything every frame, unconditionally.** The `requestAnimationFrame` loop called `render()` 60 times per second. Inside `render()`, the full compositing pipeline ran regardless of whether anything had changed — clearing accumulators, running every blend pass, running every blur pass, presenting. There was no concept of "nothing changed, skip." Dirty tracking existed for tile uploads but was completely ignored for compositing.

2. **GPU objects were allocated inside the render loop.** Every frame, for every layer, the compositor created new `wgpu::Buffer` (uniform buffers) and `wgpu::BindGroup` objects, used them once, and dropped them. For the 3-layer demo this meant 4 buffer allocations + 4 bind group allocations + 5 render passes, 60 times per second while idle. All of these objects referenced stable, unchanging data (opacity, blend mode, texture views, sampler) — there was zero reason to recreate them. They should have been created once at layer creation time and reused indefinitely.

3. **The filter system was hardcoded to blur.** `FilterPipelines` was named generically but contained only blur-specific fields (`blur_pipeline`, `blur_uniform_bufs`, `blur_pass_cache`, `cached_radius`). There was no registry, no way to add a second filter type without bolting more fields onto the struct. This violated the project's core requirement: if we implement one filter, we build a proper modular filter system and register that one filter in it.

**Root cause:**

The detailed implementation plan (`PLAN.md`) was written without performance principles. It described *what* to build (tiles, layers, compositor, filters) but not *how the GPU compositor must behave* — no rules about allocation, no dirty gating, no caching. The implementation followed the plan faithfully, which meant faithfully reproducing the omission. References to Graphite and Krita existed in the plan but weren't translated into concrete architectural constraints.

**What we learned:**

- Performance principles must be first-class in the plan, not afterthoughts. We added three: P1 (zero GPU allocation in the render loop), P2 (no work when nothing changed), P3 (cache composite results, re-composite only from the dirty layer upward).
- Every system in the core engine must be properly modular from the start, even if only one variant is implemented. No hacks in the engine; hacks are only acceptable in the UI/demo layer.
- The plan now includes an explicit engineering principle: "The core engine does not need to be 100% implemented, but every part that is implemented must be implemented properly on the first iteration."

The updated `PLAN.md` addresses all three failures. Phase 1 is being re-implemented against the corrected plan.

