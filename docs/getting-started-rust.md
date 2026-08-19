# Getting Started — Darkly in Rust

This is the **page 1** guide to using Darkly's core as a Rust library. The crate
[`crates/darkly/`](../crates/darkly/) is **platform-agnostic**: the document
model, brush engine, GPU compositor, undo, and `DarklyEngine` itself carry zero
platform dependencies. The browser is just one consumer (via the WASM bridge in
[`frontend/wasm/`](../frontend/wasm/)); a native host, a headless test, or a
future Tauri backend all drive the same engine the same way.

This guide covers the three things you need: **acquire a GPU context**,
**construct the engine**, and **drive it**.

## The two layers

- **`DarklyEngine`** — the whole editor: document, session state, compositor.
  Directly constructable with `DarklyEngine::new(gpu, w, h)`. This is what tests
  and embedders talk to today.
- **`EngineHost`** *(in progress)* — a thin wrapper that owns the engine plus a
  frame-driven task scheduler, exposing one cross-platform entry pair:
  `host.tick(time)` (frame path) and `host.pump()` (between-frame path). The host
  is the *integration seam*, not a mandatory wrapper — `DarklyEngine` stays
  directly constructable so unit tests never need it. See
  [the engine-host rework plan](#about-enginehost) below for status.

> Whether you embed `DarklyEngine` directly or through `EngineHost`, the engine
> is constructed the same way and from a `GpuContext`. Start there.

## 1. Acquire a `GpuContext`

`DarklyEngine` needs a `GpuContext`
([`crates/darkly/src/gpu/context.rs`](../crates/darkly/src/gpu/context.rs)),
which wraps a `wgpu` device, queue, and (for on-screen use) a surface. There are
two ways in:

### On-screen (browser / native window) — `GpuContext::new`

Create a `wgpu::Instance`, a surface for your canvas/window, then:

```rust
let gpu = GpuContext::new(
    instance,
    surface,
    wgpu::Limits::downlevel_webgl2_defaults(),
    initial_width,
    initial_height,
).await;
```

This is what the WASM bridge does in
[`frontend/wasm/src/api.rs`](../frontend/wasm/src/api.rs); a native windowed
backend would do the same with a desktop surface.

### Headless (tests, servers, offscreen render) — `GpuContext::new_headless`

When there's no surface, hand in a device + queue you already requested:

```rust
use darkly::gpu::context::GpuContext;
use darkly::engine::DarklyEngine;

let gpu = GpuContext::new_headless(device, queue);
let mut engine = DarklyEngine::new(gpu, 1024, 768);
```

In the test suite this is wrapped by `test_device()`
([`crates/darkly/src/gpu/test_utils.rs`](../crates/darkly/src/gpu/test_utils.rs),
behind `--features testing`):

```rust
fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}
```

## 2. Construct the engine

```rust
let mut engine = DarklyEngine::new(gpu, doc_width, doc_height);
```

`DarklyEngine::new` allocates a fresh document of `doc_width × doc_height`, builds
the compositor, undo stack, and brush/paint pipelines, and seeds a private tool
session. That's all you need for single-engine use.

**Sharing tool state across engines.** A multi-document host (e.g. the multi-tab
editor) wants every engine to read the same active tool / brush. For that, build
a `SharedToolSession` once and pass it to each engine:

```rust
let session = darkly::tool::SharedToolSession::new();
session.write().insert(darkly::brush::state::BrushState::new());

let engine = DarklyEngine::new_with_tool_session(gpu, session.clone(), w, h);
```

(`DarklyEngine::new` is just this with a private session allocated for you.)

## 3. Drive the engine

The engine is authored against three state categories — **document**
(authoritative, undoable), **session** (transient editor state), and
**compositor** (derived GPU realization). Data flows downhill: document →
compositor. You mutate the document and the compositor catches up at render time.

Each frame: mutate, then `render(time)`:

```rust
let needs_more = engine.render(time_secs);
```

`render` composites the current document into the surface (or offscreen target)
and returns whether there's outstanding work (animation or pending readbacks) —
a windowed host loops while it's `true`.

> **Never block on GPU readbacks.** Do not call `device.poll(Wait)`,
> `blocking_read()`, or any synchronous GPU→CPU readback in production code — it
> deadlocks on WebGPU. Readbacks are frame-driven and async (see
> [`docs/lessons-learned/gpu-lessons-learned.md`](lessons-learned/gpu-lessons-learned.md) §5).
> `test_utils::readback_texture()` / `blocking_read()` are `#[cfg(test)]`-only and
> work on native (Vulkan/Metal) where `device.poll(Wait)` drives the queue.

## A complete headless example

This acquires a headless GPU, builds an engine, paints an 8×8 red layer into the
document, applies an "invert" adjustment to it, and renders — exercising the full
**document → compositor** flow without a window.

```rust
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device; // --features testing

// 1. GPU context (headless) + an engine over a 64×64 document.
let (device, queue) = test_device();
let gpu = GpuContext::new_headless(device, queue);
let mut engine = DarklyEngine::new(gpu, 64, 64);

// 2. Paste an 8×8 opaque-red buffer as a new layer at (0, 0). `paste_image`
//    returns the new layer's id.
let pixels: Vec<u8> = std::iter::repeat([255, 0, 0, 255]).take(8 * 8).flatten().collect();
let layer = engine.paste_image(8, 8, &pixels, 0, 0, None);

// 3. Mutate it — apply the "invert" adjustment (red -> cyan).
engine.apply_adjustment(layer, "invert");

// 4. Render a frame. The return is whether there's outstanding work; a windowed
//    host loops while it's true, a one-shot headless render discards it.
engine.render(0.0);
```

Other common engine ops you'll reach for: `begin_stroke` / `stroke_to` /
`end_stroke` for brush input, and the selection/layer/transform methods. The
[`crates/darkly/tests/`](../crates/darkly/tests/) directory is the best worked
reference — every feature has a test that drives the engine directly.

> The `add_raster` / `send(...)` string protocol from the
> [TypeScript guide](getting-started-typescript.md) is the **browser** path: the
> WASM bridge decodes those request kinds into exactly these engine methods. In
> Rust you call the methods directly.

## About `EngineHost`

The unified frame-driven scheduler that backs `host.tick(time)` / `host.pump()`
is being built per the engine-host rework. While it lands, the stable, supported
way to embed Darkly is `DarklyEngine::new` + `render`, exactly as the headless
tests do. The host changes *how* requests and deferred readbacks are orchestrated
around the engine — not how the engine is constructed — so code written against
`DarklyEngine` today carries forward. Treat any intermediate host signatures
(e.g. closure-injected `tick`) as scaffolding, not public API.

## Running the checks

Engine code that touches the GPU must run single-threaded (the integration tests
share one process-wide wgpu device):

```bash
cargo test --workspace --exclude darkly-wasm --features darkly/testing -- --test-threads=1
```

`--features darkly/testing` exposes `gpu::test_utils`, `blocking_read`, and the
engine's `test_readback_*` accessors that integration tests rely on.

## Where to go next

- Architecture and state boundaries: [`CONTRIBUTING.md`](../CONTRIBUTING.md).
- Anything involving x/y coordinates: [`docs/coordinate-systems.md`](coordinate-systems.md).
- GPU readback rules: [`docs/lessons-learned/gpu-lessons-learned.md`](lessons-learned/gpu-lessons-learned.md).
- Driving the engine from the browser: [`docs/getting-started-typescript.md`](getting-started-typescript.md).
- Worked examples: [`crates/darkly/tests/`](../crates/darkly/tests/).
