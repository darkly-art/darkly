# 3DCoat Brush System — Deep Dive

## Artist Sentiment & Market Position

### Market Position

3DCoat (by Pilgway, led by Andrew Shpagin) occupies a **niche but respected position** as a versatile all-in-one tool covering sculpting, retopology, UV editing, and texture painting. It is not an industry standard — ZBrush dominates sculpting, Substance Painter dominates texturing — but it's a credible substitute for both, particularly for freelancers and small studios who can't justify two $500+ licenses.

3DCoat 2024 received a **7/10 from Creative Bloq**, praised for improved DCC integration and new modeling tools, but noted as "lacking some tools found in Substance 3D Painter."

The community is small but loyal. There is no equivalent to ZBrush Central's massive forum or Substance's industrial ecosystem. 3DCoat's market trajectory is "stays relevant" rather than "growing rapidly."

### What Artists Love

- **Hand-painted texturing** — universally praised as the best tool for stylized, painterly 3D texturing. Artists describe Substance Painter as "Substance Material Applier" — great for procedural PBR but bad for actual hand-painting. 3DCoat is the opposite: built for artists who want to **paint**
- **Multi-channel PBR painting** — a single brush stroke writes to color, depth, roughness, metalness simultaneously. This is the feature artists conflate with "composable brushes"
- **Brush system flexibility** — "Still love its brush system way more than Substance Painter." The brush alphas, strips, stencils, and conditions system gives deep control
- **Integrated pipeline** — sculpt → retopo → UV → paint → export, all in one tool. No file-passing between apps
- **Voxel sculpting freedom** — true volumetric sculpting with boolean operations, topology changes, tunnel-punching. ZBrush can't do this natively
- **Retopology tools** — widely considered the best manual retopo workflow of any DCC tool
- **Photoshop brush (.abr) import** — supported for brush alphas/tips
- **Price** — perpetual license, no subscription, significantly cheaper than ZBrush + Substance combined

### Top Pain Points

1. **Performance at scale** — cannot handle the mesh density ZBrush can. ZBrush handles 100M+ polys smoothly; 3DCoat becomes "laggy" at 128M triangles even on high-end hardware. 32M is the practical ceiling
2. **Triangle-based topology** — everything that isn't a voxel is a triangle. Artists from ZBrush miss quad-based subdivision workflows
3. **UI/UX fragility** — the "rooms" system forces awkward navigation between sculpting and texturing. Artists report extensive UI inconsistencies, broken features, and visual glitches. Bugs reported since 2013 remain open
4. **Documentation** — described as "scant and outdated." The official docs are thin and often missing actual content (confirmed during this research — many doc pages are empty shells)
5. **Substance Painter's procedural advantage** — 3DCoat's Smart Materials are more limited than Substance's generator/filter ecosystem. For production PBR texturing, Substance wins
6. **Small community** — fewer tutorials, fewer shared resources, fewer presets, smaller marketplace than ZBrush or Substance ecosystems
7. **Not an industry standard** — "having ZBrush on your CV carries more weight when searching for a job"

### Key Takeaway

3DCoat is the **best tool for artists who want to paint** on 3D models with a traditional artistic approach. Its brush system genuinely excels for hand-painted, stylized work. But it's a jack-of-all-trades that's master of none in any individual category — ZBrush sculpts better, Substance textures better procedurally, Blender models better. The "composable brush engine" reputation from the Krita community is a misunderstanding of what 3DCoat actually does (see analysis below).

---

## Overview & Architecture

3DCoat's brush system is **not a single engine** but a **workspace-segregated, multi-modal system**. The brush behavior changes fundamentally depending on which "Room" (workspace) the user is in:

| Room | What Brushes Operate On | Data Representation |
|------|------------------------|---------------------|
| **Sculpt Room (Voxel)** | Signed Distance Field volume | Sparse voxel octree |
| **Sculpt Room (Surface)** | Triangulated mesh vertices | Triangle mesh |
| **Paint Room** | UV-mapped texture channels | Per-pixel / Microvertex / PTEX textures |
| **Retopo Room** | Topology construction | Quad/tri mesh on reference surface |
| **Modeling Room** | Polygon mesh | Low-poly mesh |

