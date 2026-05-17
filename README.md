![darkly](https://github.com/user-attachments/assets/62115b89-ab63-453c-93ce-a513e500fad7)

[![Discord](https://img.shields.io/discord/1495886270780539021?label=Discord&logo=discord&logoColor=white&style=for-the-badge&color=9500ff)](https://discord.gg/kFz2FGhbpu)
[![Patreon](https://img.shields.io/badge/Patreon-Forbidden_Relics-orange?logo=patreon&style=for-the-badge&color=6914ff)](https://www.patreon.com/c/DarklyArt)
[![Blog](https://img.shields.io/badge/Blog-Deranged_Texts-orange?logo=substack&logoColor=white&style=for-the-badge&color=4400ff)](https://www.patreon.com/c/DarklyArt)

![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=9500ff)
![Svelte](https://img.shields.io/badge/Svelte-000000?style=for-the-badge&logo=svelte&logoColor=6914ff)
![WebAssembly](https://img.shields.io/badge/WebAssembly-000000?style=for-the-badge&logo=WebAssembly&logoColor=4400ff)

> [!IMPORTANT]
> **Darkly is in beta**! Features are being [added daily](#feature-roadmap). Please [report bugs](https://github.com/darkly-art/darkly/issues/new) so we can squash them.

[Darkly](https://darkly.art) is a GPU-native editor written in Rust. Geared towards digital painters, it's got everything you expect from a paint program, plus some special **[dark arts](#unique-darkly-features)** to help you rage against the machine.

### Darkly pledges to:

- 🛐 Honor human imagination
- ⚛️ Run offline and without a login
- ☯️ Never [steal or license](https://x.com/SamSantala/status/1798292952219091042) your art
- ☮️ Stay free and open source forever

**Try the demo [here](https://demo.darkly.art).**

## Kickstarter

We're gearing up for a Kickstarter! Vote in the [discord](https://discord.gg/kFz2FGhbpu) for which features you want most, and help us rescue our friends and colleagues from the iron grip of Adobe!

## Feature Roadmap

These are features that are valuable or essential to digital art workflows. They're subject to change, but we'll keep them up-to-date so everyone can see the progress.

### Painting & brush engine
- [x] Brush tool, eraser, fill (flood), gradient (linear), color picker (eyedropper)
- [x] 13 built‑in brushes; node‑graph brush editor with live preview
- [x] 23 brush nodes (pen input, stamps, curves, scatter, watercolor, liquify, texture overlay, …)
- [x] Pressure / tilt / spacing / distance / angle inputs
- [x] Laplacian stabilizer
- [ ] Smudge / blend / blur tool
- [ ] Clone / stamp tool
- [ ] Dodge / burn

### Color picking & swatches
- [x] HSV picker, foreground/background swatches
- [x] Eyedropper (async GPU readback)
- [ ] Recent colors
- [ ] Saved swatches / palettes
- [ ] Palette file import (.aco, .gpl)
- [ ] Color harmonies

### Layers
- [x] Raster layers + groups, drag‑reorder, visibility, lock, opacity, name, collapse, passthrough
- [x] 16 blend modes (Normal → Luminosity, Krita‑compatible)
- [x] Layer masks (one per host)
- [ ] Duplicate layer / group
- [ ] Merge down
- [ ] Flatten image
- [ ] Clipping mask
- [ ] Adjustment layers
- [ ] Group blend mode / opacity (groups don't carry BlendProps yet)

### Selection
- [x] Rect, ellipse, lasso, magic wand
- [ ] Polygon
- [x] Replace / Add / Subtract / Intersect modes
- [ ] Feather + antialias
- [ ] Invert (boolean op exists)
- [ ] Select All / Deselect / Invert as menu+hotkey actions
- [ ] Grow / Shrink / Border / Smooth as discrete commands
- [ ] Stroke selection (paint along marching ants)
- [ ] Save / load selection to channel

### Color adjustments
- [ ] Invert colors
- [ ] Hue / Saturation / Lightness
- [ ] Brightness / Contrast
- [ ] Levels
- [ ] Curves
- [ ] Color balance
- [ ] Channel mixer
- [ ] Desaturate
- [ ] Threshold
- [ ] Posterize
- [ ] Gradient map

### Transform & canvas
- [x] Affine transform tool (translate / scale / rotate via floating content)
- [x] Engine‑level canvas resize
- [ ] Crop tool / crop to selection
- [ ] Trim to content / autocrop
- [ ] Flip canvas H / V
- [ ] Rotate canvas 90° CW / CCW / 180°
- [ ] Flip layer / selection H / V
- [ ] Perspective, skew, free distort
- [ ] Warp / mesh transform

### View
- [x] Pan / zoom / rotate view
- [ ] Fit to screen
- [ ] 100% / zoom presets
- [ ] Pixel grid toggle
- [ ] Symmetry / mirror painting (X, Y, radial)
- [ ] Reference image panel
- [ ] Rulers, guides, snapping
- [ ] Navigator / overview window
- [ ] Palette Popup

### File I/O
- [x] Clipboard copy / cut / paste (PNG via browser clipboard)
- [x] Brush export / import (binary bundle)
- [ ] Export to PNG / JPEG / WebP file
- [ ] Open image from file
- [ ] Save / Open native `.darkly` document
- [ ] PSD / XCF import
- [ ] SVG export
- [ ] Recent files

### Undo & history
- [x] Undo / redo (configurable depth, defaults 100)
- [x] Coalesced property edits, GPU region snapshots, compound actions
- [ ] History panel UI
- [ ] Branched history

### Brush settings & config
- [x] Config schema with 8 sections, typed widgets, hotkey capture
- [x] Krita / Photoshop / GIMP hotkey presets
- [x] Settings modal, theme system
- [ ] Per‑brush preset save/load UI
- [ ] Brush size / hardness sliders in main UI
- [ ] Brush dynamics / stabilization settings panel

### Text & vector
- [ ] Text tool / text layers
- [ ] Vector shapes
- [ ] Bézier paths

### Misc
- [x] Hotkey system + searchable cheatsheet (80+ rebindable actions)
- [x] Floating layers (transient paste / transform)
- [ ] Autosave + crash recovery
- [ ] Animation timeline / onion skin
- [ ] File browser

## Getting started

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [wasm-pack](https://rustwasm.github.io/wasm-pack/installer/)
- [Node.js](https://nodejs.org/) >= 18

```sh
# Install all workspace dependencies (frontend + website + shared styles)
npm install

# Build the WASM package
wasm-pack build frontend/wasm --target web

# Start the frontend dev server
npm --prefix frontend run dev
```

Open the URL printed by vite (typically `https://localhost:5173`). Requires a browser with WebGPU support (Chrome 113+, Edge 113+, Firefox Nightly with flag).

**GPU backend configuration (Linux):** Chrome's WebGPU defaults to a software rasterizer on many Linux setups. Launch Chromium with GPU and Vulkan support:

```sh
chromium --enable-features=Vulkan --enable-unsafe-webgpu
```

You can verify the active backend at `chrome://gpu` - look for "Vulkan" under Graphics Feature Status. On macOS and Windows this is generally not needed (Metal and D3D12 are used by default).

## Use of AI

While AI has been leveraged heavily for this codebase, careless vibe coding and AI slop is **strictly forbidden**.

I (TheTechromancer) learned to code long before AI, and have spent much of my career maintaining [large codebases](https://github.com/blacklanternsecurity/bbot). The danger of feature creep and architectural bloat is real, which is why whenever a feature is implemented in Darkly, a human must understand how it works and its implications for the rest of the codebase.

Great care is being taken to keep Darkly architecturally lean and clean. This means enforcing modularity, vigilance against duplicate/dead code, and a *shit ton* of automated tests, including a regression test for every bug. See [AGENTS.md](AGENTS.md) for how we avoid AI slop.

## Acknowledgments

Darkly stands on the shoulders of giants. Two programs in particular have influenced this project, and we love them dearly.

**[GIMP](https://www.gimp.org/)** ([source](https://github.com/GNOME/gimp)) - originally written by **Spencer Kimball** and **Peter Mattis** in 1995, and maintained today by **Michael Natterer** and **Jehan Pagès**, with decades of contributions from a community far too large to list here (see the upstream [`AUTHORS`](https://github.com/GNOME/gimp/blob/master/AUTHORS) file).

**[Krita](https://krita.org/)** ([source](https://github.com/KDE/krita)) - led by **Halla Rempt**, with core contributions over the years from **Dmitry Kazakov**, **Cyrille Berger**, **Sven Langkamp**, **Wolthera van Hövell tot Westerflier**, **L. E. Segovia**, **Scott Petrovic**, and many more (see the upstream [`developers.txt`](https://github.com/KDE/krita/blob/master/krita/data/aboutdata/developers.txt)).

### Veils

Several of Darkly's veil shaders are ports or adaptations of work originally published on [Shadertoy](https://www.shadertoy.com/). The originals are exquisite; please go see them in their native habitat!

- **Bokeh** - based on [ldG3W3](https://www.shadertoy.com/view/ldG3W3) by [Dave Hoskins](https://www.shadertoy.com/user/Dave_Hoskins).
- **Kuwahara** - based on [mlffWf](https://www.shadertoy.com/view/mlffWf) by [p4vv37](https://www.shadertoy.com/user/p4vv37), with technique notes from [Acerola / Garrett Gunnell](https://github.com/GarrettGunnell/Post-Processing/tree/main/Assets/Kuwahara%20Filter).
- **Rainy glass** - ported from "Heartfelt" ([ltffzl](https://www.shadertoy.com/view/ltffzl)) by [Martijn Steinrucken / BigWIngs](https://www.shadertoy.com/user/BigWIngs). Licensed CC BY-NC-SA 3.0.
- **VHS** - ported from [XtBXDt](https://www.shadertoy.com/view/XtBXDt) by [FMS_Cat](https://www.shadertoy.com/user/FMS_Cat).
- **Watercolor** - based on [mdlXW2](https://www.shadertoy.com/view/mdlXW2) by [aeva](https://www.shadertoy.com/user/aeva).
