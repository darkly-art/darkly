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
- `crates/darkly/src/brush/pipelines.rs` — `BrushPipelines` (procedural + composite render pipelines, follows `PaintPipelines` pattern)
- `crates/darkly/src/brush/nodes/procedural.rs` — GPU source node: size/softness/opacity/color inputs → Texture output, renders SDF circle/gaussian to dab texture
- `crates/darkly/src/brush/nodes/color_output.rs` — GPU terminal node: dab Texture + position Vec2 inputs, composites dab onto canvas layer
- `shaders/brush/procedural.wgsl` — SDF circle/gaussian dab generation
- `shaders/brush/composite.wgsl` — dab texture compositing onto canvas (positioned quad, alpha-over blend)

### Modify

- `crates/darkly/src/brush/eval.rs` — add `evaluate_gpu` to trait, add `texture_slots` to runner, add `execute_gpu(ctx)`

### Key Design

- Dab textures pre-allocated (no GPU allocation during painting)
- `BrushPipelines` separate from `PaintPipelines` — different concerns (dab gen + dab composite vs. SDF circle + gradient)
- Two render passes per dab: generate → composite

### Verify

- GPU integration test: build minimal graph (pen_input → procedural → color_output), seed sensors, execute, readback canvas, verify non-zero pixels at expected position

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

- Load Darkly, open brush builder, see default graph (pen_input → procedural → color_output)
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
- `procedural.rs` — add scatter_x, scatter_y, rotation input ports

### Verify

- In brush builder: wire pressure→curve→size, speed→remap→opacity, tilt→rotation
- Draw test strokes, verify dynamics visually
- Verify fuzzy_dab produces different values per dab, fuzzy_stroke same within stroke

---

## Later Phases (Outlined)

**7a: Stamp Tips + Texture** — `nodes/stamp.rs` (image dab source), `nodes/texture_overlay.rs` (paper grain), DabTexturePool image uploads

**7b: Smudge** — `nodes/smudge.rs` (canvas readback under dab, blend with paint), dulling vs smear modes per Krita's `KisColorSmudgeStrategy`

**7c: Smoothing + Stabilizer** — weighted average, pulled string, spring dynamics; retroactive smoothing via StrokeRecord replay (the Procreate taffy feel)

**7d: Presets** — `.darkly-brush` format (JSON graph + binary resources), save/load, default library

**7e: Stroke Re-rendering** — keep StrokeRecord in undo stack entry, "edit last stroke" mode (re-render with tweaked parameters), discard vectors on next action

**7f: Brush Builder Polish** — refined UI, curve editor widget, preset thumbnails, import/export

---

## Dependency Graph

```
Phase 1 (nodegraph infra)
    ↓
Phase 2 (brush wire types + CPU nodes)
    ↓
Phase 3 (GPU stage nodes)
    ↓
Phase 4 (stroke engine + engine integration)
    ↓
Phase 5 (WASM bridge + brush builder UI)  ← visual debugging from here on
    ↓
Phase 6 (dynamics + math nodes)  ← testable in brush builder
    ↓
Phase 7a-f (stamps, smudge, stabilizer, presets, re-rendering, UI polish)
```

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