This architecture means the "brush system" is really **multiple separate systems** sharing a common UI layer (the Brush Components panel). A voxel sculpt brush and a paint brush have almost nothing in common underneath.

### Brush Components Panel

The unified brush configuration UI has these subsystems:

```
Brush Top Bar (quick-access controls)
├── Alphas Panel         — grayscale tip images (the "stamp" shape)
├── Strips Panel         — repeating textures along stroke path
├── Stencils Panel       — screen-space masking images
├── Stroke Modes (E-Panel) — how the brush draws (freehand, line, lasso, spline, etc.)
├── Brush Options Panel  — core parameters (size, depth, opacity, smoothing, etc.)
├── Conditions Limiter   — masks brush effect by surface properties (curvature, AO, height, color)
├── Smart Materials      — procedural multi-channel PBR material presets
└── Presets              — saved brush configurations
```

---

## 1. Sculpting Brushes

### Voxel Mode

Voxel sculpting operates on a **signed distance field (SDF)** stored in a **sparse octree**. The SDF is modified by brush operations, then an isosurface extraction algorithm (Marching Cubes variant / Surface Nets) polygonizes the result for display.

**Key characteristic:** Voxels naturally handle topology changes. You can punch holes, merge objects, perform boolean operations — no mesh topology constraints. This is 3DCoat's most distinctive capability vs. ZBrush.

| Tool | Description |
|------|-------------|
| **Vox Clay** | Blends expansive strokes with simultaneous smoothing. Authentic clay-buildup look |
| **Clay Fast** | Like Clay but without post-relaxing. Faster but rougher |
| **Draw** | Standard push/pull along normals. Good for spontaneous detail, horns, spikes |
| **Airbrush** | Soft, gradual buildup of volume |
| **Carve** | High peaks and deep gouges with no smoothing. Sharp detail work |
| **Flatten** | Flattens surface beneath brush to a plane. Also carves into it |
| **Fill** | Fills cavities without removing other detail. More precise than Smooth |
| **Smooth** | Evens out irregular areas (also available as Shift-hold modifier on any brush) |
| **Scrape** | Scrapes peaks down to a flat plane. Not affected by pen dynamics — size and intensity only |
| **Grow** | Inflates or deflates surface beneath cursor |
| **Pinch** | Creates tight edges, cavities, and peaks by pulling geometry toward stroke center |
| **Smudge** | Drags surface topology along with brush. Good for wrinkles, folds |
| **Build** | Adds discrete volume blobs |
| **Cutoff** | Boolean-like cuts through voxel volumes |
| **Sphere** | Adds/subtracts sphere primitives |
| **Cube/Primitive** | Adds/subtracts box primitives |
| **Muscle** | Draws tube/muscle-like forms along a stroke |
| **Snake / Toothpaste** | Extrudes tube/rope shapes that follow cursor path |
| **Split** | Splits/cuts the mesh |
| **VoxHide** | Hides parts of the voxel volume for focused work |
| **Close Holes** | Fills holes in voxel shells |
| **Res+** | Locally increases voxel resolution in the brushed area |

**Voxel-only operations:** Boolean (merge, subtract, intersect), soft booleans (new in 2025 — booleans with automatic edge beveling).

### Surface Mode

Surface mode operates on **triangulated mesh directly** — pushing/pulling vertices like ZBrush-style subdivision surface sculpting. Faster than voxel mode but cannot change topology.

All voxel tools have surface equivalents, plus additional tools:

