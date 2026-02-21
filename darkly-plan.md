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