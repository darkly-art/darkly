# Node-Graph Composable Brush Engine — Implementation Plan

## The Vision

We are building a GPU-native brush system capable of rivaling Procreate — that smooth, satisfying, realtime stabilization that feels taffy-like.

**The Procreate secret:** Despite being a raster editor, Procreate keeps the full stroke and all its input data (pressure, tilt, speed, etc.) as a vector. This enables its most characteristic features:
- Editing the stroke after finishing it
- Realtime, retroactive changes to the stroke as the user moves the pen (stabilization)
- A brush editor that lets the user draw a stroke, then tweak brush settings and watch the stroke morph in realtime

Procreate accomplishes this by redrawing the entire stroke every frame. Because it is GPU-native, it can afford to do this. We are GPU-native too — this is our guiding star.

**The architecture:** A node-based brush system where the pen device and stroke engine are the only mandatory nodes, and the rest are fully in the user's control. Sensor inputs (pressure, tilt, speed) flow through user-wirable math/curve nodes into GPU stage parameters.

**The plan:** We will not build these all at once. Our first job is to recreate basic Krita brushes and compare them to hone our node system. Once we can produce a Krita brush with good results, we move on to Procreate-level quality. Crawl, walk, run.

**Non-negotiable groundwork:** The stroke engine must retain all raw input events as vectors for the duration of the stroke (and optionally in the undo stack for post-stroke editing). This is ephemeral — Darkly is a raster program, and the rasterized layer pixels are the document truth. Vectors are discarded on the next action. But while the stroke is live (and while it's the most recent undoable action), the vector data enables re-rendering with different parameters.

## Value Range Convention

Established by researching Blender's node system (`source/blender/makesdna/DNA_node_types.h`, `NOD_socket_declarations.hh`) and Krita's dynamics (`KisDynamicSensor`, `KisCurveOption`):

**Everything speaks 0-1.** Sensors output 0-1. Curves map 0-1 → 0-1. Math nodes operate on raw values (multiply 0.5 * 0.8 = 0.4, still in 0-1). GPU stage inputs expect 0-1 and internally map to their actual parameter range (e.g., size: 0-1 → 1-500px). Connecting any sensor to any parameter input does something sensible without remapping.

**Port min/max is slider metadata for disconnected ports.** When a port has no wire, the UI shows a slider — min/max controls that slider's range. When a wire is connected, the slider disappears and the wire value takes over. This follows Blender's `bNodeSocketValueFloat` pattern: min/max constrain the manual-input slider, not the data flow.

**Explicit remap nodes for power users.** If a user wants only the top half of pressure range to affect size, they add a Remap node. This is the exception, not the rule — the default wiring should just work.

## Current State

Darkly has a complete GPU-authoritative rendering engine but no brush system — painting is currently hard-coded `StrokeOp::PaintCircle` dispatching to an SDF circle shader. No dab system, no dynamics, no pipeline.

---

## Phase 1 — Domain-Agnostic Graph Infrastructure

**Goal:** Pure-Rust graph data structure. No GPU, no brush concepts. Fully testable with `cargo test`.

### Create

- `crates/darkly/src/nodegraph/mod.rs` — `WireKind` trait (Copy+Eq+Hash+Serialize), re-exports
- `crates/darkly/src/nodegraph/graph.rs` — `NodeId(u64)`, `PortRef`, `Connection`, `PortDef<W>`, `NodeInstance<W>`, `Graph<W>`
  - Operations: `add_node`, `remove_node`, `connect` (type-check via `W::compatible`, cycle-check via DFS), `disconnect`
  - `GraphError` enum: TypeMismatch, CycleDetected, PortNotFound, InputAlreadyConnected
- `crates/darkly/src/nodegraph/compiler.rs` — `ExecStep`, `ExecutionPlan`
  - `compile(graph, node_registry)`: Kahn's topological sort → slot allocation → step ordering
- `crates/darkly/src/nodegraph/registration.rs` — `NodeRegistration<W>` (type_id, category, display_name, ports, params, is_gpu)

### Modify

- `crates/darkly/src/lib.rs` — add `pub mod nodegraph`

### Verify

Unit tests with a test-only `TestWireKind` enum:
- Add/connect/disconnect/remove round-trip
- Cycle detection rejects A→B→A
- Type mismatch rejected by `connect()`
- Topological sort produces valid execution order
- Compilation assigns correct slot indices
- Serde round-trip

---

## Phase 2 — Brush Wire Types + CPU Nodes

**Goal:** `BrushWireType`, evaluation runtime, `PaintInformation` with vector stroke storage, first CPU nodes. End-to-end test: build graph → compile → evaluate → assert outputs.

### Create

- `crates/darkly/src/brush/mod.rs` — `BrushNodeRegistry` (HashMap-backed, from `nodes::registrations()`), type alias `BrushNodeRegistration`
- `crates/darkly/src/brush/wire.rs` — `BrushWireType` enum (Scalar, Int, Bool, Vec2, Vec4, Color, Texture, Mask), `WireKind` impl with coercions, `ScalarValue` (16-byte Copy enum), `TextureHandle(u16)`
- `crates/darkly/src/brush/paint_info.rs` — `PaintInformation` struct (position, pressure, tilt, rotation, time, speed, distance, drawing_angle, etc.), `StrokeRecord { events: Vec<PaintInformation>, color, brush_graph_id }`
- `crates/darkly/src/brush/eval.rs` — `BrushNodeEvaluator` trait (evaluate_cpu), `BrushGraphRunner` (plan + evaluators + flat `Vec<Option<ScalarValue>>` slot table), `seed_sensors()`, `execute_cpu()`
- `crates/darkly/src/brush/nodes/pen_input.rs` — 16 sensor output ports, no-op evaluator (seed_sensors writes directly to slots)
- `crates/darkly/src/brush/nodes/constant.rs` — Float param → Scalar output
- `crates/darkly/src/brush/nodes/multiply.rs` — Scalar * Scalar → Scalar
- `crates/darkly/src/brush/nodes/curve.rs` — Scalar in → power curve → Scalar out (gamma param; full piecewise-linear later)
- `crates/darkly/src/brush/nodes/paint_color.rs` — Color output (foreground color)

### Modify

- `crates/darkly/src/lib.rs` — add `pub mod brush`
- `crates/darkly/build.rs` — add `generate_registry(&src.join("brush/nodes"), "crate::brush::BrushNodeRegistration");`

### Key Design

- `StrokeRecord` stores raw pre-smoothing events — re-rendering with different smoothing is possible
- `pen_input` is special-cased: `seed_sensors()` writes directly to pre-known slot indices (memcpy, not virtual dispatch)
- Zero heap allocation per dab: flat pre-sized Vec, no HashMap lookups

### Verify

- Build graph (pen_input → multiply(constant 0.5) on pressure), compile, evaluate with mock PaintInformation, assert output = 0.5 * pressure
- Verify curve node with different gamma values
- Verify StrokeRecord accumulates events

---

## Phase 3 — GPU Stage Nodes + Dab Texture Pool

**Goal:** First visual output from the graph. Procedural dab generation + composite onto canvas = a painted circle from the node graph.

### Create

- `crates/darkly/src/brush/dab_pool.rs` — `DabTexturePool` (pre-allocated RGBA8 textures at max dab size, acquire/release)
- `crates/darkly/src/brush/gpu_context.rs` — `BrushGpuContext` (encoder, device, queue, dab_pool, canvas target, pipelines)
- `crates/darkly/src/brush/pipelines.rs` — `BrushPipelines` (circle + stamp + composite render pipelines, follows `PaintPipelines` pattern)
- `crates/darkly/src/brush/nodes/circle.rs` — GPU shape node: softness input → Texture output, renders SDF circle mask to dab texture
- `crates/darkly/src/brush/nodes/color_output.rs` — GPU terminal node: dab Texture + position Vec2 inputs, composites dab onto canvas layer
- `shaders/brush/circle.wgsl` — SDF circle mask generation
- `shaders/brush/composite.wgsl` — dab texture compositing onto canvas (positioned quad, alpha-over blend)

### Modify

- `crates/darkly/src/brush/eval.rs` — add `evaluate_gpu` to trait, add `texture_slots` to runner, add `execute_gpu(ctx)`

### Key Design

- Dab textures pre-allocated (no GPU allocation during painting)
- `BrushPipelines` separate from `PaintPipelines` — different concerns (dab gen + dab composite vs. SDF circle + gradient)
- Two render passes per dab: generate → composite

### Verify

- GPU integration test: build minimal graph (circle → stamp ← pen_input, paint_color → color_output), seed sensors, execute, readback canvas, verify non-zero pixels at expected position

---

## Phase 4 — Stroke Engine + Engine Integration

**Goal:** Paint on the canvas with a node-graph brush. This is the first visual milestone.

### Create

- `crates/darkly/src/brush/stroke_engine.rs` — `StrokeEngine` with:
  - `begin(runner, color, spacing, smoothing)`
  - `move_to(raw_info, gpu_ctx)`: store event in StrokeRecord → apply smoothing → compute derived values → interpolate between last point and current → place dabs at spacing intervals → evaluate graph per dab
  - `end() -> StrokeRecord`: return full stroke record
  - `replay(record, ctx)`: skeleton for re-rendering (iterate events, call move_to)
- `crates/darkly/src/brush/spacing.rs` — `SpacingConfig` (proportional spacing as % of diameter)
- `crates/darkly/src/brush/interpolation.rs` — `lerp_paint_info(a, b, t)` (linear interp of all fields)

### Modify

- `crates/darkly/src/engine/mod.rs` — add fields: `stroke_engine: Option<StrokeEngine>`, `brush_pipelines: BrushPipelines`, `dab_pool: DabTexturePool`, `brush_registry: BrushNodeRegistry`, `active_brush_graph: Option<CompiledBrushGraph>`
- `crates/darkly/src/engine/types.rs` — add `StrokeOp::BrushStroke { x, y, pressure, x_tilt, y_tilt, rotation, tangential_pressure, time_ms }` variant
- `crates/darkly/src/engine/painting.rs` — handle `BrushStroke`: lazy-init StrokeEngine from active graph, build PaintInformation, call move_to
- `frontend/wasm/src/api.rs` — extend `stroke_to()` to accept `brush_stroke` op type with full tablet data
- `frontend/src/tools/brush.svelte.ts` — extract tablet data from PointerEvent (pressure, tiltX, tiltY, twist), call `stroke_to('brush_stroke', {...})`

### Key Design

- `StrokeRecord` populated on every `move_to()` — lives for the stroke's duration and optionally in the undo stack, discarded on next action
- Spacing is distance-based (Krita default: ~10% of diameter), matching `KisSpacingInformation::isotropicSpacing`
- Basic weighted moving average smoothing for positions
- All dab render passes accumulated into single command encoder per `move_to()`
- Existing `PaintCircle`/`EraseCircle` paths remain for non-brush tools

### Verify

- Open Darkly, select brush tool, draw → see pressure-sensitive strokes
- Verify evenly spaced dabs (no gaps, no pileup)
- Verify all events stored in StrokeRecord
- Compare performance to existing SDF circle path

---

## Phase 5 — WASM Bridge + Brush Builder UI

**Goal:** Visualize and edit brush graphs in the browser. The brush builder develops alongside the backend — it doesn't need to be pretty, but it must be functional for debugging and testing every subsequent phase.

### Create

- `frontend/src/ui/brush_builder/BrushBuilder.svelte` — main container (node canvas + stroke preview area)
- `frontend/src/ui/brush_builder/NodeCanvas.svelte` — SVG wires + HTML nodes, pan/zoom
- `frontend/src/ui/brush_builder/NodeWidget.svelte` — single node: header, ports, inline param sliders (reuse VeilItem pattern)
- `frontend/src/ui/brush_builder/WireRenderer.svelte` — bezier curves for connections, color-coded by wire type
- `frontend/src/ui/brush_builder/PortWidget.svelte` — port circle, drag to connect, type-compatibility dimming
- `frontend/src/ui/brush_builder/NodePalette.svelte` — available nodes by category (from WASM registry)
- `frontend/src/state/brush_graph.svelte.ts` — reactive graph state, JSON sync to WASM

### Modify

- `frontend/wasm/src/api.rs` — add: `brush_node_types()`, `brush_graph_validate(json)`, `brush_graph_compile(json)`, `brush_graph_default()`
- `crates/darkly/src/engine/mod.rs` — add methods: `brush_node_types()`, `compile_brush_graph(json)`, `default_brush_graph()`
- `crates/darkly/src/brush/mod.rs` — add `compile_from_json()`, `default_graph()`
- `frontend/src/tools/brush.svelte.ts` — compile default brush on tool activation
- `frontend/src/ui/LeftSidebar.svelte` — brush builder toggle/entry point

### Key Design

- Graph state lives in Svelte as reactive state. On change, JSON sent to Rust for validation/compilation. Errors shown inline on nodes/ports.
- Live recompilation on graph change — edit a node, see the stroke update immediately
- Doesn't need to be polished — functional wireframe is fine. Polish later.

### Verify

- Load Darkly, open brush builder, see default graph (circle → stamp ← pen_input, paint_color → color_output)
- Draw strokes with the default brush
- Drag-connect pressure to size, see pressure sensitivity change live

---

## Phase 6 — Dynamics + Math Nodes

**Goal:** Full dynamics system — any parameter wirable to any sensor through math/curve nodes. Now testable visually through the brush builder.

### Create

- `crates/darkly/src/brush/nodes/add.rs` — Scalar + Scalar → Scalar
- `crates/darkly/src/brush/nodes/clamp.rs` — clamp(Scalar, min, max) → Scalar
- `crates/darkly/src/brush/nodes/remap.rs` — remap(Scalar, in_range, out_range) → Scalar
- `crates/darkly/src/brush/nodes/mix.rs` — mix(a, b, t) → Scalar/Color
- `crates/darkly/src/brush/nodes/split_vec2.rs` — Vec2 → (Scalar, Scalar)
- `crates/darkly/src/brush/nodes/make_color.rs` — (R, G, B, A) → Color

### Modify

- `pen_input.rs` — ensure all 16 sensors properly seeded (fuzzy_dab/fuzzy_stroke via deterministic PRNG)
- `stamp.rs` — scatter_x, scatter_y, rotation input ports (moved from former procedural node)

### Verify

- In brush builder: wire pressure→curve→size, speed→remap→opacity, tilt→rotation
- Draw test strokes, verify dynamics visually
- Verify fuzzy_dab produces different values per dab, fuzzy_stroke same within stroke

---

## Phase 7 — Preset Format + Round-Trip

**Goal:** Define and ship the `.darkly-brush` preset format. Save/load from disk, round-trip tested. This comes before stamp tips because every subsequent phase adds new node types and capabilities — having a tested serialization format early catches compatibility issues as the system grows.

### Format Design

A `.darkly-brush` file is a ZIP archive containing:
- `preset.json` — metadata envelope + node graph
- `resources/` — binary assets (brush tip images, textures) referenced by filename from the graph

```json
{
  "format_version": 1,
  "name": "Soft Round Pressure",
  "engine_version": "0.1.0",
  "category": "Basic",
  "author": "Darkly",
  "description": "Pressure-sensitive soft round brush",
  "tags": ["round", "soft", "basic"],
  "graph": { /* Graph<BrushWireType> serialization */ },
  "resources": [
    { "name": "tip.png", "kind": "brush_tip", "path": "resources/tip.png" }
  ]
}
```

**Why ZIP, not flat JSON:** Brush tip images and textures are binary blobs — embedding them as base64 in JSON bloats the file and makes diffs unreadable. ZIP keeps the JSON human-readable and the binaries efficient. This matches how KPP (PNG+XML), ORA (ZIP+XML), and GIMP's GIH formats handle embedded resources.

### Create

- `crates/darkly/src/brush/preset.rs` — `BrushPreset` struct (name, category, author, description, tags, graph, resources), `PresetResource` struct, `save(path)`, `load(path)`, format version handling
- `crates/darkly/src/brush/preset_library.rs` — `PresetLibrary` (scan directory, list presets, load by name, save new)

### Modify

- `crates/darkly/src/brush/mod.rs` — add `pub mod preset; pub mod preset_library;`
- `crates/darkly/src/engine/mod.rs` — add `preset_library: PresetLibrary`, methods for save/load/list
- `frontend/wasm/src/api.rs` — add `brush_preset_list()`, `brush_preset_load(name)`, `brush_preset_save(json)`, `brush_preset_export(name) -> Vec<u8>`, `brush_preset_import(bytes)`

### Verify

- **Round-trip test:** Create graph programmatically → save to `.darkly-brush` → load back → assert graph equals original (node types, connections, param values, positions all match)
- **Round-trip with resources:** Save preset with embedded brush tip image → load → verify image bytes identical
- **Format version test:** Load a v1 preset, verify forward-compat strategy (unknown fields ignored, missing optional fields defaulted)
- **Corrupt file handling:** Truncated ZIP, missing preset.json, malformed JSON — all return clean errors
- **Library scanning:** Create temp directory with multiple presets, verify `PresetLibrary` discovers and lists them all
- **WASM bridge test:** Save via WASM → list → load → compile → paint — full stack round-trip

---

## Phase 8 — Stamp Tips + User-Exposed Properties

**Goal:** Image-based dab sources and user-exposed brush properties. Stamp tips are the big unlock for Krita brush compatibility — most pixel brush presets are a grayscale stamp image + dynamics. User-exposed properties let brush creators surface labeled sliders to end users, making every brush built from this point forward properly configurable without opening the node graph.

### 8a: Stamp Tips

#### Create

- `crates/darkly/src/brush/nodes/stamp.rs` — Universal GPU stamper: takes any tip texture (from `circle` or `image`), stamps it with size/rotation/mirror/ratio/opacity/color transforms. Inputs: tip, size, rotation, mirror_x, mirror_y, ratio, opacity, color, scatter_x, scatter_y. Outputs: dab, dab_size, scatter_offset.
- `crates/darkly/src/brush/brush_tip.rs` — `BrushTip` enum (Auto { hardness, shape, spikes, ratio, fade }, Predefined { image, application_mode }), `BrushTipApplication` enum (AlphaMask, ImageStamp, LightnessMap, GradientMap — per Krita's `enumBrushApplication`)
- `shaders/brush/stamp.wgsl` — Sample brush tip texture, apply color + opacity, handle rotation/mirror/ratio transforms

#### Modify

- `crates/darkly/src/brush/dab_pool.rs` — add brush tip texture upload and caching (separate from dab render targets)
- `crates/darkly/src/brush/preset.rs` — handle brush tip images as preset resources

#### Key Design

- Brush tip textures uploaded once and cached — not per-dab
- Alpha mask mode: tip grayscale = opacity, color from paint color (most common)
- Image stamp mode: tip RGB used directly (for colored brushes)
- Lightness map mode: tip luminance modulates paint color lightness (Krita's default for color smudge)
- Auto brush tips generated on the GPU as a texture at brush load time, then treated identically to predefined tips

#### Verify

- Load a grayscale PNG brush tip → paint with it → verify dab shape matches tip
- Rotation dynamics: wire drawing_angle → stamp rotation → verify dabs rotate along stroke
- Mirror: enable mirror_x → verify horizontally flipped dabs
- Round-trip: save preset with embedded tip → load → verify painting identical
- Compare output with Krita using same tip image at same settings

### 8b: User-Exposed Properties (`user_input` Node)

A `user_input` node is a source node (like `constant`) that the brush creator places and labels. Functionally identical to `constant` — outputs a Scalar from a parameter — but semantically marked so the system surfaces it in a user-facing properties panel. This is the Krita "Brush Settings" vs "Brush Editor" distinction, or Procreate's per-brush slider panel.

**Infrastructure prerequisite:** Add `ParamDef::String { name, default }` and `ParamValue::String(String)` to `params.rs`. Needed for the user-facing label.

**Node definition:**
- `type_id`: `"user_input"`, category: `"input"`, display_name: `"User Input"`
- Params: `label` (String, default `""`), `value` (Float, 0-1, default 0.5)
- Ports: one output `"value"` (Scalar)
- Evaluator: reads `param_f32(1)`, outputs as Scalar (identical to `ConstantEvaluator`)

**Value range:** Output is always 0-1 per the system convention. If the brush creator wants a different effective range, they wire through remap/multiply nodes. The user always sees a 0-1 slider. This keeps the node simple and consistent with the rest of the system.

#### Create

- `crates/darkly/src/gpu/params.rs` — add `ParamDef::String` / `ParamValue::String` variants
- `crates/darkly/src/brush/nodes/user_input.rs` — node registration + evaluator (auto-discovered by `build.rs`)

#### Modify

- `crates/darkly/src/engine/brush_graph.rs` — add `brush_user_inputs() -> Vec<UserInputInfo>` (walks graph, finds all `user_input` nodes, returns `{ node_id, label, value }`)
- `frontend/wasm/src/api.rs` — add `brush_user_inputs()` query
- `frontend/src/ui/brush_builder/` — properties panel component showing labeled sliders for equipped brush

#### Key Design

- Query-based: the properties panel is derived from the graph, not stored separately. Add a `user_input` node = it appears in properties. Remove it = gone.
- Mutation uses existing `brush_graph_set_param(node_id, 1, Float(value))` — no new mutation API needed.
- Preset serialization already handles this: `user_input` nodes are just graph nodes with params, and the preset format serializes the full graph.
- Multiple `user_input` nodes → multiple sliders. Order determined by node position (top-to-bottom, left-to-right) for a stable, creator-controlled layout.

#### Verify

- Place 3 `user_input` nodes labeled "Size", "Softness", "Scatter" → equip brush → verify properties panel shows 3 labeled sliders
- Adjust slider → verify brush behavior changes in realtime
- Save/load preset with `user_input` nodes → verify labels and values round-trip
- Remove a `user_input` node from the graph → verify it disappears from properties

---

## Phase 9 — Texture Overlay

**Goal:** Pattern/grain textures applied to dabs. Needed for pencil, charcoal, canvas-texture brushes.

### Create

- `crates/darkly/src/brush/nodes/texture_overlay.rs` — GPU node: takes dab Texture + pattern Texture, composites pattern onto dab. Inputs: dab, pattern, scale, offset_x, offset_y, strength, blend_mode. Output: Texture. Blend modes: Multiply, Subtract, Overlay (per Krita's `KisTextureOption`)
- `crates/darkly/src/brush/pattern.rs` — `BrushPattern` (tiling image, scale, offset mode: fixed / random per dab / follow stroke)
- `shaders/brush/texture_overlay.wgsl` — Pattern sampling with tiling, blend with dab

### Modify

- `crates/darkly/src/brush/preset.rs` — handle pattern textures as preset resources

### Verify

- Apply pencil-grain texture to round brush → verify visible grain in strokes
- Scale parameter changes grain size
- Multiply vs overlay produce visually distinct results
- Round-trip: save preset with pattern → load → verify identical

---

## Phase 10 — Stroke Stabilizer

**Goal:** Retroactive stroke reshaping — the defining feature. When drawing, the stroke is always at the cursor (zero lag), but the stroke behind the pen continuously reshapes itself as new input arrives. Changing direction pulls and bends the already-drawn curve. It should feel like piping frosting onto a cake or pulling taffy — the stroke has physical body and weight, it stretches and flows, but it's always anchored to where the pen is right now. Procreate is the only app that has achieved this feel; they are closed source. This is a hard research problem that requires designing our own system from first principles.

### What this is NOT

These are common misunderstandings that must be avoided:

- **NOT pulled string / Lazy Nezumi.** In those systems, the cursor moves ahead and the stroke trails behind, catching up over time. There is a visible gap between pen and mark. That is the opposite of what we want.
- **NOT forward-only smoothing.** Exponential moving average, Gaussian filtering, or any algorithm that only affects future points and leaves the past frozen is insufficient. The past must move.
- **NOT a delay/lag system.** The stroke is never behind the cursor. The pen tip IS the stroke tip, always, every frame.
- **NOT a post-processing step.** The reshaping happens live during the stroke, not after pen-up. The user sees the curve behind them shift as they draw.

### What this IS

When you draw and change direction, the stroke you already drew — the pixels on screen behind your pen — visibly moves to accommodate the new direction. The curve reshapes retroactively. If you draw right then turn sharply down, the corner doesn't stay sharp — the stroke behind the corner bends and flows into the new direction. The already-rendered pixels shift. You are not just adding to the stroke, you are sculpting it as you go.

The pen is always at the tip of the stroke (zero lag). The smoothing manifests as the past reshaping to meet the present, not the present lagging behind to stay smooth.

### Architecture

Three layers, each ignorant of the others' internals:

**Layer 1 — Stabilizer** (owns the curve)

Receives raw input points. Produces a stabilized vector (the smooth curve). On each new input point, the stabilizer re-runs on the full input history and produces an updated vector. It then diffs the updated vector against the previous frame's vector to find the **divergence point** — the earliest position on the curve where the new vector differs from the old one.

The stabilizer is NOT optional. All strokes flow through it, always. The user controls the algorithm and its strength (including zero, which passes through raw input unchanged), but the mechanism — vector production, diffing, divergence detection — is always active. This is because the entire rendering pipeline depends on it: the stroke buffer, the save points, the partial re-render. These aren't features of the stabilizer, they're features of how strokes are rendered, and the stabilizer is the entry point.

The specific smoothing algorithm is TBD pending research. Do not assume a particular approach.

**Layer 2 — Dab save-point system** (owns restore/rewind)

The stroke renders into a **stroke buffer** — a texture separate from the canvas that holds only the in-progress stroke. The canvas the user sees is: pre-stroke snapshot composited with the stroke buffer.

Each dab rendered into the stroke buffer is associated with a position on the stabilized vector. After each dab is composited into the stroke buffer, the system diffs the stroke buffer against the **pre-stroke canvas snapshot** (which already exists today for undo). This diff produces a bounding box representing the cumulative change from the start of the stroke through this dab. That bounding box (and the pixel data within it from the pre-stroke snapshot) is stored as the **save point** for this dab.

To understand save points: dab #10's save point is NOT "what dab #10 changed." It is the bounding box of everything dabs #1 through #10 changed, diffed against the pre-stroke snapshot. This means any save point, paired with the pre-stroke snapshot, can fully restore the stroke buffer to its state at that dab. You don't walk save points in reverse — you jump directly to the one you need.

**Rewind operation:** The stabilizer reports "divergence at vector position mapped to dab #44." The save-point system takes dab #44's save point, copies the pre-stroke snapshot pixels into that bounding box region of the stroke buffer. The stroke buffer now looks exactly as it did after dab #44 was rendered. This is a single GPU copy operation — not a replay.

**Layer 3 — Brush engine** (owns rendering)

Receives a canvas (the stroke buffer, already restored to the correct state), a vector (the tail from the divergence point to the tip), and a brush. Renders dabs. It does not know that the canvas was rewound, that the vector is a partial tail, or that stabilization exists. It just paints, same as it does today.

### Per-frame flow during a stroke

```
1. New pen input event arrives (position, pressure, tilt, etc.)

2. Stabilizer:
   a. Append raw input to history
   b. Re-run stabilization on full history → new stabilized vector
   c. Diff new vector against previous frame's vector
   d. Find divergence point → maps to a dab index

3. Save-point system:
   a. Look up save point for the dab at the divergence point
   b. Copy pre-stroke snapshot pixels into the stroke buffer
      within that save point's bounding box
   c. Stroke buffer is now restored to post-divergence-dab state

4. Brush engine:
   a. Render dabs from divergence point to tip into stroke buffer
   b. Each new dab gets a save point (diff stroke buffer vs pre-stroke snapshot)

5. Compositor:
   a. Composite stroke buffer onto pre-stroke snapshot → display
```

### Performance characteristics

**Smooth continuous motion (the common case):** The stabilizer's new vector matches the previous vector everywhere except the tip. Divergence is at the last dab. No rewind needed — just render one new dab and create its save point. Cost: identical to the current (non-stabilized) pipeline.

**Direction change:** The tail of the vector reshapes. Divergence is further back — say 10-20 dabs. Rewind is one GPU copy (the save point). Then 10-20 dabs re-rendered. This is exactly when the visual change is most significant, so the extra work produces visible results.

**Worst case (very long stroke with global reshaping):** Divergence is near the start. Rewind + re-render most of the stroke. This should be rare with a well-designed stabilizer (influence should be local, not global). If it happens frequently, the stabilizer algorithm needs adjustment — the rendering system can handle it, but it signals the algorithm is doing too much retroactive work.

### Critical implementation detail

The dab-to-vector-position mapping is **load-bearing**. Each dab must know which position on the stabilized vector it corresponds to, so that when the stabilizer says "divergence at vector position X," the save-point system can look up "that's dab #N" and restore to it. If this mapping is wrong — if dabs and vector positions are out of sync — the rewind point is wrong and the stroke will have visual artifacts at the splice between restored and re-rendered regions.

This mapping must survive re-stabilization: when the vector reshapes, the dabs that were placed along the OLD vector need to be associated with the corresponding positions on the NEW vector. The stabilizer's diff must account for this — it's not just "which positions changed" but "which old dabs correspond to which new positions."

### Research

- Investigate candidate algorithms from first principles: spline re-fitting (cubic, Catmull-Rom, B-spline), relaxation/optimization on control points, physical simulation with retroactive propagation
- Read prior art in curve fitting and online spline approximation literature
- Prototype candidates and evaluate by feel

### Create

- `crates/darkly/src/brush/stabilizer.rs` — the stabilizer algorithm. Takes raw input points, produces a smooth curve that reshapes retroactively as new points arrive. Always in the pipeline (not optional). The specific algorithm is TBD pending research.
- Stroke buffer texture — separate from the canvas, holds only the in-progress stroke. Composited onto the pre-stroke snapshot for display each frame.
- Dab save-point storage — per-dab bounding box diffed against the pre-stroke snapshot, keyed to vector position. Uses the existing diff infrastructure (`DiffRectPass`).

### Modify

- `crates/darkly/src/brush/stroke_engine.rs` — all strokes flow through the stabilizer. The brush engine receives a canvas and a vector and renders dabs, unaware of rewind.
- `crates/darkly/src/brush/nodes/color_output.rs` — during a stroke, composites dabs into the stroke buffer instead of directly onto the canvas.
- Compositing pipeline — per-frame composite of stroke buffer onto pre-stroke snapshot for display.

### Verify

- Zero perceptible lag — stroke is always at the cursor, every frame, no exceptions
- Drawing a sharp corner → the stroke behind the corner visibly reshapes into a smooth curve as you continue moving
- Drawing a straight line then curving → the transition region of the already-drawn stroke bends to meet the new direction
- The feel is physical — like frosting, like taffy. The stroke has weight and body
- Smooth continuous motion renders only one dab per input event (verify via dab count instrumentation)
- Sharp direction change re-renders a tail of dabs (verify the rewind point is correct — no artifacts at the splice between restored and re-rendered regions)
- Save-point restore produces pixel-identical results to full stroke re-render from scratch
- Performance: partial re-render keeps up with input event rate during normal painting
- Stabilizer at strength zero: stroke is identical to raw input (pass-through, but the full pipeline still runs)

---

## Phase 11 — KPP Import

**Goal:** Load Krita `.kpp` preset files and convert them to `.darkly-brush` presets. This is the "download from krita-artists.org and paint" milestone.

### Create

- `crates/darkly/src/brush/kpp_import.rs` — `import_kpp(bytes) -> Result<BrushPreset>`: extract PNG thumbnail, parse embedded XML, extract base64-encoded resources (brush tips, patterns, gradients), map Krita settings to a node graph
- `crates/darkly/src/brush/kpp_mapping.rs` — `KritaSettings → Graph<BrushWireType>` translation: for each Krita option (size, opacity, flow, rotation, scatter, etc.), emit the corresponding nodes + connections. Sensor curve XML → Curve nodes with matching gamma/control points.

### Krita Engine Coverage

**Pixel brush (`paintopid="paintbrush"`):**
- Auto brush → Procedural node with matching hardness/shape
- Predefined brush → Stamp node with extracted tip image
- All standard options: size, opacity, flow, softness, rotation, scatter, spacing, ratio, mirror
- Sensor curves: pressure, speed, distance, tilt, drawing_angle, fuzzy, fade, time
- Texture option → Texture Overlay node

**Color smudge (`paintopid="colorsmudge"`):**
- Same brush tip handling as pixel brush
- Smudge length, color rate → Smudge node params
- Smear vs dulling mode
- Lightness map application mode

**Unsupported engines (graceful fallback):**
- MyPaint, hairy, spray, sketch, etc. → return `Err` with engine name, or fall back to a basic round brush with a warning

### Modify

- `frontend/wasm/src/api.rs` — add `brush_import_kpp(bytes) -> Result<String>` (returns preset JSON or error)
- Preset library — imported presets saved as `.darkly-brush` for future loads

### Verify

- **Known-good presets:** Download 5-10 popular brush packs from krita-artists.org, import each preset, verify it loads without error and produces reasonable output
- **Property coverage:** For each Krita option type, create a minimal KPP exercising that option → import → verify the corresponding node graph is correct
- **Sensor curves:** Create KPP with custom pressure→size curve → import → verify Curve node control points match
- **Embedded resources:** KPP with predefined brush tip + texture → import → verify both extracted and wired into graph
- **Round-trip:** Import KPP → save as .darkly-brush → load → verify identical graph
- **Unsupported engine:** Import KPP with hairy brush engine → verify clean error message

---

## Phase 12 — Color Smudge

**Goal:** Canvas readback + paint blending. This unlocks the second most popular Krita engine (~25% of community presets).

### Create

- `crates/darkly/src/brush/nodes/smudge.rs` — GPU node: reads canvas pixels under dab position, blends with paint color according to smudge_length and color_rate. Inputs: position, dab_size, smudge_length (0=full smudge, 1=no smudge), color_rate (how much paint color mixed in), paint_color, mode (dulling/smear). Output: Color (blended color to use for this dab).
- `shaders/brush/smudge_readback.wgsl` — Read canvas region, average/sample, blend with paint

### Key Design

- **Dulling mode:** Average canvas color under dab → blend with paint color at color_rate → use as dab color. Simple, most common.
- **Smear mode:** Read canvas at offset position (previous dab location → current), producing a directional smear. More complex, used for oil-paint effects.
- Per Krita's `KisColorSmudgeStrategy` hierarchy — start with dulling, add smear as a follow-up.
- Canvas readback requires a copy of the canvas region before compositing the current dab — coordinate with `color_output.rs` to read-before-write.

### Verify

- Paint with smudge brush over existing colors → verify color pickup and blending
- Smudge length 0 → pure smudge (no paint), 1 → pure paint (no smudge)
- Color rate controls paint/canvas mix ratio
- Import Krita color smudge preset → verify reasonable output
- Performance: smudge readback should not significantly degrade painting FPS

---

## Later Phases (Outlined)

**13: Color Dynamics** — hue/saturation/value randomization per dab, color mixing along stroke, gradient mapping

**14: Stroke Re-rendering** — keep StrokeRecord in undo stack entry, "edit last stroke" mode (re-render with tweaked parameters), discard vectors on next action

**15: Brush Builder Polish** — refined UI, curve editor widget, preset browser with thumbnails, drag-drop import, category filtering

**16: Default Brush Library** — ship a curated set of `.darkly-brush` presets covering common use cases (pencil, ink, watercolor, airbrush, eraser, smudge, textured)

---

## Dependency Graph

```
Phase 1 (nodegraph infra)                    ✓ complete
    ↓
Phase 2 (brush wire types + CPU nodes)       ✓ complete
    ↓
Phase 3 (GPU stage nodes)                    ✓ complete
    ↓
Phase 4 (stroke engine + integration)        ✓ complete
    ↓
Phase 5 (WASM bridge + brush builder UI)     ✓ complete
    ↓
Phase 6 (dynamics + math nodes)              ✓ complete
    ↓
Phase 7 (preset format + round-trip)         ✓ complete
    ↓
Phase 8 (stamp tips + user-exposed properties) ✓ complete
    ↓
Phase 9 (texture overlay)                    ✓ complete
    ↓
Phase 10 (smoothing + stabilizer)            ← CURRENT
    ↓
Phase 11 (KPP import)                        ← "download and paint" milestone
    ↓
Phase 12 (color smudge)                      ← ~90% of Krita brush packs work
    ↓
Phase 13-16 (color dynamics, re-rendering, UI polish, default library)
```

### Parallelizable Work

Some phases can overlap:
- **Phase 7 + 8** can be developed together — stamp node is a new node type that immediately tests the preset resource system
- **Phase 9 + 10** can overlap — texture overlay and smoothing are independent of each other
- **Phase 9 + 11** can overlap — texture overlay is needed by KPP import, but basic KPP import (brushes without texture) can land first
- **Phase 8a + 8b** are independent of each other and can be developed in parallel

## Critical Existing Files

| File | Role |
|------|------|
| `crates/darkly/src/gpu/veil.rs` | Registration pattern template (VeilRegistration, VeilRegistry, Veil trait) |
| `crates/darkly/src/gpu/veils/bokeh.rs` | Example module with register() + GPU pipeline + cache |
| `crates/darkly/src/gpu/effect.rs` | EffectPipeline + EffectCache pattern |
| `crates/darkly/src/gpu/paint_target.rs` | GpuPaintTarget, PaintPipelines, blend states, render pass patterns |
| `crates/darkly/src/gpu/params.rs` | ParamDef/ParamValue (reuse directly for node params) |
| `crates/darkly/build.rs` | generate_registry() auto-discovery |
| `crates/darkly/src/engine/painting.rs` | Stroke lifecycle, integration point |
| `crates/darkly/src/engine/mod.rs` | DarklyEngine struct, initialization |
| `frontend/wasm/src/api.rs` | WASM bridge methods |
| `frontend/src/tools/brush.svelte.ts` | Brush tool, pointer event handling |
| `shaders/paint_circle.wgsl` | Reference for SDF + selection masking shader patterns |