| Tool | Description |
|------|-------------|
| **Crease** | Pulls polygons together, creating cavities (or extrusions with Ctrl) |
| **Layer** | Adds displacement up to a fixed depth ceiling — won't build up beyond the limit |
| **Elastic** | Moves geometry with rubbery deformation |
| **Chisel** | Hard-edged carving |
| **Freeze** | Paints a mask protecting geometry from editing (analogous to ZBrush masking) |
| **Stamps** | Applies height-map stamps to the surface |
| **Strips** | Draws repeating patterned strips along a stroke path |
| **Shift (Relax)** | Relaxes mesh topology without changing shape |
| **Tangent Smooth** | Smooths along surface tangent plane |
| **Polish** | Polishes/flattens while preserving hard edges |

### LiveClay / Dynamic Tessellation

A hybrid mode where the mesh **auto-retessellates as you sculpt**, combining voxel-like freedom (topology changes) with surface-mode speed. Works with most surface brushes. This is similar to ZBrush's DynaMesh but operates in real-time during sculpting rather than requiring a manual remesh step.

### 3D Brush Alphas (Unique to 3DCoat)

3DCoat supports using **3D mesh geometry** as brush stamps — not just 2D grayscale heightmaps but actual 3D objects that boolean-union or carve into the voxel volume. You can create an alpha from your current sculpt and use it as a stamp. This is unique among digital sculpting tools.

**Vector Displacement Maps (VDM)** were added in 3DCoat 2024 — EXR-based VDM brushes that capture full 3D displacement including undercuts and overhangs, not just heightfield data.

---

## 2. Paint Room Brushes

The Paint Room operates on UV-mapped textures and simultaneously paints across **multiple PBR channels**:

### Paintable Channels

| Channel | Description |
|---------|-------------|
| **Color (Albedo/Diffuse)** | Base color |
| **Depth / Height / Displacement** | Surface height (generates normal maps and/or displacement) |
| **Roughness (Glossiness)** | Surface roughness/smoothness |
| **Metalness** | Metal vs non-metal |
| **Normal Map** | Per-pixel normal direction |
| **Emissive (Glow)** | Self-illumination |
| **Opacity** | Alpha transparency |
| **Ambient Occlusion** | Baked AO |
| **Curvature** | Auto-generated curvature map for smart materials |
| **Specular** | Specular color (for spec/gloss workflow) |

**Each channel can be independently locked/unlocked.** Per-channel opacity and depth control means you can paint roughness at 30% while painting color at 80%. This simultaneous multi-channel painting is the core differentiator — a single stroke creates a complex, physically plausible result.

### Paint Tools

| Tool | Description |
|------|-------------|
| **Brush (Paint)** | Standard painting brush with alpha, opacity, flow |
| **Pen / Pencil** | Hard-edged painting/inking |
| **Airbrush** | Soft-edged spray painting, accumulates |
| **Eraser** | Erases to layer below or base |
| **Fill** | Flood-fills region or entire object |
| **Stamp** | Stamps a texture/image onto surface |
| **Clone** | Clone-stamps from one area to another (Ctrl+LMB to set source) |
| **Blur** | Blurs painted texture (part of Color Operations multi-tool) |
| **Sharpen** | Sharpens painted texture |
| **Smudge** | Pushes/smears paint around. Also "Shift Layer in tangent space" |
| **Dodge / Burn** | Lightens or darkens existing paint |
| **Color Pick** | Eyedropper — samples color from surface |
| **Projection Painting** | Projects an image/photo onto the surface |
| **Smart Material Brush** | Applies a procedural PBR material via brush |
| **Speckle** | Speckled/stippled painting |
| **Spline Drawing** | Paint along a defined spline path |
| **Image Curve Tool** | Curves-based image application |
| **Text Curve Tool** | Text along a path |
| **Hide Tool** | Mask painting (hide areas from editing) |

### Painting Modes

3DCoat supports four distinct texture painting approaches:

**Per-Pixel Painting:**
- Standard UV-mapped texture painting
- Quality doesn't depend on camera distance — WYSIWYG
- Can blur pixels because every pixel has neighbors
- Can operate on back side of model (fill, blur, effects)
- Best for: high-res textures, digital art, pixel art

