![darkly](https://github.com/user-attachments/assets/62115b89-ab63-453c-93ce-a513e500fad7)

[![Discord](https://img.shields.io/discord/1495886270780539021?label=Discord&logo=discord&logoColor=white&style=for-the-badge&color=9500ff)](https://discord.gg/kFz2FGhbpu)
[![Patreon](https://img.shields.io/badge/Patreon-Forbidden_Relics-orange?logo=patreon&style=for-the-badge&color=6914ff)](https://www.patreon.com/c/DarklyArt)
[![Blog](https://img.shields.io/badge/Blog-Deranged_Texts-orange?logo=substack&logoColor=white&style=for-the-badge&color=4400ff)](https://www.patreon.com/c/DarklyArt)

![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=9500ff)
![Svelte](https://img.shields.io/badge/Svelte-000000?style=for-the-badge&logo=svelte&logoColor=6914ff)
![TypeScript](https://img.shields.io/badge/TypeScript-000000?style=for-the-badge&logo=typescript&logoColor=6914ff)
![WebAssembly](https://img.shields.io/badge/WebAssembly-000000?style=for-the-badge&logo=WebAssembly&logoColor=4400ff)
![WebGPU](https://img.shields.io/badge/WebGPU-000000?style=for-the-badge&logo=webgpu&logoColor=4400ff)

> [!IMPORTANT]
> **Darkly is in beta**! Features are being [added daily](#feature-roadmap). Please [report bugs](https://github.com/darkly-art/darkly/issues/new) so we can squash them.

[Darkly](https://darkly.art) is a GPU-native editor written in Rust and geared towards digital painters. It has everything you expect from a paint program, plus some special **[dark arts](#unique-darkly-features)** to help you rage against the machine.

If you're a sane artist who likes rulers, clean vector lines, and predictable outcomes, Darkly may not be for you. Darkly is a chaotic playground for brave explorers, where messes are made and happy accidents abound. Madness isn't a bug, it's a feature.

[![darkly](https://github.com/user-attachments/assets/647404d5-c2fe-4f9f-a1f9-7b532c3a3cd0)](https://demo.darkly.art)

### Darkly pledges to:

- 🛐 Honor human imagination
- ⚛️ Run offline and without a login
- ☯️ Never [steal or license](https://x.com/SamSantala/status/1798292952219091042) your art
- ☮️ Stay free and open source forever

**Try the demo [here](https://demo.darkly.art).**

## Kickstarter

We're gearing up for a Kickstarter! Vote in the [discord](https://discord.gg/kFz2FGhbpu) for which features you want most, and help us rescue our soulmates from Adobe's iron grip!

## Unique Darkly Features

In addition to the usual paint features, Darkly has some entropic tools to empower you on your dark quests.

### Veils

Veils are where Darkly gets its name; *"For now we see through a glass, darkly"*. They're a special type of layer that sits overtop the viewport, and is visible only to the artist. By placing your art behind a strange or mysterious filter, they let you look on it with different eyes, inviting you to see something that wasn't there before.

Veils have practical uses, too. By non-destructively hiding detail, they can help counteract age-old human tendencies like art fatigue (losing good judgement because you stared at it for too long), destructive self-criticism, and premature fixation on detail.

Veils live in their own dedicated group, but within that you can stack and order them however you like. Keep in mind that adding too many can drain your battery, due to the heavy load on your GPU.

You may find different uses for veils, or not use them at all. But they give you permission to be messy, and have some happy accidents along the way.

### Voids

Voids are a special type of layer that specializes in pulling inspiration from outside sources. 

Add a void layer with churning noise, then freeze it when you like how it looks. Use the `Screenshare` void to stream another app (3D software, movie, or video game) directly into a layer. This is great for hybrid workflows, and situations where you want to try out different lighting or camera angles, without having to render and paste over and over.

Need a hand reference? Add the `Camera` void, hold up your hand, align it easily with your drawing, and bypass hours of posed camera shots and manual copy-paste.

Voids are less restricted than veils and can live anywhere in your layer stack -- over or underneath any other layer. They support masks and blend modes. They are the natural compliment to veils, and a natural integration point other art programs like Blender, which may have dedicated voids in the future 🧡

### Node-Based Brush Engine

![brush-engine-screenshot](https://github.com/user-attachments/assets/28baa4dc-9cf1-4d9f-b1e3-4ccbe5943171)

Darkly features a unified node-based brush system. Every brush type -- clone, liquify, watercolor, etc. -- all live in a single engine. This enables infinite customizability, mixing and matching of brush features, and on-the-fly creation of custom brushes.

## Feature Roadmap

These are features that are helpful or essential to digital art workflows. They're subject to change, and feel free to suggest new ones. They are always kept up-to-date so everyone can see the progress.

### Painting & brush engine
- [x] Node-based brush engine
- Brushes
    - [x] Simple round
    - [x] Calligraphy
    - [x] Ink pen
    - [x] Liquify
    - [x] Watercolor
    - [ ] Clone
    - [x] Smudge
    - [ ] Blur
    - [ ] Dodge/burn
    - [ ] Pencil / Charcoal
    - [ ] Oil / Impasto
- [x] Brush tool, eraser, fill (flood), gradient (linear), color picker (eyedropper)
- [x] Pressure / tilt / spacing / distance / angle inputs
- [x] Laplacian stabilizer

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
- [x] Duplicate layer / group
- [x] Merge down
- [x] Void layers (domain‑warped FBM noise)
- [x] Camera void (live webcam with scale / rotation / pan)
- [ ] Flatten image
- [ ] Clipping mask
- [ ] Adjustment layers
- [ ] Group blend mode / opacity (groups don't carry BlendProps yet)

### Selection
- [x] Rect, ellipse, lasso, polygon, magic wand
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
- [x] Mirror view
- [ ] Fit to screen
- [ ] 100% / zoom presets
- [ ] Symmetry / mirror painting (X, Y, radial)
- [ ] Navigator / overview window
- [ ] Palette Popup

### File I/O
- [x] New document (custom canvas size + background color)
- [x] Clipboard copy / cut / paste (PNG via browser clipboard)
- [x] Brush export / import (binary bundle)
- [x] Export to PNG / JPEG / WebP file
- [x] Open image from file
- [x] Save / Open native `.darkly` document
- [ ] Recent files
- [ ] PSD / XCF / KRA import

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

It's okay to use AI for this codebase, but careless vibe coding is **strictly forbidden**.

I (TheTechromancer) learned to code before AI, and have spent much of my career maintaining [large codebases](https://github.com/blacklanternsecurity/bbot). The [danger](https://www.reddit.com/r/vibecoding/comments/1su03dk/vibe_coded_for_6_months_my_codebase_is_a_disaster/) of feature creep and architectural bloat is real, which is why whenever a feature is implemented in Darkly, a human must first understand the changes and their long-term implications for the codebase.

Great care is being taken to keep Darkly lean and clean. This means enforcing modularity, guarding vigilantly against duplicate/dead code, and writing a *shit ton* of unit tests, including at least one regression test for every bug. See [AGENTS.md](AGENTS.md) for how we avoid AI slop.

Note that while we allow AI for coding, we are unlikely to accept any PR implementing generative AI in Darkly itself.

For details on Darkly's stance on AI, see [this blog post]().

## Acknowledgments

Darkly stands on the shoulders of giants. Three programs in particular have influenced this project, and we love them dearly.

**[GIMP](https://www.gimp.org/)** ([source](https://github.com/GNOME/gimp)) - originally written by **Spencer Kimball** and **Peter Mattis** in 1995, and maintained today by **Michael Natterer** and **Jehan Pagès**, with decades of contributions from a community far too large to list here (see the upstream [`AUTHORS`](https://github.com/GNOME/gimp/blob/master/AUTHORS) file).

**[Krita](https://krita.org/)** ([source](https://github.com/KDE/krita)) - led by **Halla Rempt**, with core contributions over the years from **Dmitry Kazakov**, **Cyrille Berger**, **Sven Langkamp**, **Wolthera van Hövell tot Westerflier**, **L. E. Segovia**, **Scott Petrovic**, and many more (see the upstream [`developers.txt`](https://github.com/KDE/krita/blob/master/krita/data/aboutdata/developers.txt)).

**[Graphite](https://graphite.art/)** ([source](https://github.com/GraphiteEditor/Graphite)) - founded by **Keavon Chambers** (@Keavon), with the core team of **Dennis Kobert** (@TrueDoctor), **Timon Schelling** (@timon-schelling), and **Adam Gerhant** (@pendapia), plus heroic contributions from **Hypercube** (@0HyperCube), **James Lindsay**, and [hundreds more](https://github.com/GraphiteEditor/Graphite/graphs/contributors). Graphite is a pioneer in bringing serious 2D graphics tooling to **Rust + WebAssembly + WebGPU**.

While Darkly's architecture is fundamentally different, it was really insightful to see how these tools tackled many of the same hard problems, and their unique and smart approaches that made them pillars of open source!

### Veils & Voids

Some of Darkly's veil and void shaders are ports or adaptations of work originally published on [Shadertoy](https://www.shadertoy.com/). I suck at shaders and the creators of these shaders are true artists. Please go see them in their native habitat!

- **Lens Blur** (veil) - based on [ldG3W3](https://www.shadertoy.com/view/ldG3W3) by [Dave Hoskins](https://www.shadertoy.com/user/Dave_Hoskins).
- **Painting** (veil) - a generalized Kuwahara filter, based on [mlffWf](https://www.shadertoy.com/view/mlffWf) by [p4vv37](https://www.shadertoy.com/user/p4vv37), with technique notes from [Acerola / Garrett Gunnell](https://github.com/GarrettGunnell/Post-Processing/tree/main/Assets/Kuwahara%20Filter).
- **Rainy glass** (veil) - ported from "Heartfelt" ([ltffzl](https://www.shadertoy.com/view/ltffzl)) by [Martijn Steinrucken / BigWIngs](https://www.shadertoy.com/user/BigWIngs).
- **VHS** (veil) - ported from [XtBXDt](https://www.shadertoy.com/view/XtBXDt) by [FMS_Cat](https://www.shadertoy.com/user/FMS_Cat).
- **Watercolor** (veil) - based on [mdlXW2](https://www.shadertoy.com/view/mdlXW2) by [aeva](https://www.shadertoy.com/user/aeva).
- **Noise** (void) - domain-warp algorithm from Inigo Quilez's ["Domain warping"](https://iquilezles.org/articles/warp/) article; the texture-sampled noise primitive (a 3D volume sampled by the FBM octave loop) is inspired by "Watery" ([MssSRS](https://www.shadertoy.com/view/MssSRS)) by [nimitz](https://www.shadertoy.com/user/nimitz) (twitter: @stormoid).