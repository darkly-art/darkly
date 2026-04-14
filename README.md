# DARKLY

Darkly is a digital paint program for artists. Its purpose is to empower veteran and new artists alike by accomodating their existing workflows, and introducing new elements designed to help them compete with AI.

Darkly's characteristic feature is "veils" - a way of mutating or obfuscating the canvas to stimulate creative exploration. 

Similar to how an AI leverages entropy to iteratively derive an image from noise, Darkly introduces entropy onto the canvas in a variety of clever and satisfying ways -- heat waves, rainy glass, turbulent water, retro CRT. These veil effects help to counteract inherent human tendencies which have always plagued artists:

- artist's block (the dreaded "white canvas")
- premature fixation on detail (RIP composition)
- lack of confidence / fear of exploration
- artist blindness (losing fresh eyes after staring at a work for too long)

Basically, Darkly gives you permission to explore wildly and freely, brainstorming with entropy, trying out different compositions, and unlocking secrets latent in your own imagination. 

It frees you from rulers, sharp lines, and your own inner critic, enabling you to paint freely and without judgement, behind the veil.

The result is a speedier and more creative ideation process, which unearths artistic expressions unique to you. What will *YOU* see in the entropy? It may surprise you!

## Architecture

Darkly's Rust core (`crates/darkly/`) is platform-agnostic. It contains the document model, GPU compositor, filters, veils, undo system, and the `DarklyEngine` — all with zero platform dependencies. A WASM bridge wraps the engine for the browser:

```
crates/darkly/          Platform-agnostic core (wgpu, pure Rust)
  src/engine.rs         DarklyEngine — all editor logic
  src/gpu/              Compositor, filters, veils, shaders
frontend/wasm/          WASM bridge (wasm-bindgen) → browser
frontend/src/           Svelte UI
```

## Getting started

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [wasm-pack](https://rustwasm.github.io/wasm-pack/installer/)
- [Node.js](https://nodejs.org/) >= 18

```sh
# Build the WASM package
wasm-pack build frontend/wasm --target web

# Install frontend dependencies and start the dev server
cd frontend
npm install
npm run dev
```

Open the URL printed by vite (typically `https://localhost:5173`). Requires a browser with WebGPU support (Chrome 113+, Edge 113+, Firefox Nightly with flag).

**GPU backend configuration (Linux):** Chrome's WebGPU defaults to a software rasterizer on many Linux setups. Launch Chromium with GPU and Vulkan support:

```sh
chromium --enable-features=Vulkan --enable-unsafe-webgpu
```

You can verify the active backend at `chrome://gpu` — look for "Vulkan" under Graphics Feature Status. On macOS and Windows this is generally not needed (Metal and D3D12 are used by default).

## Adding filters and veils

Darkly uses auto-discovery: drop a `.rs` file in `crates/darkly/src/gpu/filters/` or `crates/darkly/src/gpu/veils/` and export a `pub fn register()`. The build script generates `mod.rs` automatically. No other files need to be touched.

See `filters/noise.rs` or `veils/pixelate.rs` for the pattern.