**Microvertex Painting (Displacement):**
- Subdivides mesh, paints onto vertices of the dense mesh
- Colors translate to displacement values — pushes/pulls vertices
- Good for intricate details: wrinkles, pores, fine patterns
- Exports as displacement maps
- Best for: heavy displacement painting

**PTEX Painting:**
- Per-face texturing that **eliminates UV mapping entirely**
- Each polygon gets its own small texture patch
- Seamless by design — 1-pixel border between patches handles filtering
- Every pixel on texture corresponds to only one vertex on the patch
- Best for: avoiding UV seam artifacts, rapid prototyping

**Vertex Painting (Surface Painting / Polypaint):**
- Direct vertex color application
- Similar to ZBrush's Polypaint
- Best for: quick color blocking, vertex-color-based workflows

### Blend Modes

Standard Photoshop-style blend modes: Normal, Multiply, Screen, Overlay, Soft Light, Hard Light, Darken, Lighten, Color Dodge, Color Burn, Add, Subtract, Difference, Exclusion, Hue, Saturation, Color, Luminosity (~20+ modes).

---

## 3. Brush Components — The Subsystems

### Alphas (Brush Tips)

The alpha is a **grayscale intensity map** controlling the brush shape — analogous to Krita's brush tips or Photoshop's brush tips.

**Supported import formats:** PNG, TGA, BMP, TIF, PSD, EXR, HDR, ABR (Photoshop)

**Alpha features:**
- Built-in library of alphas (fabric, rock, skin, scales, ornamental, etc.)
- Create alpha from current sculpt (capture 3D geometry as a 2D alpha)
- Convert 3D objects into brush stamps
- Alphas affect both sculpt depth AND paint opacity simultaneously
- Alpha rotation modes: Fixed angle, Follow stroke direction, Follow pen tilt, Random per dab
- In 3DCoat 2024: VDM (Vector Displacement Map) brushes via EXR import — full 3D displacement including undercuts

### Strips

**Repeating textures along the stroke path.** Unlike alphas (which stamp once per dab), strips tile continuously.

- Used for: stitching, zippers, chains, rope, scales, ornamental borders
- Parameters: scale, depth, spacing, rotation along path
- Best in Paint Room for normal map depth detail
- Works with per-pixel painting modes and microvertex displacement

### Stencils

**Screen-space masking images** that constrain where the brush applies.

- Load any image as a stencil
- Stencil masks all active channels simultaneously
- Interactively move, rotate, scale (T, R, S keys)
- Multiple stencils can be loaded
- Grayscale: white = allowed, black = masked, grey = partial
- Stencil stays fixed in screen space while the model rotates beneath it

### Stroke Modes (E-Panel)

The E-Panel defines **how the brush draws** — the geometric mode of stroke application:

| Mode | Description |
|------|-------------|
| **Freehand (Dots)** | Standard freehand drawing — dabs placed along cursor path |
| **Line** | Straight line from click to release |
| **Rectangle Lasso** | Paint/sculpt within a rectangular region |
| **Ellipse Lasso** | Paint/sculpt within an elliptical region (modifier key for perfect circle) |
| **Vertex Lasso** | Click to add vertices, double-click to close — irregular polygon region |
| **Closed Spline** | Draw a closed spline shape, apply operation inside. Close with Esc |
| **Curve / Spline** | Draw along a user-defined spline path |
| **Stamp / Single Dab** | One dab per click |
| **Drag Rectangle** | Drag to define a rectangle |
| **Pressure modes** | Various tablet pressure response modes |

### Conditions (Height/Color Limiter)

A **masking system** that restricts brush effect based on surface properties. This is one of 3DCoat's most powerful and distinctive features:

| Condition | Description |
|-----------|-------------|
| **Curvature** | Paint more in cavities or more on convex surfaces |
| **Ambient Occlusion** | Paint more in occluded crevices |
| **Height** | Paint more on peaks or in valleys |
| **Color** | Paint only on/near a specific existing color |
| **Mask / Freeze** | Paint only on/outside a painted mask |

A condition mask can combine multiple factors. The system controls how depth, color, and glossiness are affected by surface conditions.

