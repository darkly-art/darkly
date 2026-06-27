<div align="center">

<a href="https://darkly.art"><img src="https://github.com/user-attachments/assets/62115b89-ab63-453c-93ce-a513e500fad7" alt="Darkly — a GPU-native paint engine in Rust" width="600"></a>

**A GPU-native paint engine for Rust: brushes, layers, blend modes, masks, selections, and undo, on [wgpu](https://wgpu.rs) (WebGPU). Runs on the desktop or in the browser via WebAssembly.**

[![crates.io](https://img.shields.io/crates/v/darkly?style=for-the-badge&logo=rust&label=crates.io&labelColor=black&color=9500ff)](https://crates.io/crates/darkly)
[![docs.rs](https://img.shields.io/docsrs/darkly?style=for-the-badge&logo=docsdotrs&labelColor=black&color=8100ff)](https://docs.rs/darkly)
[![GitHub](https://img.shields.io/github/stars/darkly-art/darkly?style=for-the-badge&logo=github&label=GitHub&labelColor=black&color=6c00ff)](https://github.com/darkly-art/darkly)
[![License](https://img.shields.io/crates/l/darkly?style=for-the-badge&label=License&labelColor=black&color=5800ff)](https://github.com/darkly-art/darkly/blob/master/LICENSE)
[![Discord](https://img.shields.io/discord/1495886270780539021?style=for-the-badge&logo=discord&logoColor=white&label=Discord&labelColor=black&color=4400ff)](https://discord.gg/kFz2FGhbpu)

</div>

---

`darkly` is the Rust core of [Darkly](https://darkly.art), a GPU-native paint program for artists. It provides all the essential features: pressure-sensitive brush strokes, layers and groups, blend modes, masks, selections, and undo/redo, all on the GPU.

To render, it needs a wgpu surface (a native window or an HTML canvas). It then builds the layer tree and the GPU textures behind it, and composites frames on demand. The [Darkly web app](https://demo.darkly.art) is this exact crate behind a thin WebAssembly bridge.

This crate can be used to power the backend for any drawing app, whiteboard, texture editor, etc. It includes only the engine: no frontend. The [Darkly](https://darkly.art) app is the reference frontend.

## Features

- **Node-graph brush engine.** Every brush (round, watercolor, smudge, liquify, clone) compiles to a single GPU pipeline.
- **Layers and groups.** A real layer tree with blend modes and per-layer opacity.
- **Masks and selections.** Rectangle, ellipse, lasso, polygon, and magic-wand modifiers.
- **First-class undo/redo.** Every edit is an undoable, serializable document operation.
- **GPU-native compositing.** wgpu/WebGPU pipelines, with the document as the source of truth and the compositor derived from it.
- **One engine, native or web.** The same code drives desktop builds and the in-browser editor.

## Example

Create a layer, paint a brush stroke across it, and composite a frame. The wgpu `Instance` and `Surface` come from a native window or an HTML canvas:

```rust
use darkly::engine::{DarklyEngine, types::StrokeOp};
use darkly::gpu::context::GpuContext;

async fn run(instance: wgpu::Instance, surface: wgpu::Surface<'static>) {
    // GpuContext owns the wgpu device/queue + canvas surface; the engine owns
    // the authoritative document, session state, and undo stack.
    let gpu = GpuContext::new(instance, surface, wgpu::Limits::default(), 1920, 1080).await;
    let mut engine = DarklyEngine::new(gpu, 1920, 1080);

    // Add a raster layer and paint a horizontal brush stroke across it.
    let layer = engine.add_raster_layer(None);
    engine.begin_stroke(layer);
    for i in 0..20 {
        engine.stroke_to(StrokeOp::BrushStroke {
            x: i as f32 * 96.0,
            y: 540.0,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: i as f64 * 16.0,
            cr: 0.1, cg: 0.6, cb: 1.0, ca: 1.0, // brush color, linear RGBA
        });
    }
    engine.end_stroke();

    // Composite the document to the surface. Call once per frame; the arg is the
    // animation clock in seconds (drives time-based veils). Returns whether it drew.
    engine.render(0.0);

    // Undo is first-class: this rolls the stroke back off the document.
    engine.undo();
}
```

> **Pre-1.0:** Darkly is in beta and this crate tracks the app's needs, so the API will change between releases. Pin an exact version if you depend on it.

## Project

- **Try it now:** [demo.darkly.art](https://demo.darkly.art)
- **Documentation:** [darkly.art/docs](https://darkly.art/docs)
- **Community:** [Discord](https://discord.gg/kFz2FGhbpu)
- **Issues and source:** [github.com/darkly-art/darkly](https://github.com/darkly-art/darkly)

## Acknowledgements

A special thank you to **[Nick Cameron](https://github.com/nrc)** for graciously gifting us the `darkly` crate on crates.io. 💜