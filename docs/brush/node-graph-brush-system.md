# Node Graph System for Composable Brush Engine

## Context

The first attempt at the composable brush engine (linear pipeline) was prototyped and scrapped — it wasn't tweakable or debuggable without a proper brush builder. We're starting over, building the node graph and brush builder simultaneously so every feature is tweakable from day one.

The node graph is the brush's "brain": sensor inputs (pressure, tilt, speed, etc.) flow through math/curve nodes into GPU stage parameters. The brush builder is the visual editor where users wire these up.

### Current State

- **No brush system exists** — the prototype was fully cleaned up. No `brush/` directory, no brush stages, no stroke engine.
- **Current painting** is immediate GPU circle compositing: `stroke_to()` → `gpu_stroke_to()` → `StrokeOp::PaintCircle` dispatches to `GpuPaintTarget::composite_circle()`. No dab interpolation, no dynamics, no pipeline.
- **build.rs auto-discovery** pattern proven with `veils/` and `tools/` — ready to reuse.
- **GPU infrastructure** fully operational: `EffectPipeline`, `EffectCache`, ping-pong textures, wgpu render passes (veils demonstrate the pattern).
- **ParamDef/ParamValue** system exists in `gpu/params.rs` — Float/Int/Bool with ranges.
- **Frontend** is Svelte 5 + wasm-bindgen. Veil parameter UI (auto-generated sliders/checkboxes from ParamDef metadata) is the template for node parameter editing.

---

## Architecture: Two Layers

### Layer 1: `nodegraph/` — domain-agnostic graph infrastructure

Topology, connections, validation, topological sort, compilation to execution plans, serialization. Generic over a `WireKind` trait that domains implement. This layer knows nothing about brushes, GPU, or painting.

### Layer 2: `brush/` — brush-domain nodes and execution

Defines `BrushWireType` (closed enum), individual node implementations (sensors, math, GPU stages), the brush-specific evaluation context, stroke engine, and dab placement.

---

## Wire Types (closed enum)

```rust
// crates/darkly/src/brush/wire.rs

#[derive(Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum BrushWireType {
    Scalar,   // f32
    Int,      // u32
    Bool,
    Vec2,     // [f32; 2]
    Vec4,     // [f32; 4]
    Color,    // [f32; 4] linear RGBA
    Texture,  // GPU texture handle (RGBA dab buffer)
    Mask,     // single-channel GPU texture
}
```

Implicit coercions: `Int → Scalar`, `Bool → Scalar`, `Mask → Texture`.

Runtime values split into two storage domains for performance:
- **`ScalarValue`** — 16-byte Copy enum for CPU-side data: `F32(f32)`, `U32(u32)`, `Bool(bool)`, `Vec2([f32;2])`, `Vec4([f32;4])`, `Color([f32;4])`
- **`TextureSlot(u16)`** — index into a GPU texture handle array

The CPU hot path touches only a flat `Vec<Option<ScalarValue>>` — Copy, cache-friendly, zero allocation. GPU texture handles are separate and small (typically 2–5 per graph).

---

## Core Data Structures

### WireKind trait

```rust
// crates/darkly/src/nodegraph/mod.rs

pub trait WireKind: Copy + Clone + Eq + Hash + Serialize + DeserializeOwned + 'static {
    fn name(self) -> &'static str;
    fn color(self) -> &'static str;  // CSS color for wire rendering
    fn compatible(from: Self, to: Self) -> bool;  // type checking + coercions
}
```

### Graph

```rust
// crates/darkly/src/nodegraph/graph.rs

#[derive(Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u64);

#[derive(Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct PortRef { pub node: NodeId, pub port: u16 }

#[derive(Clone, Serialize, Deserialize)]
pub struct Connection { pub from: PortRef, pub to: PortRef }

#[derive(Clone, Serialize, Deserialize)]
pub struct Graph<W: WireKind> {
    nodes: HashMap<NodeId, NodeInstance<W>>,
    connections: Vec<Connection>,
    next_id: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct NodeInstance<W: WireKind> {
    pub id: NodeId,
    pub type_id: String,
    pub position: [f32; 2],         // UI canvas coords
    pub params: Vec<ParamValue>,    // current parameter overrides
    pub input_ports: Vec<PortDef<W>>,
    pub output_ports: Vec<PortDef<W>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PortDef<W: WireKind> {
    pub name: &'static str,
    pub wire_type: W,
}
```