**Important UX note:** The documentation warns "Remember to set to Always when you finish using this option" — the condition persists across strokes until manually cleared.

### Brush Options Panel

Core parameters available on most brushes:

| Parameter | Description |
|-----------|-------------|
| **Radius / Size** | Brush radius (screen-space or world-space option) |
| **Depth / Intensity** | Strength of sculpting or painting effect |
| **Opacity** | Transparency of paint strokes |
| **Smoothing** | Built-in smoothing applied per-stroke |
| **Focal Shift** | Shifts the falloff center point. Negative = sharp (more full-intensity area, steep drop at edge). Positive = soft/gradual (airbrush-like). Continuous, not binary |
| **Hardness** | How sharp the brush edge is |
| **Spacing** | Distance between dabs along a stroke |
| **Accumulate** | Whether effect accumulates within a single stroke |
| **Backface Culling** | Ignore back-facing polygons |
| **Autosmooth** | Automatic smoothing during sculpting |
| **Steady Stroke / Lazy Mouse** | Stabilizes input with lag for smoother strokes |
| **Jitter** | Randomizes position/size/rotation of dabs |
| **Invert (Ctrl)** | Inverts operation (add ↔ subtract) |

### Falloff Curve Editor

Spline-based curve editor defining radial falloff:
- X axis = distance from brush center (0=center, 1=edge)
- Y axis = brush strength
- Bezier tangent handles for smooth curves
- Built-in presets: Smooth (bell), Constant, Linear, Sharp, Pinch (ring), Needle
- Custom curves saveable as presets
- Per-brush curves (each brush can have its own falloff)
- Same curve system applies to pressure-to-size, pressure-to-opacity, pressure-to-depth mappings

---

## 4. Dynamics / Pressure System

3DCoat's dynamics system is **simpler than Krita's 16-sensor system** but covers the essential tablet inputs.

### Supported Inputs

| Input | Usage |
|-------|-------|
| **Pressure** | The primary dynamic. Separate pressure curves for depth, opacity, and smoothing |
| **Tilt** | Alpha rotation and brush shape modulation. When enabled, alpha rotates to follow pen tilt direction |
| **Pen Rotation (barrel)** | Supported on compatible tablets (Wacom Art Pen, etc.) |
| **Velocity / Speed** | Can modulate certain parameters, less prominently featured than in Krita |

### Pressure Curve System

- Spline-based curve editor mapping raw tablet pressure → output intensity
- Separate curves for: Depth, Opacity, Smoothing
- Global pressure response dialog (Edit menu) — global mapping for all brushes
- Individual brush presets can override the global curve
- Standard Bezier control point editing

### Tablet Support (2025)

Supported: Wacom, Huion, XP-Pen, Microsoft Surface Pro. Xencelabs not mentioned in documentation.

### What's Missing vs. Krita

- No "Fuzzy Dab" or "Fuzzy Stroke" (per-dab/per-stroke randomization sensors)
- No "PressureIn" (max pressure in stroke)
- No "Distance" or "Fade" (dab-count-based dynamics)
- No "Perspective" sensor
- No "Tangential Pressure" (airbrush wheel)
- No periodic/cycling on distance/time sensors
- No additive vs multiplicative sensor composition modes
- No arbitrary sensor-to-parameter mapping — the mapping is mostly fixed (pressure → depth/opacity/smoothing)

The dynamics system is more "Photoshop-like" than "Krita-like" — adequate for professional work but less deep for artists who want per-parameter custom sensor mappings.

---

## 5. Smart Materials

Smart Materials are **layered procedural PBR material definitions** applied through the brush system. This is the feature that most creates the perception of "composable brushes."

### Architecture

A Smart Material is a **saved layer stack** of procedural texture layers:
1. Each layer can use procedural textures (noise, patterns) or imported images
2. Each layer has a blend mode, opacity, and per-channel settings
3. Layers can affect different channels: color, depth, glossiness, metalness
4. Layers can be driven by **condition masks** (curvature, AO, height, color)
5. The stack is resolution-independent for procedural layers

