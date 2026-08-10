<a href="https://github.com/darkly-art/darkly"><img src="https://github.com/user-attachments/assets/62115b89-ab63-453c-93ce-a513e500fad7" alt="darkly" width="675"></a>

[![Discord](https://img.shields.io/discord/1495886270780539021?label=Discord&logo=discord&logoColor=white&style=for-the-badge&color=9500ff)](https://discord.gg/kFz2FGhbpu)
[![Patreon](https://img.shields.io/badge/Patreon-Forbidden_Relics-orange?logo=patreon&style=for-the-badge&color=6914ff)](https://www.patreon.com/c/DarklyArt)
[![Blog](https://img.shields.io/badge/Blog-Deranged_Texts-orange?logo=substack&logoColor=white&style=for-the-badge&color=4400ff)](https://darkly.art/blog)

![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=9500ff)
![Svelte](https://img.shields.io/badge/Svelte-000000?style=for-the-badge&logo=svelte&logoColor=9500ff)
![WebAssembly](https://img.shields.io/badge/WebAssembly-000000?style=for-the-badge&logo=WebAssembly&logoColor=6914ff)
![WebGPU](https://img.shields.io/badge/WebGPU-000000?style=for-the-badge&logo=webgpu&logoColor=6914ff)
[![Tests](https://img.shields.io/github/actions/workflow/status/darkly-art/darkly/ci.yml?branch=master&label=Tests&logo=github&labelColor=black&logoColor=4400ff&style=for-the-badge&color=4400ff)](https://github.com/darkly-art/darkly/actions/workflows/ci.yml)
[![Coverage](https://img.shields.io/codecov/c/github/darkly-art/darkly?token=TIIFB7UHAJ&label=Coverage&logo=codecov&labelColor=black&logoColor=4400ff&style=for-the-badge&color=4400ff)](https://codecov.io/gh/darkly-art/darkly)

> [!IMPORTANT]
> **Darkly is in beta**! Features are being [added daily](#feature-roadmap). Please [report bugs](https://github.com/darkly-art/darkly/issues/new) so we can squash them.

Do you suffer from the _oppressive sanity_ of rulers, guides, and nondestructive workflows? Break free with [Darkly](https://darkly.art), the home of happy accidents and beautiful catastrophies. Madness isn't a bug, it's a feature.

Darkly is a Photoshop alternative where painters are first-class citizens. It has a powerful brush engine, and **[dark arts](#dark-arts)** to help you commune with your imagination.

**Try the demo [here](https://demo.darkly.art).**

### Darkly pledges to:

- 🛐 Honor human imagination
- ⚛️ Run offline and without a login
- ☯️ Never [steal or license](https://x.com/SamSantala/status/1798292952219091042) your art
- ☮️ Stay free and open source forever

https://github.com/user-attachments/assets/1fc0632d-5846-4c64-bac8-e39b0794b8b5

## Features

### Node-Based Brush Engine

![brush-engine-screenshot](https://github.com/user-attachments/assets/67f8826e-a5b5-4cbe-83e1-3e29246c293c)

Darkly features a unified node-based brush system. Every brush type -- clone, liquify, watercolor, etc. -- all live in a single engine. This enables infinite customizability, mixing and matching of brush features, and on-the-fly creation of custom brushes.

### Familiar Hotkeys

<img src="https://github.com/user-attachments/assets/63544586-f006-4616-b378-97dd54e321d3" width="400"/>

On first launch, Darkly will ask you which editor preset you want.  Currently we support GIMP, Krita, and Photoshop. I come from Krita, so that one's gotten the most TLC. But we want everyone to feel at home no matter which editor they come from. If you find any gaps, please let us know!

### Hotkey Cheatsheet

Full documentation is on the way; however, Darkly is mostly self-documenting, meaning if you can't find something, you can quickly search with `CTRL+F` and immediately see its hotkey, description, etc. - without leaving the app.

<img src="https://github.com/user-attachments/assets/370043b6-24a7-4a73-8816-ed58ce9108c4" width="500"/>

If you like using hotkeys, we also have a cheat sheet just for you. You can print it or put it on a second screen.

<img src="https://github.com/user-attachments/assets/2bb1737b-169b-4ca2-9687-2c54fbc07a6b" width="500"/>


## Dark Arts

### Veils

https://github.com/user-attachments/assets/ee281ac2-37a8-4e52-91b3-78d564420e9d

Veils are where Darkly gets its name; *"For now we see through a glass, darkly"*. They're a special type of layer that sits overtop the viewport, visible only to the artist. By shrouding your art behind a mysterious pane, they invite you to see something that maybe wasn't there before.

Veils have practical uses too:

- By hiding fine details, they can prevent **premature fixation on detail**, freeing you to focus on composition.
- During the sketching / ideation phase, they can help with **blank page syndrome** and **destructive self-criticism** by giving you permission to be messy, and explore freely.
- They can also help remedy **art fatigue** (losing eyes for a piece by staring at it for too long) by helping you view it through a fresh lens.

> [!NOTE]
> Veils live in their own group, but within it you can stack and order them however you like. Remember that adding too many can drain your battery, due to the heavy load on your GPU.

### Voids

https://github.com/user-attachments/assets/a9ac3819-7209-442b-a8ba-93f567a7506e

Voids are a type of layer that specializes in pulling inspiration from outside sources.

You can use the `Noise` void to inject entropy, or `Screenshare` to stream another app (3D software, movie, or video game) directly into a layer. This is great for hybrid workflows, and situations where you need a quick reference, or want to try out different lighting or camera angles, without having to pose, render and paste over and over.

Voids can live anywhere in your layer stack -- over or underneath any other layer. They support masks and blend modes. They are the natural compliment to veils, and a natural integration point for other art programs like Blender, which has its own [dedicated void](https://extensions.blender.org/add-ons/darkly-stream/) 🧡

## Feature Roadmap

These features are ordered roughly by importance. They will be implemented mostly in order from top to bottom, prioritizing ones most requested by the community.

For a feature to count, it must be:
1) Implemented in Rust backend
2) Have a proper frontend action, with a menu path and hotkey in each editor preset, if applicable

### Essential / Must-Have
- [x] Node-based brush engine
- Brushes
    - [x] Simple round
    - [x] Ink pen
    - [x] Charcoal
    - [x] Smudge
    - [x] Watercolor
    - [x] Liquify
    - [x] Clone
    - [x] Blur
    - [ ] Dodge/burn
    - [x] Calligraphy
    - [ ] Oil / Impasto
- [x] Brush tool, eraser, fill, gradient, color picker
- [x] Text tool
- [x] Pressure / tilt / spacing / distance / angle inputs
- [x] Laplacian stabilizer
- [x] HSV picker, foreground/background swatches
- [x] Color picker
- [x] Raster layers + groups
- [x] 16 blend modes
- [x] Layer masks
    - [x] Link/unlink mask and host transforms
- [x] Rect, ellipse, lasso, polygon, magic wand selection
- [x] Selection Replace / Add / Subtract / Intersect modes
- [x] Pan / zoom / rotate view
- [x] Undo / redo
- [x] New document
- [x] Open image from file
- [x] Save / Open native `.darkly` document
- [x] Export to PNG / JPEG / WebP file
- [x] Clipboard copy / cut / paste
- [ ] Documentation

### Important — expected for serious work
- [x] Generic transform tool
- [x] Perspective transform
- [x] Merge down
- [x] Duplicate layer / group
- [x] Crop to selection
- [x] Canvas resize
- [x] Image rescale
- [x] Select All / Deselect / Invert
- [x] Command palette
- [x] Application menu
- [x] Autosave + crash recovery
- [x] Filter layers
    - [x] Brightness / Contrast
    - [x] Hue / Saturation / Lightness
    - [x] Levels
    - [x] Curves
    - [x] Invert colors
    - [x] Black and White (desaturate)
    - [x] Chromatic aberration
- [ ] Clipping mask
- [x] Feather + antialias
- [x] Grow / Shrink / Border / Smooth
- [x] Flip canvas H / V
- [x] Rotate canvas 90° CW / CCW / 180°
- [x] Flip layer / selection H / V
- [ ] Recent colors
- [ ] Saved swatches / palettes
- [ ] Palette popup
- [ ] Navigator / overview window
- [ ] History panel UI
- [x] Process recording
- [x] Mirror view
- [x] Reset view
- [x] Canvas-rotation snapping
- [x] Fit to screen / center view
- [ ] 100% / zoom presets
- [ ] Symmetry / mirror painting
- [x] Installable PWA
- [x] Krita / Photoshop / GIMP hotkey presets
- [x] Settings modal
- [x] Theme system
- [x] Hotkey system + searchable cheatsheet
- [x] Floating layers

### Advanced & specialized — power-user, niche, and polish
- [x] Veils
- [x] Veil picker
- [x] Void layers
- [x] Camera void
- [x] Screenshare void
- [x] [Blender void](https://github.com/darkly-art/blender-extension)
- [x] Group blend mode / opacity
- [x] Dockable / tiled panels (drag to reorder, tab, split-dock)
- [x] Pop out panels into separate OS windows (cross-window drag)
- [ ] Brush save/load + editable nodes/wires
- [ ] Recent files
- [ ] PSD / XCF / KRA import
- [ ] Perspective, skew, free distort
- [ ] Warp / mesh transform
- [ ] Gradient map
- [ ] Color balance
- [ ] Channel mixer
- [ ] Threshold
- [ ] Posterize
- [ ] Color harmonies
- [ ] Palette file import
- [ ] Trim to content / autocrop
- [ ] Stroke selection
- [ ] Save / load selection to channel
- [ ] Snap canvas to right angles
- [ ] Branched history

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

# ...or expose it on your LAN (binds all interfaces, 0.0.0.0)
npm --prefix frontend run dev -- --host
```

Open the URL printed by vite (typically `https://localhost:5173`). Requires a browser with WebGPU support (Chrome 113+, Edge 113+, Firefox Nightly with flag).

**GPU backend configuration (Linux):** Chrome's WebGPU defaults to a software rasterizer on many Linux setups. Launch Chromium with GPU and Vulkan support:

```sh
chromium --enable-features=Vulkan --enable-unsafe-webgpu
```

You can verify the active backend at `chrome://gpu` - look for "Vulkan" under Graphics Feature Status. On macOS and Windows this is generally not needed (Metal and D3D12 are used by default).

## Use it in your own project

Darkly's core is a standalone, platform-agnostic Rust crate, [`darkly`](https://crates.io/crates/darkly). The document model, brush engine, GPU compositor, and undo system all live here with zero platform dependencies, and run anywhere [wgpu](https://wgpu.rs) does: native (Vulkan / Metal / DX12) or the browser (WebGPU). The app above is a Svelte frontend over this exact crate, through a thin WebAssembly bridge.

If you're building a graphics tool in Rust -- a drawing app, annotation layer, whiteboard, texture editor, etc. -- you can embed the engine instead of writing all these features from scratch.

[![crates.io](https://img.shields.io/crates/v/darkly?style=for-the-badge&logo=rust&label=crates.io&labelColor=black&color=9500ff)](https://crates.io/crates/darkly)
[![docs.rs](https://img.shields.io/docsrs/darkly?style=for-the-badge&logo=docsdotrs&labelColor=black&color=4400ff)](https://docs.rs/darkly)

```sh
cargo add darkly
```

See the [crate README](crates/darkly/README.md) for a runnable example, and the [API docs](https://docs.rs/darkly) for full details.

## Contribution

We love hackers as much as artists. Contributions are welcome! Please see [AGENTS.md](./AGENTS.md) for details.

### Use of AI

It's acceptable to use AI for this codebase, but careless vibe coding is **strictly forbidden**.

I (TheTechromancer) learned to code before AI, and have spent much of my career maintaining [large codebases](https://github.com/blacklanternsecurity/bbot). The [danger](https://www.reddit.com/r/vibecoding/comments/1su03dk/vibe_coded_for_6_months_my_codebase_is_a_disaster/) of feature creep and architectural bloat is real, which is why whenever a feature is implemented in Darkly, a human must first understand the changes and their long-term implications for the codebase.

Great care is being taken to keep Darkly lean and clean. This means enforcing modularity, guarding vigilantly against duplicate/dead code, and writing a *shit ton* of unit tests, including at least one regression test for every bug. See [AGENTS.md](AGENTS.md) for how we avoid AI slop.

Note that while we allow AI for coding, we are **unlikely to accept any PR implementing generative AI in Darkly itself**. AI features are not off the table; however they must run fully offline and without any reliance on third party APIs. Additionally, any feature that speeds up generation while sacrificing creative input or control from the artist, will likely be rejected.

**Every PR must open with a short human-written description explaining _why_ the effort was undertaken and who it's useful to**, above the AI-generated technical description. PRs that look entirely machine-generated will be closed.

We are not sensitive to AI-related questions. If you're unsure, please ask!

## Acknowledgments

Darkly stands on the shoulders of giants. Three programs in particular have influenced this project, and we love them dearly.

**[GIMP](https://www.gimp.org/)** ([source](https://github.com/GNOME/gimp)) - originally written by **Spencer Kimball** and **Peter Mattis** in 1995, and maintained today by **Michael Natterer** and **Jehan Pagès**, with decades of contributions from a community far too large to list here (see the upstream [`AUTHORS`](https://github.com/GNOME/gimp/blob/master/AUTHORS) file).

**[Krita](https://krita.org/)** ([source](https://github.com/KDE/krita)) - led by **Halla Rempt**, with core contributions over the years from **Dmitry Kazakov**, **Cyrille Berger**, **Sven Langkamp**, **Wolthera van Hövell tot Westerflier**, **L. E. Segovia**, **Scott Petrovic**, and many more (see the upstream [`developers.txt`](https://github.com/KDE/krita/blob/master/krita/data/aboutdata/developers.txt)).

**[Graphite](https://graphite.art/)** ([source](https://github.com/GraphiteEditor/Graphite)) - founded by **Keavon Chambers** (@Keavon), with the core team of **Dennis Kobert** (@TrueDoctor), **Timon Schelling** (@timon-schelling), and **Adam Gerhant** (@pendapia), plus heroic contributions from **Hypercube** (@0HyperCube), **James Lindsay**, and [hundreds more](https://github.com/GraphiteEditor/Graphite/graphs/contributors). Graphite is a pioneer in bringing serious 2D graphics tooling to **Rust + WebAssembly + WebGPU**.

While Darkly's architecture is fundamentally different, it was really insightful to see how these tools tackled many of the same hard problems, and their unique and smart approaches that made them pillars of open source!

A special thank you to **[Nick Cameron](https://github.com/nrc)** for graciously gifting us the `darkly` crate on crates.io. 💜

### Veils & Voids

Some of Darkly's veil and void shaders are ports or adaptations of work originally published on [Shadertoy](https://www.shadertoy.com/). I suck at shaders and the creators of these shaders are true artists. Please go see them in their native habitat!

- **Lens Blur** (veil) - based on ["Bokeh Venice"](https://www.shadertoy.com/view/ldG3W3) by [Dave Hoskins](https://www.shadertoy.com/user/Dave_Hoskins).
- **Painting** (veil) - based on ["Generalized Kuwahara shader"](https://www.shadertoy.com/view/mlffWf) by [p4vv37](https://www.shadertoy.com/user/p4vv37), with technique notes from [Acerola / Garrett Gunnell](https://github.com/GarrettGunnell/Post-Processing/tree/main/Assets/Kuwahara%20Filter).
- **Rainy glass** (veil) - ported from ["Heartfelt"](https://www.shadertoy.com/view/ltffzl) by [Martijn Steinrucken / BigWIngs](https://www.shadertoy.com/user/BigWIngs).
- **VHS** (veil) - ported from ["20151110_VHS"](https://www.shadertoy.com/view/XtBXDt) by [FMS_Cat](https://www.shadertoy.com/user/FMS_Cat).
- **Watercolor** (veil) - based on ["watercolor propagation"](https://www.shadertoy.com/view/mdlXW2) by [aeva](https://www.shadertoy.com/user/aeva).
- **Noise** (void) - domain-warp algorithm from Inigo Quilez's ["Domain warping"](https://iquilezles.org/articles/warp/) article; the texture-sampled noise primitive (a 3D volume sampled by the FBM octave loop) is inspired by ["Watery"](https://www.shadertoy.com/view/MssSRS) by [nimitz](https://www.shadertoy.com/user/nimitz) (twitter: @stormoid).