Operations: `add_node`, `remove_node`, `connect` (type check + cycle check), `disconnect`, `topological_sort` (Kahn's algorithm), `validate`.

### Compiler

```rust
// crates/darkly/src/nodegraph/compiler.rs

pub struct ExecutionPlan {
    pub steps: Vec<ExecStep>,
    pub slot_count: usize,
}

pub struct ExecStep {
    pub node_id: NodeId,
    pub type_id: String,
    pub input_slots: Vec<Option<usize>>,  // indices into flat value table
    pub output_slots: Vec<usize>,
    pub is_gpu: bool,
}
```

Topological sort → assign slot index to every output port → resolve input ports → emit steps in order. At runtime: a flat `Vec<Option<ScalarValue>>` indexed by slot number. Zero HashMap lookups, zero allocation per dab.

### Node Registration

```rust
// crates/darkly/src/nodegraph/registration.rs

pub struct NodeRegistration<W: WireKind> {
    pub type_id: &'static str,
    pub category: &'static str,
    pub display_name: &'static str,
    pub input_ports: &'static [PortDef<W>],
    pub output_ports: &'static [PortDef<W>],
    pub params: &'static [ParamDef],
    pub is_gpu: bool,
}
```

Each node file exports `pub fn register() -> NodeRegistration<BrushWireType>` plus a factory for its evaluator. build.rs auto-generates the `mod.rs`.

---

## Node Design

### Pen Input (single node, 16 outputs)

One "Pen Input" node with an output port per sensor: pressure, speed, distance, x_tilt, y_tilt, tilt_direction, tilt_elevation, rotation, drawing_angle, tangential_pressure, fade, time, fuzzy_dab, fuzzy_stroke, pressure_in, position (Vec2).

Special: its evaluator is a no-op — `seed_sensors()` writes PaintInformation fields directly into the node's pre-assigned output slots at the start of each dab. Only one per graph (enforced at edit time). Unused outputs cost nothing.

### Transfer Curves

Piecewise-linear interpolation with up to 8 control points. Control point data stored as node parameters (not wire values). Takes `Scalar` in, produces `Scalar` out. The curve editor in the brush builder lets users draw the curve shape.

### GPU Stage Nodes

GPU stages receive a `BrushEvalContext` with: `CommandEncoder`, `DabTexturePool`, canvas region view, `Device`, `Queue`. They read scalar parameters from the slot table, encode GPU render passes, and write to texture slots. Dab buffer ping-pong is managed through Texture port wiring.

---

## Brush Evaluation (Per-Dab)

```rust
// crates/darkly/src/brush/eval.rs

pub struct BrushGraphRunner {
    plan: ExecutionPlan,
    evaluators: Vec<Box<dyn BrushNodeEvaluator>>,
    slots: Vec<Option<ScalarValue>>,      // reused per-dab
    texture_slots: Vec<Option<TextureSlot>>,
}
```

Per-dab execution:
1. Clear slots, seed sensor values from `PaintInformation`
2. Execute CPU steps (sensors → math → curves) — read/write flat slot arrays
3. Execute GPU steps (stages) — encode render passes into CommandEncoder
4. Submit command buffer

Zero heap allocation per dab. The slot table is pre-sized at compile time.

---

## Stroke Engine

```rust
// crates/darkly/src/brush/stroke_engine.rs

pub struct StrokeEngine {
    runner: BrushGraphRunner,
    spacing: SpacingConfig,
    smoothing: SmoothingConfig,
    last_point: Option<PaintInformation>,
    accumulated_distance: f32,
}
```

Lifecycle: `begin()` → `move_to()` (repeated) → `end()`

`move_to()`:
1. Build `PaintInformation` from pointer event (position, pressure, tilt, time, etc.)
2. Apply smoothing (weighted moving average of positions)
3. Interpolate between last point and current point
4. At each spacing interval: evaluate compiled graph via `BrushGraphRunner::execute_dab()`

---

## File Organization

```
crates/darkly/src/
    nodegraph/                    # Domain-agnostic graph infrastructure
        mod.rs                    # WireKind trait, pub mod declarations
        graph.rs                  # Graph<W>, NodeInstance, Connection, graph operations
        compiler.rs               # topological sort, ExecutionPlan, ExecStep
        registration.rs           # NodeRegistration<W>, PortDef<W>

    brush/                        # Brush domain
        mod.rs
        wire.rs                   # BrushWireType, ScalarValue, TextureSlot
        eval.rs                   # BrushNodeEvaluator trait, BrushGraphRunner, BrushEvalContext
        paint_info.rs             # PaintInformation (16 sensor fields)
        stroke_engine.rs          # StrokeEngine (begin/move_to/end, spacing, smoothing, dab placement)
        dab_buffer.rs             # DabTexturePool (pre-allocated GPU textures)
        spacing.rs                # SpacingConfig
        nodes/                    # Auto-discovered by build.rs
            mod.rs                # @generated
            pen_input.rs          # All 16 sensor outputs
            constant.rs           # Constant Scalar/Color/etc
            paint_color.rs        # Current foreground color
            multiply.rs           # Scalar × Scalar → Scalar
            add.rs                # Scalar + Scalar → Scalar
            clamp.rs              # clamp(Scalar, min, max) → Scalar
            remap.rs              # remap(Scalar, in/out ranges) → Scalar
            mix.rs                # mix(a, b, t) → Scalar/Color/Vec2
            curve.rs              # Scalar through transfer curve → Scalar
            procedural.rs         # GPU source: circle/gaussian dab generation
            color_output.rs       # GPU output: composite dab onto canvas

frontend/src/
    ui/brush_builder/
        BrushBuilder.svelte       # Main container (node canvas + stroke preview)
        NodeCanvas.svelte         # SVG wires + HTML nodes, pan/zoom
        NodePalette.svelte        # Available nodes by category (from WASM registry)
        NodeWidget.svelte         # Single node: header, ports, inline params
        WireRenderer.svelte       # Bezier curves for connections
        PortWidget.svelte         # Port circle, drag to connect
    state/
        brush_graph.svelte.ts     # Reactive graph state
```

### build.rs change

Add one line:
```rust
generate_registry(&src.join("brush/nodes"), "crate::brush::node::NodeRegistration");
```

Where the generated `registrations()` returns `Vec<NodeRegistration<BrushWireType>>` (type alias `BrushNodeRegistration`).

---

## WASM Bridge

New methods on `DarklyHandle`:

```
brush_node_types() → JsValue           // all registered node types with ports/params/categories
brush_graph_validate(json) → JsValue   // validation errors (type mismatches, cycles, etc.)
brush_graph_compile(json) → bool       // compile graph JSON, set as active brush
brush_graph_default() → JsValue        // minimal default graph (procedural → color_output)
```

Graph state lives in Svelte as reactive state. On change, JSON is sent to Rust for validation/compilation. Errors are shown inline on nodes/ports in the UI.

---

## Integration with Engine

The `StrokeEngine` replaces the current `StrokeOp::PaintCircle` dispatch in `engine.rs`:

- `engine.begin_stroke()` → initializes `StrokeEngine` with compiled brush graph
- `engine.stroke_to()` → feeds position + tablet data into `StrokeEngine::move_to()` (which handles smoothing, spacing, per-dab graph evaluation)
- `engine.end_stroke()` → finalizes stroke, undo snapshot

The existing `GpuPaintTarget` / `composite_circle` / `erase_circle` system remains for non-brush tools (fill, gradient, etc.). The brush tool exclusively uses the node graph pipeline.

---

## Implementation Phases

### Phase 1 — Graph infrastructure (no GPU, no UI)
**Files:** `nodegraph/mod.rs`, `graph.rs`, `compiler.rs`, `registration.rs`
- `WireKind` trait, `Graph<W>`, `NodeInstance`, `Connection`, `PortDef`
- Graph operations: add/remove/connect/disconnect
- Cycle detection (DFS), topological sort (Kahn's)
- Compiler: `Graph<W>` → `ExecutionPlan` (slot allocation, step ordering)
- Serde round-trip
- Unit tests for all topology operations

### Phase 2 — Brush wire types + first CPU nodes
**Files:** `brush/mod.rs`, `wire.rs`, `eval.rs`, `paint_info.rs`, `nodes/pen_input.rs`, `nodes/constant.rs`, `nodes/multiply.rs`, `nodes/curve.rs`
- `BrushWireType`, `ScalarValue`, `TextureSlot`
- `BrushNodeEvaluator` trait, `BrushGraphRunner`
- `PaintInformation` struct
- build.rs addition for `brush/nodes/`
- First CPU nodes: pen_input, constant, multiply, curve
- End-to-end test: build graph in code → compile → evaluate with mock PaintInformation → assert scalar outputs

### Phase 3 — GPU stage nodes + DabTexturePool
**Files:** `brush/dab_buffer.rs`, `brush/nodes/procedural.rs`, `brush/nodes/color_output.rs`
**Shaders:** `shaders/brush/procedural.wgsl`, `shaders/brush/color_output.wgsl`
- `DabTexturePool` for pre-allocated GPU textures
- `BrushEvalContext` with CommandEncoder, texture pool, canvas region
- Procedural source: generates circle/gaussian dab from size + softness scalars
- Color output: composites dab onto canvas layer texture
- Hardcoded test graph: pressure → multiply → procedural.size, procedural.dab → color_output

### Phase 4 — Stroke engine + engine integration
**Files:** `brush/stroke_engine.rs`, `brush/spacing.rs`, `engine.rs`
- `StrokeEngine` with begin/move_to/end lifecycle
- `SpacingConfig` (proportional spacing as % of brush diameter)
- Dab interpolation along stroke path
- Smoothing (weighted moving average)
- Wire into `engine.rs`: new stroke path alongside existing `StrokeOp` dispatch
- **Visual milestone:** paint on canvas with pressure-sensitive GPU brush

### Phase 5 — WASM bridge + Svelte node editor
**Files:** `frontend/wasm/src/api.rs`, all `frontend/src/ui/brush_builder/` files, `brush_graph.svelte.ts`
- WASM exports: node_types, validate, compile, default_graph
- Node canvas with pan/zoom, drag-to-place nodes, drag-to-connect ports
- Type-checked port connections (incompatible ports dim during drag)
- Per-node parameter editing (reuse VeilItem slider/checkbox pattern)
- Live recompilation on graph change
- **Visual milestone:** build a brush in the UI, paint with it

### Phase 6 — Remaining nodes
**Files:** additional `nodes/*.rs` files
- Math: add, clamp, remap, mix
- Sensors: all remaining pen_input outputs properly seeded
- Transfer curve editor UI in brush builder
- GPU stages: stamp (image tips), blur, texture_overlay, scatter, smudge

### Phase 7 — Presets
- Serialize graph as part of `.darkly-brush` format
- Save/load brush presets
- Default preset library (round, soft, textured, smudge)

---

## Key Files to Modify

- `crates/darkly/build.rs` — add `brush/nodes` registry generation
- `crates/darkly/src/lib.rs` — add `pub mod nodegraph` and `pub mod brush`
- `crates/darkly/src/engine.rs` — integrate StrokeEngine alongside existing StrokeOp dispatch
- `frontend/wasm/src/api.rs` — WASM bridge for brush graph + stroke with tablet data
- `frontend/src/ui/LeftSidebar.svelte` — brush builder toggle/entry point

## Key Files to Reference (patterns to follow)

- `crates/darkly/src/gpu/veil.rs` — registration pattern, trait design, EffectPipeline usage
- `crates/darkly/src/gpu/veils/bokeh.rs` — complete module example (register + trait impl + GPU pipeline + cache + encode)
- `crates/darkly/src/gpu/effect.rs` — EffectPipeline, EffectCache, create_blit_pipeline
- `crates/darkly/src/gpu/params.rs` — ParamDef/ParamValue (reused directly)
- `frontend/src/ui/veils/VeilItem.svelte` — auto-generated parameter UI from ParamDef metadata

## Verification

1. **Phase 1:** `cargo test` — graph operations, cycle detection, topological sort, compilation, serde round-trip
2. **Phase 2:** `cargo test` — build graph → compile → evaluate → assert correct scalar values in slots
3. **Phase 3:** GPU test (requires wgpu device) — compile graph with procedural + color_output, encode passes, verify no panics
4. **Phase 4:** Visual — paint strokes on canvas with pressure-sensitive brush, compare to existing circle compositing
5. **Phase 5:** Visual — build a brush in the node editor, paint with it, modify a parameter, see live change