### How They Work

```
Smart Material "Worn Metal"
├── Layer 1: Base metal color + roughness
├── Layer 2: Edge wear (driven by curvature mask, reduces metalness on edges)
├── Layer 3: Cavity dirt (driven by AO mask, darkens color in crevices)
├── Layer 4: Scratches (noise-based, adds roughness variation)
└── Layer 5: Dust (height-based, accumulates on upward-facing surfaces)
```

When applied via brush:
- The brush alpha/pressure/dynamics control **WHERE** the material is applied
- The Smart Material controls **WHAT** is applied (the complex multi-channel material)
- This creates a **two-level composition**: brush dynamics control spatial application, Smart Material controls content

### Procedural Components

- Noise generators (Perlin, Voronoi, cellular, fractal)
- Curvature-based masking (dirt in crevices, wear on edges)
- Ambient Occlusion-based masking
- Height/slope-based masking
- World-space or UV-space projection
- Layered compositing of all the above

### Smart Material Presets

- Stored in `.3dcpack` file format
- Ship with a built-in library (metal, wood, stone, fabric, etc.)
- Users can create and share custom Smart Materials
- Online library of community smart materials
- Can be applied as: fill (entire object), brush (paint on), or layer (non-destructive)

### Smart Material Layers (Non-Destructive)

A paint layer can be a Smart Material fill layer:
- Procedural, non-destructive, re-parameterizable
- Updates automatically when the mesh changes
- Parameters can be tweaked after application

---

## 6. GPU Acceleration

3DCoat has been **one of the earliest 3D apps to move core sculpting operations to the GPU**, predating much of the industry.

### Voxel Sculpting GPU Pipeline

1. SDF stored in **sparse octree** (only occupied/near-surface voxels)
2. Brush operations modify SDF values in the affected region
3. **Surface extraction** (Marching Cubes / Surface Nets) polygonizes the SDF for display — runs on GPU
4. Only the **modified region** of the octree is re-extracted (incremental)
5. Historically OpenCL, transitioning to Vulkan compute in 2021+ versions

The re-meshing after each sculpt stroke is the key performance bottleneck. 3DCoat optimizes by limiting re-extraction to the brush-affected region.

### Paint Room GPU Pipeline

- Texture painting uses **GPU-accelerated projection painting**
- Brush strokes rendered as screen-space operations, then projected onto UV-mapped textures
- Depth channel uses parallax/relief mapping for real-time preview without actual geometry displacement
- Texture data lives on GPU (as OpenGL/Vulkan textures) and CPU (for undo, save)
- Layer compositing for PBR channel stack is GPU-accelerated

### Rendering

- PBR viewport with IBL (image-based lighting), real-time shadows
- Matcap display mode for sculpting
- Matches output to game engine conventions (Unity, Unreal material models)
- FPS monitor available in viewport (new in 2025)

---

## 7. Layer System

### Paint Layers

Photoshop-style layer system for texture painting:

| Feature | Description |
|---------|-------------|
| **Layer opacity** | Overall transparency |
| **Blend modes** | Standard modes (Normal, Multiply, Screen, Overlay, etc.) |
| **Layer lock** | Lock transparency, painting, position |
| **Layer visibility** | Toggle on/off |
| **Layer depth** | Each paint layer carries its own depth/normal channel |
| **Groups / Folders** | Organize layers hierarchically |
| **Clipping masks** | Clip layer to alpha of layer below |
| **Layer masks** | Per-layer masks controlling visibility |
| **Smart Material layers** | Non-destructive procedural fill layers |

### Sculpt Layers

In surface sculpting mode:
- Separate displacement storage per layer
- Adjustable layer opacity (blend displacement amount)
- Merge, duplicate, reorder layers

### Interaction with Brushes

- Brushes always paint onto the currently selected layer
- Layer blend mode affects how strokes composite
- Layer opacity scales brush output
- Lock transparency prevents brush from painting outside existing alpha

---

