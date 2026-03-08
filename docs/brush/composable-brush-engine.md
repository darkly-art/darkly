# Composable Brush Engine — Design

## Motivation

No open source project has shipped a composable brush engine — one where independent effect stages stack into a single-stroke pipeline. The concept is well-understood and widely desired, but every existing open source painting tool uses monolithic brush engines:

- **Krita** — 16 isolated "snowflake" engines, each a hardcoded bundle of behaviors. Want smudging with hatching? Can't.
- **libmypaint** — single atomic composite function. Parametrically rich, but no chaining.
- **Blender sculpt** — discrete per-type units, explicitly traded flexibility for performance.
- **PixiEditor 2.1** — node graph for brush *definition* (dab placement, sizing), but not for effect *stacking*.
- **Our Paint** — node-based dab control with GPU rendering, but niche and sparsely documented.

The Krita community has explicitly requested this architecture: a feature request titled **"Universal Brush Engine / Arbitrary Brush Stacks"** ([krita-artists.org](https://krita-artists.org/t/universal-brush-engine-arbitrary-brush-stacks/46729)) proposes decomposing Krita's isolated engines into modular components that can be sequentially stacked (e.g., blur → smudge → RGBA brush), with each stage operating on the output of the previous one. It remains unimplemented.

The fully composable vision exists only in closed-source commercial tools:
- **3DCoat** — multi-channel PBR output + conditions masking + smart materials. What *feels* composable is actually multi-faceted output, not a stackable pipeline (see [3DCoat analysis](3dcoat-brush-system.md)).
- **Black Ink** — the gold standard. Full node graph + Brush Shader Language (BSL), runs almost entirely on GPU, supports canvases up to 65K×65K. Entirely proprietary.

### What Artists Actually Want

Distilled from community discussions, pain points, and the Krita feature request:

1. **Combine behaviors freely** — "apply blur + smudge + texture in one stroke" without switching engines
2. **Multi-channel output** — a single stroke affecting color, opacity, and potentially displacement/roughness simultaneously, with independent per-channel intensity
3. **Deep dynamics** — every parameter mappable to tablet input via custom transfer curves
4. **Great defaults** — ship with presets that rival Krita's David Revoy packs and Photoshop's built-in brushes
5. **Easy preset creation** — build custom brushes by assembling stages, not by writing code
6. **Shareable presets** — single self-contained files, no external dependencies

---

## Lessons from Prior Art

### Krita — Dynamics Excellence, Engine Fragmentation

**Copy:** The dynamics system. 16 sensors (pressure, tilt, speed, distance, rotation, fade, fuzzy…) with per-parameter cubic transfer curves. The `ValueComponents` accumulation model (multiplicative × scaling + additive offsets) is elegant and expressive. This is the best input→parameter mapping system in any open source tool.

**Avoid:** The "snowflake" architecture. 16 separate `KisPaintOp` subclasses with inconsistent feature parity. Adding a new capability means either modifying an existing engine (breaking encapsulation) or creating engine #17 (fragmentation). The `KisBrushBasedPaintOp` base class shares *some* code, but fundamental behaviors (smudging, filtering, deformation) can't be mixed.

**Avoid:** CPU-only rendering. Krita's brush pipeline is entirely CPU-bound. Large brushes on large canvases lag badly, and GPU acceleration is blocked on their Qt6 port with "all signs point to no" for years. We start GPU-first.

See [Krita Brush System — Deep Dive](krita-brush-system.md).

### 3DCoat — Multi-Channel is the Real Composability

**Copy:** Multi-channel output. A single brush stroke writing to color + depth + roughness + metalness simultaneously, with independent per-channel intensity. This is what most artists *mean* when they say "composable brushes" — not pipeline stacking, but multi-faceted output from a single gesture.

**Copy:** Conditions masking. Restricting brush effects by surface properties (curvature, ambient occlusion, height, existing color) creates incredibly targeted painting. For 2D: modulate by existing paint density, distance from edge, local luminance, local color similarity.

**Copy:** Smart Materials as a concept — pre-composed, parameterizable effect recipes that artists can save and share. This maps directly to our preset system.

**Avoid:** Workspace segregation. 3DCoat's "rooms" system means the brush system is really multiple separate systems sharing a UI skin. We want one unified engine.

See [3DCoat Brush System — Deep Dive](3dcoat-brush-system.md).

### Black Ink — The Gold Standard (Closed Source)

Black Ink proves the architecture works: every brush parameter driven by a visual node graph ("Controllers"), a shader language (BSL) for fully custom computation, and the whole thing runs on GPU. It handles 65K×65K canvases smoothly. This is the north star, even though we can't study its implementation.

Key takeaway: the power of Black Ink isn't just that it *has* a node graph — it's that every brush aspect is programmable. Users create brushes that couldn't exist in any fixed engine. Our linear pipeline should be designed so that exposing a graph later is an evolution, not a rewrite.

### ABR Import — A Cautionary Tale

Photoshop's .abr format is a reverse-engineering nightmare. Every tool that imports ABR (Krita, GIMP, community parsers) handles only a fraction of the format. Krita imports *only brush tips* — all dynamics, parameters, patterns, and presets are silently discarded. This is one of the most common support complaints across every non-Adobe painting tool.

**Lesson:** Our preset format must be open, documented, self-contained, and human-readable. We should also plan an ABR import path that creates proper Darkly presets with best-effort parameter mapping, not just tip extraction.

See [ABR Format — Reverse Engineering Analysis](abr-format.md).

---

## What "Composable" Means

A brush is an **ordered pipeline of independent stages**. Each stage is a GPU pass that generates, transforms, or blends pixel data. Stages can be freely combined — an artist assembles their brush by choosing which stages to include and in what order.

```
Source Stage → [Transform Stages...] → [Blend Stage] → Output
     │               │                       │            │
  Generates       Modifies the            Mixes with     Writes to
  the dab         dab image               canvas data    target channels
```

This is fundamentally different from:
- **Monolithic engines** (Krita) — where each engine is a frozen bundle of behaviors
- **Workspace segregation** (3DCoat) — where different tools have completely separate brush systems
- **Parametric-only composability** (libmypaint) — where inputs are composable but the pipeline is atomic

Each stage is self-contained: its own parameters, its own dynamics bindings, its own GPU shader. The pipeline assembles them. Adding a new stage means creating one file — no central dispatcher, no match arms, no registry edits.

### Pipeline Model

**Linear pipeline, designed for future graph extension.** Stages execute in order, each receiving the output of the previous stage (or the canvas data, depending on stage type). This is simpler to implement, simpler for users to understand, and matches the mental model of "apply this, then this, then this."

The underlying data model should use stage IDs and explicit input references rather than array indices, so that evolving to a DAG (directed acyclic graph) later requires changing the *editor UI* and the *scheduler*, not the stage system itself.

### Stage Types

| Type | Role | Input | Output | Example |
|------|------|-------|--------|---------|
| **Source** | Generates the initial dab | Dab position + dynamics | RGBA dab buffer | Stamp, Procedural Shape, Noise |
| **Transform** | Modifies the dab | RGBA dab buffer | RGBA dab buffer | Blur, Sharpen, Texture Overlay, Scatter, Warp |
| **Blend** | Mixes dab with canvas data | RGBA dab + canvas region | RGBA dab buffer | Smudge, Color Mix, Paint Thickness |
| **Output** | Writes to target channels | RGBA dab buffer | Channel writes | Color, Opacity, Displacement |

Source stages come first. Transform stages chain after. Blend stages read from both the dab and the canvas. Output stages write to one or more channels.

### GPU Execution

Each stage is a GPU compute or render pass. Between stages, dab data lives in a ping-pong buffer pair (the same pattern Darkly already uses for filter/veil effects). The pipeline:

1. Allocates dab-sized ping-pong textures (sized to current brush diameter)
2. Source stage writes to buffer A
3. Transform stages alternate: read A → write B, read B → write A, …
4. Blend stage reads canvas region + current buffer → writes result
5. Output stage composites onto canvas tile(s)

For brushes with no transform or blend stages (simple stamp), the pipeline collapses to source → output with zero intermediate passes.

---

## Architecture

### Stage System — Following the Modularity Pattern

Brush stages follow the same auto-discovery pattern as filters and veils:

```
crates/darkly/src/gpu/
├── brush.rs                    # BrushStage trait, BrushPipeline, BrushStageRegistry
├── brush_stages/
│   ├── mod.rs                  # @generated by build.rs
│   ├── stamp.rs                # Image tip stamping (Source)
│   ├── procedural.rs           # Circle/rect/gaussian generation (Source)
│   ├── scatter.rs              # Positional scatter (Transform)
│   ├── blur.rs                 # Gaussian blur on dab (Transform)
│   ├── texture_overlay.rs      # Per-dab or per-stroke texture (Transform)
│   ├── smudge.rs               # Canvas color blending (Blend)
│   └── color_output.rs         # Write to color channel (Output)
```

Each stage exports `register() -> BrushStageRegistration`:

```rust
pub struct BrushStageRegistration {
    pub type_id: &'static str,
    pub stage_type: StageType,           // Source, Transform, Blend, Output
    pub params: &'static [ParamDef],
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    pub from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn BrushStage>,
}
```

`BrushStageRegistry` mirrors `FilterRegistry` — HashMap-backed, lazy pipeline caching, auto-populated from generated `brush_stages::registrations()`.

### The BrushStage Trait

```rust
pub trait BrushStage: std::fmt::Debug {
    fn type_id(&self) -> &'static str;
    fn stage_type(&self) -> StageType;
    fn clone_boxed(&self) -> Box<dyn BrushStage>;
    fn pipeline(&self) -> &wgpu::RenderPipeline;
    fn bind_group_layout(&self) -> &wgpu::BindGroupLayout;

    /// Encode this stage's GPU pass.
    /// `dab_src` / `dab_dst`: ping-pong dab textures
    /// `canvas_region`: read-only view of canvas under the dab (for Blend stages)
    /// `dynamics`: evaluated sensor values for this dab
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        dab_src: &wgpu::TextureView,
        dab_dst: &wgpu::TextureView,
        canvas_region: &wgpu::TextureView,
        dynamics: &DynamicsSnapshot,
        cache: &mut StageCache,
    );

    /// Create per-instance GPU resources (uniform buffers, bind groups, aux textures).
    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        max_dab_size: u32,
    ) -> StageCache;
}
```

### BrushPipeline — Assembling Stages

A `BrushPipeline` is a concrete brush: an ordered list of stages with their dynamics bindings. It is the runtime representation of a preset.

```rust
pub struct BrushPipeline {
    pub name: String,
    pub stages: Vec<BoundStage>,
    pub spacing: SpacingConfig,
    pub smoothing: SmoothingConfig,
}

pub struct BoundStage {
    pub stage: Box<dyn BrushStage>,
    pub dynamics: Vec<ParameterBinding>,  // per-parameter sensor assignments
}
```

At stroke time, for each dab position:
1. Evaluate all dynamics sensors against current `PaintInformation`
2. Apply transfer curves → `DynamicsSnapshot`
3. Execute each stage's `encode()` in order
4. Submit the command buffer

### Initial Built-in Stages

**Source stages:**
- **Stamp** — image-based brush tips. Loads GBR, PNG, or imported ABR tips. Supports scale, rotation, subpixel offset via mip-mapped texture pyramid. Coloring modes: tint (grayscale → paint color), lightness-preserving, gradient-mapped, source-device.
- **Procedural** — circle, rectangle, gaussian, curve-defined falloff. SIMD-style GPU generation, no intermediate bitmap. Parameters: radius, aspect ratio, softness/fade, spike count, anti-aliasing.

**Transform stages:**
- **Blur** — gaussian blur on the dab buffer. Parameters: radius (driven by dynamics).
- **Texture Overlay** — multiplies a texture pattern onto the dab. Two modes: per-dab (texture anchored to dab center, tiling) and per-stroke (texture anchored to canvas coordinates, the mode Krita is missing). Parameters: scale, strength, mode.
- **Scatter** — offsets the dab position randomly within a configurable radius. Parameters: amount (driven by dynamics), angular distribution.

**Blend stages:**
- **Smudge** — blends paint color with canvas pixels under the dab. Parameters: smudge length (0 = pure paint, 1 = pure pickup), dulling mode vs smear mode. This is the Color Smudge engine's core behavior, isolated as a composable stage.
- **Color Mix** — mixes the dab color with previous dab color for wet-paint blending. Parameters: mix ratio, paint persistence across dabs.

**Output stages:**
- **Color Output** — writes the dab to the color channel with composite op, opacity, flow. This is the default and most common output stage.
- **Opacity Output** — writes to an opacity/alpha channel independently. For effects where opacity varies independently from color.

---

## Dynamics System

### Sensors

Matching Krita's proven 16-sensor set, with room for extension:

| Sensor | Source | Accumulation | Notes |
|--------|--------|-------------|-------|
| Pressure | Tablet pressure (0–1) | Multiplicative | The fundamental sensor |
| PressureIn | Max pressure so far in stroke | Multiplicative | For build-up effects |
| Speed | Drawing speed (normalized) | Multiplicative | |
| Distance | Distance along stroke | Multiplicative | Supports length + periodic |
| Fade | Dab count | Multiplicative | Ink depletion, trail fade |
| Time | Elapsed time | Multiplicative | Supports length + periodic |
| Rotation | Tablet barrel rotation | Additive | |
| Drawing Angle | Stroke direction | Absolute | Locked-angle mode, fan corners |
| X-Tilt | Pen tilt X axis | Multiplicative | |
| Y-Tilt | Pen tilt Y axis | Multiplicative | |
| Tilt Direction | Derived tilt azimuth | Additive | |
| Tilt Elevation | Derived tilt altitude | Multiplicative | |
| Perspective | Perspective grid value | Multiplicative | For perspective-aware painting |
| Tangential Pressure | Airbrush wheel | Multiplicative | |
| Fuzzy Dab | Random per dab | Additive | |
| Fuzzy Stroke | Random per stroke | Additive | |

### Parameter Binding

Each stage parameter can be independently bound to one or more sensors. A `ParameterBinding` holds:

```rust
pub struct ParameterBinding {
    pub param_name: &'static str,
    pub base_value: f32,                        // the "constant" in Krita terms
    pub sensor_curves: Vec<SensorCurve>,
}

pub struct SensorCurve {
    pub sensor: SensorId,
    pub curve: CubicCurve,                      // 256-point transfer function
    pub accumulation: Accumulation,             // Multiplicative, Additive, Absolute
}
```

Evaluation follows Krita's proven `ValueComponents` model:
1. Each sensor evaluates to a raw 0–1 value
2. The value is mapped through the cubic transfer curve
3. Results accumulate: `scaling` (product of multiplicative), `additive` (sum of additive), `absolute_offset`
4. Final value: `base_value * scaling + base_value * additive + absolute_offset`, clamped to param range

### PaintInformation

The per-dab data packet, constructed from input events:

```rust
pub struct PaintInformation {
    pub position: Vec2,
    pub pressure: f32,
    pub x_tilt: f32,
    pub y_tilt: f32,
    pub rotation: f32,                 // barrel rotation
    pub tangential_pressure: f32,      // airbrush wheel
    pub time: f64,
    pub speed: f32,
    pub drawing_distance: f32,
    pub drawing_angle: f32,
    pub dab_index: u32,
    pub max_pressure_in_stroke: f32,
}
```

---

## Dab Placement & Stroke Rendering

### Spacing

`SpacingConfig` controls dab density along the stroke:

- **Proportional spacing** — distance between dabs as a fraction of brush diameter (default 0.25 = 25% spacing, standard in most tools)
- **Auto-spacing** — adjusts based on brush size: `coeff * (size < 1.0 ? size : sqrt(size))`
- **Anisotropic spacing** — X/Y spacing can differ, rotated by a spacing ellipse angle

### Stroke Interpolation

Between raw input events:
1. Construct `PaintInformation` for each event
2. Interpolate between consecutive events to find dab positions using spacing distance
3. At each dab position, mix `PaintInformation` (linear interpolation of pressure, tilt, etc.)
4. For Bezier input (common on tablets): subdivide at midpoint until flat (threshold 0.5), then linear

### Smoothing / Stabilizer

Weighted moving average over the last N input positions. Three modes:
- **None** — raw input, no filtering
- **Basic** — simple moving average of positions
- **Weighted** — exponentially weighted, more recent positions have more influence. Configurable window size and weight decay.

This is an area where Krita is notably weak ("stabilizes but doesn't actually smooth"). We should study CSP and SAI's stabilizer implementations, which are considered the gold standard for line smoothing.

---

## Preset System

### Format — `.darkly-brush`

A self-contained file with a human-readable header and binary resource payloads:

```
┌─────────────────────────────────┐
│  Magic: "DKBR"                  │
│  Version: u16                   │
│  Header length: u32             │
├─────────────────────────────────┤
│  JSON header:                   │
│  {                              │
│    "name": "Oil Thick Flat",    │
│    "version": 1,                │
│    "pipeline": [                │
│      {                          │
│        "stage": "stamp",        │
│        "params": {              │
│          "tip": "res://flat.png"│
│        },                       │
│        "dynamics": {            │
│          "scale": {             │
│            "base": 1.0,         │
│            "sensors": [{        │
│              "id": "pressure",  │
│              "curve": "...",    │
│              "mode": "mult"     │
│            }]                   │
│          }                      │
│        }                        │
│      },                         │
│      {                          │
│        "stage": "texture",      │
│        "params": {              │
│          "texture":"res://c.png"│
│          "mode": "per_stroke",  │
│          "strength": 0.3        │
│        }                        │
│      },                         │
│      {                          │
│        "stage": "smudge",       │
│        "params": {              │
│          "length": 0.4,         │
│          "mode": "dulling"      │
│        }                        │
│      },                         │
│      {                          │
│        "stage": "color_output", │
│        "params": {              │
│          "opacity": 0.85,       │
│          "flow": 0.7,           │
│          "composite": "normal"  │
│        },                       │
│        "dynamics": {            │
│          "opacity": {           │
│            "base": 0.85,        │
│            "sensors": [{        │
│              "id": "pressure",  │
│              "curve": "0,0;0.3,│
│                0.8;1,1",        │
│              "mode": "mult"     │
│            }]                   │
│          }                      │
│        }                        │
│      }                          │
│    ],                           │
│    "spacing": 0.25,             │
│    "smoothing": "weighted",     │
│    "resources": {               │
│      "flat.png": {              │
│        "offset": 0,             │
│        "size": 4096,            │
│        "hash": "sha256:..."     │
│      },                         │
│      "canvas.png": {            │
│        "offset": 4096,          │
│        "size": 8192,            │
│        "hash": "sha256:..."     │
│      }                          │
│    }                            │
│  }                              │
├─────────────────────────────────┤
│  Binary resource blob:          │
│  [flat.png bytes][canvas bytes] │
└─────────────────────────────────┘
```

**Design principles:**
- **Self-contained** — all resources embedded. No "missing brush tip" after sharing. (Lesson from Krita's v2.2 → v5.0 migration and ABR's chronic issues.)
- **Human-readable header** — JSON pipeline definition can be inspected, hand-edited, diffed.
- **Version-stamped** — forward compatibility via version field. Unknown fields are preserved, not discarded.
- **Content-addressed resources** — SHA-256 hash deduplication. If two presets use the same tip image, the resource is stored once in the user's cache.
- **Compact** — binary resource blob avoids base64 bloat (Krita's approach of base64-in-XML is ~33% overhead).

### Preset Creation

**For us (shipping defaults):** Curate a set of well-designed presets spanning core use cases:
- Natural media (pencil, charcoal, oil, watercolor, ink)
- Digital art (hard round, soft round, airbrush, flat shader)
- Effects (scatter, splatter, texture stamps, glow)
- Utility (eraser, blender, smudge)

Each default preset is a pipeline of stages with carefully tuned dynamics curves. Presets should be **opinionated** — a good pencil preset shouldn't require tweaking to feel like a pencil.

**For users (building their own):** A visual pipeline editor:
- Drag stages from a palette into an ordered list
- Per-stage parameter sliders with live preview
- Per-parameter dynamics curves (draw or select from templates: linear, S-curve, exponential, step)
- Sensor assignment per parameter (dropdown of available sensors)
- Save/load/share as `.darkly-brush` files

The visual editor is a frontend concern. The Rust engine needs to support:
- Creating a `BrushPipeline` from a serialized preset definition
- Hot-swapping stages at runtime (for live preview during editing)
- Serializing a `BrushPipeline` back to the preset format

### ABR Import Path

Based on our [ABR format research](abr-format.md):

1. **Brush tips** — extract sampled brushes from ABR v1/v2/v6, convert grayscale→RGBA, create Stamp source stages
2. **Patterns** — extract pattern data from v10 descriptor sections when present
3. **Parameters** — parse ActionDescriptor blocks for spacing, angle, roundness, scatter, and map to closest Darkly pipeline equivalents
4. **Generate presets** — each imported ABR brush becomes a `.darkly-brush` preset with best-effort parameter mapping, not just a raw tip image

This won't achieve perfect Photoshop fidelity (ABR's format is too opaque and version-fragmented), but it will be dramatically better than Krita's tips-only import.

---

## Conditions Masking (Future Extension)

Borrowed from 3DCoat, conditions masking restricts where a brush takes effect based on local properties. In a 2D context:

- **Existing paint density** — only paint on empty areas, or only on already-painted areas
- **Local luminance** — restrict to highlights or shadows
- **Local color similarity** — paint only on areas close to a target color
- **Edge distance** — modulate near edges of existing paint regions

This would be implemented as a special Transform stage type that generates a mask from canvas analysis, then multiplies it onto the dab. Not in the initial system, but the architecture should not preclude it.

---

## Implementation Phases

This system will be built incrementally, with human feedback checkpoints between phases. The core building blocks must feel right before we build on top of them — a smudge stage is worthless if the underlying dab placement feels mechanical, and presets are meaningless if individual stages don't produce convincing results. Each phase ends with a hands-on testing session where strokes are evaluated for naturalness, responsiveness, and visual quality.

### Phase 1 — Pipeline Skeleton + Round Brush

**Goal:** A basic round brush that paints on the canvas via the GPU pipeline. Validates the entire architecture end-to-end.

Build:
- `BrushStage` trait, `BrushStageRegistry`, `BrushPipeline` structs
- Ping-pong dab buffer allocation and management
- `Procedural` source stage (circle with gaussian falloff)
- `ColorOutput` output stage (composite dab onto canvas tiles)
- Basic dab placement with proportional spacing
- Linear stroke interpolation between input events
- WASM bridge: wire up `stroke_to()` to the new pipeline

Result: A hard/soft round brush. No dynamics (fixed size/opacity), no smoothing. Dabs are placed, GPU-rendered, and composited onto the canvas.

**Feedback checkpoint:** Does the basic stroke feel responsive? Is dab placement smooth at various speeds? Any visible gaps or overdraw artifacts? How does latency compare to the existing CPU paint path?

### Phase 2 — Dynamics Core

**Goal:** Pressure-sensitive painting. The brush should feel alive.

Build:
- `PaintInformation` struct (position, pressure, tilt, speed, time, etc.)
- Sensor evaluation (start with pressure, speed, distance — the three most impactful)
- `CubicCurve` transfer function (256-point LUT)
- `ParameterBinding` and `ValueComponents` accumulation
- Wire dynamics to Procedural stage (pressure → size) and ColorOutput (pressure → opacity)

Result: A pressure-sensitive round brush with configurable size and opacity curves.

**Feedback checkpoint:** Does pressure response feel natural? Is the curve system expressive enough? How does it compare to Krita/Photoshop pressure feel? Test with different pressure curves (linear, S-curve, concave, convex) and evaluate each.

### Phase 3 — Stamp Source + Texture

**Goal:** Image-based brush tips and texture overlay — the two features that turn a generic circle into a real brush.

Build:
- `Stamp` source stage (load PNG/GBR images as tips, mip-mapped texture pyramid for scale/rotation)
- Tip coloring modes (tint grayscale → paint color, lightness-preserving)
- `TextureOverlay` transform stage (multiply texture onto dab)
- Per-dab mode (texture anchored to dab center) AND per-stroke mode (texture anchored to canvas coordinates)
- Remaining sensors (tilt, rotation, fade, fuzzy) as needed for tip rotation/scatter

Result: Textured brush strokes with image tips. Can approximate pencil, charcoal, chalk by combining the right tip + texture + dynamics.

**Feedback checkpoint:** Do image tips look crisp at various sizes? Does the mip-mapping produce smooth scaling without aliasing? Does per-stroke texture feel correct (canvas grain that doesn't move with the brush)? Does tip rotation via tilt/drawing-angle feel natural?

### Phase 4 — Smudge + Color Mixing

**Goal:** Paint blending — the critical feature that makes digital paint feel like real paint.

Build:
- Canvas region readback (GPU read of pixels under the dab for Blend stages)
- `Smudge` blend stage (blend paint color with canvas, dulling vs smear modes)
- `ColorMix` blend stage (wet-paint mixing between consecutive dabs)
- Dynamics on smudge length and mix ratio

Result: Paint that blends, smears, and mixes. Combined with Stamp + Texture from Phase 3, this enables realistic oil and watercolor brushes.

**Feedback checkpoint:** Does smudging feel natural or mechanical? Is there visible banding or stepping in the blend? How does dulling vs smear compare to Krita's Color Smudge? Does wet mixing produce believable color transitions? Test with complementary colors (blue into orange) — does it go muddy or stay vibrant?

### Phase 5 — Smoothing + Scatter + Polish

**Goal:** Stroke quality refinements that separate a toy from a tool.

Build:
- Stroke smoothing/stabilizer (weighted moving average, study CSP/SAI approaches)
- `Scatter` transform stage (random positional offset)
- Bezier stroke interpolation (for smooth curves from tablet input)
- Airbrush mode (timed dab repeat while pen is stationary)
- Remaining dynamics sensors (tangential pressure, perspective, etc.)

Result: Smooth, professional-feeling strokes. Scatter enables spray/splatter brushes.

**Feedback checkpoint:** Does the stabilizer produce smooth lines without feeling laggy? Compare to CSP's stabilizer at equivalent settings. Does scatter feel random-natural or random-mechanical? Does airbrush buildup feel correct?

### Phase 6 — Preset System

**Goal:** Save, load, and share brush configurations. Ship default presets.

Build:
- `.darkly-brush` file format (JSON header + binary resources)
- Serialization/deserialization of `BrushPipeline`
- Preset loading into the registry
- Resource embedding and extraction (tips, textures)
- Curate initial default presets using all stages from Phases 1–5

Result: A library of ready-to-use brushes. Users can save their configurations.

**Feedback checkpoint:** Do the default presets cover the core use cases (pencil, ink, oil, watercolor, airbrush, eraser, blender)? Does each preset feel polished out-of-the-box? Are the presets meaningfully different from each other, or do they blur together?

### Phase 7 — Pipeline Editor UI + User Creation

**Goal:** Users can build their own brushes.

Build:
- Visual pipeline editor (frontend/Svelte)
- Stage palette with drag-to-add
- Per-stage parameter controls with live preview
- Dynamics curve editor (draw custom curves, select templates)
- Hot-swap stages at runtime for instant feedback

Result: Full user-facing brush creation workflow.

**Feedback checkpoint:** Can a non-technical artist build a usable brush without documentation? Is the stage metaphor intuitive? Does live preview update fast enough to feel interactive?

### Phase 8 — ABR Import + Advanced Stages

**Goal:** Import existing brush collections. Add remaining stage types.

Build:
- ABR parser (tips + patterns + best-effort parameters → Darkly presets)
- `Blur` transform stage
- `Sharpen` transform stage
- Additional source stages as needed (noise, pattern fill)
- Conditions masking (if architecture validated)

Result: Artists can bring their existing Photoshop brush libraries into Darkly.

### What We Explicitly Defer

- **Node graph UI** — the linear pipeline is the MVP. Graph is a future evolution.
- **Multi-channel output** (displacement, roughness) — architecture supports it, but not until core channels are solid.
- **Bristle physics** (Krita's Hairy brush) — a specialized engine, deferred until the pipeline proves insufficient for natural media emulation.

---

## Open Questions

1. **Performance budget** — How many stages can run per dab before latency becomes perceptible? Need to benchmark GPU pass overhead at typical dab rates (60–120 Hz at 500px+ diameter). The ping-pong pattern adds texture copies; compute shaders reading/writing a single buffer per stage may be more efficient.

2. **Conditions masking implementation** — Should this be a stage type (composable, follows the pattern) or a pipeline-level feature (applied as a post-process mask after all stages)? Stage type is more modular; pipeline-level is more efficient (single canvas read).

3. **Multi-channel output** — How do we handle stages that need to write to multiple channels simultaneously (e.g., color + displacement)? Options: multiple output stages in one pipeline, or a single output stage with multi-channel configuration.

4. **Stabilizer quality** — CSP and SAI's stabilizers are considered the gold standard. Need to research their algorithms (likely a form of Catmull-Rom or predictive smoothing with lookahead) rather than settling for simple weighted moving average.

5. **Texture mode** — Krita's biggest missing feature is per-stroke texture (texture anchored to canvas coordinates rather than dab center). Our Texture Overlay stage should support this from day one, but the implementation needs care: the texture UV must be computed from the dab's canvas position, not its local position.

6. **Hot-swapping performance** — When a user edits a pipeline in the preset editor, we want live preview. How expensive is it to rebuild the GPU pipeline when a stage is added/removed/reordered? The lazy pipeline caching pattern helps (stage pipelines are cached), but the overall command buffer encoding may need to be re-planned.