## 8. Symmetry

| Mode | Description |
|------|-------------|
| **Mirror X/Y/Z** | Mirror across any axis plane |
| **Mirror XY/XZ/YZ/XYZ** | Multi-axis combinations (up to 8-way with all three) |
| **Radial** | N-fold rotational symmetry (2, 3, 4, 5, 6, 8, 12, etc.) |
| **Mirror + Radial** | Both simultaneously |
| **Topological** | Symmetry based on mesh topology, not world space — works even on asymmetric meshes with symmetric topology |
| **Local** | Symmetry around object's local axis rather than world axis |
| **Symmetry plane visualization** | Visual guide showing the symmetry plane |

---

## 9. Brush Preset Format & Import/Export

### Preset Storage

- Brush presets stored as proprietary files in 3DCoat's user data directories
- `.3dcpack` is the distribution format for sharing brush/material packages
- "Penpack" format also supported for brush packages
- Presets include: alpha image reference, all parameter settings, curves, conditions
- Organized by category/room in a palette/panel system
- **No public specification** for the preset format (unlike Krita's documented `.kpp`)

### Import Formats

| Format | What It Imports |
|--------|----------------|
| **PNG, TGA, BMP, TIF** | Brush alphas / tip shapes |
| **PSD** | Brush alphas with layer support |
| **EXR, HDR** | Brush alphas, VDM brushes (2024+) |
| **ABR (Photoshop)** | Brush tips — imported as alphas. Dynamics don't fully transfer |
| **3D mesh files** | 3D brush stamps for voxel sculpting |

### Export

- All painted channels export as separate image files (PNG, TGA, PSD, EXR)
- Export presets for different engines (Unity, Unreal, etc.)
- PSD export with layers preserved
- UDIM tile support for high-res assets
- Texture baking from high-poly to low-poly (normal, AO, curvature, color, displacement, thickness)

---

## 10. The "Composable Brush Engine" — Analysis

### The Claim

Krita artists cite 3DCoat's engine as inspiration for a composable/stackable system where you can "apply blur + smudge + texture in one stroke." This is the claim we need to evaluate.

### What 3DCoat Actually Does

The "composability" in 3DCoat comes from **three features working together**, none of which is a composable brush pipeline in the technical sense:

**1. Multi-Channel Painting (the core illusion of composability)**
A single brush stroke writes to color + depth + roughness + metalness simultaneously, each at independently controlled intensities. This **feels** composable because a single stroke produces a complex, multi-faceted result. But architecturally, it's one engine writing to multiple render targets — not multiple engines chained together.

**2. Smart Materials (the closest analog to composable effects)**
A Smart Material is a pre-composed layer stack of procedural effects (curvature-driven dirt, edge wear, noise-based scratches). Applied through the brush, it delivers a complex multi-effect result. But the composition happens at material-definition time, not at brush-engine runtime. You can't dynamically add "blur this stroke" as a stage.

**3. Conditions Limiter (additive masking)**
The curvature/AO/height/color condition system restricts **where** the brush applies. This adds spatial intelligence to strokes. But it's a masking system, not an effect pipeline.

### What 3DCoat Does NOT Have

- **No stackable brush effects** — you cannot chain "blur + smudge + texture" as a pipeline within a single brush
- **No node-based brush system** — it's parameter-panel based (though 2025 added a Node Room for sculpt materials, not brush composition)
- **Multi-channel painting is fixed to PBR channels** — not arbitrary user-defined effect stages
- **No per-stage dynamics** — you can't have "pressure controls color opacity, tilt controls roughness amount, speed controls displacement depth" as independently configured per-channel dynamics

### The Idealized System (What Artists Actually Want)

```
Stroke Input
  → Effect Stage 1: Color (with its own dynamics: pressure→opacity, tilt→hue)
  → Effect Stage 2: Smudge/Blend (with its own dynamics: speed→smudge length)
  → Effect Stage 3: Texture Overlay (with its own dynamics: pressure→depth)
  → Effect Stage 4: Post-processing (blur, sharpen — with its own dynamics)
  → Final Composite
```

3DCoat's approach is **pragmatic rather than architecturally composable**: it achieves complex results through multi-channel output and Smart Materials rather than through a generalized effect pipeline. The Krita community's desire for such a system is real, but 3DCoat is not the reference architecture for it.

---

## 11. Recent Developments (2024–2025)

### 3DCoat 2024

- **Vector Displacement Map (VDM) brushes** — EXR-based 3D displacement brushes capturing undercuts and overhangs. Library of VDM brushes provided in Alphas panel
- **Live Booleans** — procedural boolean operations for modeling
- **Layer masks + Clipping masks** — Photoshop-style layer composition in Paint Room
- **Performance improvements** — GPU painting, faster voxel operations
- **UI modernization** — improved panels and workflows

### 3DCoat 2025

- **Node Room** — non-destructive material authoring using node-based workflows for sculpt materials. Includes a built-in editor using GLSL-adjacent language for creating custom nodes. This is significant — it's 3DCoat's first step toward node-based composition, though for materials rather than brushes
- **Soft Booleans for Voxels** — boolean operations with automatic edge beveling. Brush radius can define bevel radius
- **Smart Hybrid** — creates NURBS-like smooth patches from low-poly meshes
- **Surface Array** — duplicate objects along surfaces or selected faces
- **Expanded tablet support** — Huion, XP-Pen, Microsoft Surface Pro
- **Hotkey Manager** — improved shortcut customization
- **18 UI themes** — visual customization
- **Non-Modal Search** — filter alphas, materials, objects, layers, presets simultaneously
- **Photogrammetry integration** — RealityCapture integration

---

## 12. Relevance to Darkly

### What to Learn From 3DCoat

1. **Multi-channel output is the real "composability" artists want.** A single brush stroke that affects color + depth/normal + roughness feels incredibly expressive. For Darkly's 2D context, the analog would be a brush that simultaneously affects color, opacity, and filter parameters (blur, texture, etc.) with independent per-channel intensity controls.

2. **The Conditions system is brilliant.** Masking brush effects by surface properties (curvature, height, existing color) enables incredibly targeted painting. For a 2D engine, similar conditions could be: paint more in dark areas, paint less where there's already paint, modulate by distance from edge, etc.

3. **Smart Materials prove that pre-composed effect stacks work.** Artists don't need full real-time composability — saved, parameterizable effect recipes are sufficient and more practical. A preset that says "apply grunge: dark in crevices, highlight on edges, noise in flat areas" is more useful than a general-purpose node graph.

4. **Strips (repeating textures along stroke path) are universally useful.** Chains, stitches, borders, rope — this is a simple feature that adds enormous value.

5. **3D brush stamps (mesh-as-alpha) are novel but 3D-specific.** Not directly relevant to Darkly's 2D context.

### What to Avoid

1. **The workspace-segregated architecture.** Having fundamentally different brush engines per workspace leads to inconsistent behavior and duplicated effort. A unified brush engine that works across contexts is better.

2. **The undocumented preset format.** 3DCoat's proprietary, undocumented format makes presets opaque. Darkly should have a transparent, well-documented preset format.

3. **The "jack of all trades" positioning.** 3DCoat tries to be everything — sculpting, retopo, UV, painting, modeling — and masters none of them enough to be the industry standard in any category. Better to excel at one thing.

4. **The weak dynamics system.** 3DCoat's pressure/tilt/speed mapping is adequate but not deep. Krita's per-parameter sensor system with custom curves is far more expressive. Darkly should aim for Krita-level dynamics depth.

### The Key Insight

The feature that makes artists say "composable" about 3DCoat is **not** architectural composability — it's **multi-faceted output from a single gesture**. The single most impactful thing Darkly can do is ensure that a brush stroke can simultaneously affect multiple properties (color, blur, texture, displacement) with independent control over each. This is achievable without a complex node-based composition system — it just requires multiple output channels on the brush engine with per-channel parameter control.
