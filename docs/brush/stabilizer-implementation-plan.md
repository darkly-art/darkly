# Stroke Stabilizer Implementation Plan

## Context

The stabilizer is the defining feature of the brush engine — retroactive stroke reshaping where the pen tip is always at the cursor (zero lag), but the stroke behind the pen continuously reshapes as direction changes. Feels like piping frosting / pulling taffy. Procreate is the only app that has achieved this; they are closed source. We design from first principles.

Currently, dabs render directly onto the layer texture with no way to undo mid-stroke. The stabilizer requires a stroke buffer (separate texture) so dabs can be rewound and re-rendered.

## Architecture

Three layers, each ignorant of the others' internals:

1. **Stabilizer** — takes raw input, produces stabilized polyline, diffs against previous frame → divergence point
2. **Save-point system** — stroke buffer texture + per-dab cumulative bounding boxes → O(1) rewind via GPU copy
3. **Brush engine** — renders dabs into stroke buffer, unaware of rewind/stabilization (unchanged)

Per-frame flow: `raw input → stabilize → diff → rewind stroke buffer → re-render tail → composite onto layer`

## Algorithm: Iterative Laplacian Relaxation

Maintain a polyline of all raw input positions. On each new input:
1. Append to polyline
2. Run N iterations of Laplacian smoothing on interior points (first + last pinned)
3. Each iteration: `point[i] = lerp(point[i], avg(point[i-1], point[i+1]), strength)`
4. Sensor values (pressure, tilt, etc.) smoothed the same way
5. Diff against previous frame's polyline → find divergence point (walk backward from tip until delta < 0.5px)

Why this algorithm: simple, O(N × iterations), produces retroactive reshaping (the "taffy" feel), zero lag (tip pinned at cursor), strength=0 is exact pass-through, and the influence is local (distant points converge and stop changing).

Iterations derived from strength: `iterations = (strength * 5.0).ceil() as u32` (1-5 iterations).

## Implementation Phases

### Phase A: Stroke Buffer Infrastructure (pass-through stabilizer)

Goal: All painting goes through a stroke buffer with per-frame compositing. No stabilization yet — output identical to current pipeline.

**A1. `crates/darkly/src/brush/stroke_buffer.rs`** (new)

```
StrokeBuffer {
    texture + view          // stroke-in-progress (same size/format as canvas)
    pre_stroke_texture + view  // copy of layer before stroke started
    width, height
}
```

Methods:
- `new(device, width, height)` — create both textures
- `clear(encoder)` — clear stroke buffer to transparent
- `save_pre_stroke(encoder, layer_texture)` — copy layer → pre_stroke
- `restore_region(encoder, bbox)` — copy pre_stroke → stroke buffer at bbox (the rewind op)
- `composite_onto_layer(encoder, pipelines, queue, layer_texture, layer_view, dirty_bbox)` — copy pre_stroke → layer at bbox, then alpha-over blend stroke buffer → layer at bbox

The composite step: copy pre_stroke region to layer (texture-to-texture copy), then render pass with source-over blend of stroke buffer onto layer. Reuse existing blend shader infrastructure from `BrushPipelines` or `PaintPipelines`.

**A2. `crates/darkly/src/brush/save_points.rs`** (new)

```
DabSavePoint { cumulative_bbox: [u32; 4], vector_index: usize }
SavePointStore { points: Vec<DabSavePoint> }
```

Methods:
- `push(dab_bbox, vector_index)` — union with previous cumulative bbox, append
- `rewind_bbox(dab_index)` — cumulative bbox at given index
- `full_bbox()` — cumulative bbox of all dabs (= last save point's bbox)
- `truncate(n)` — keep only first n save points
- `clear()` / `len()`

No GPU readback needed — dab bbox computed CPU-side from position ± dab_size/2.

**A3. Wire stroke buffer into `engine/painting.rs`**

- Add `stroke_buffer: Option<StrokeBuffer>` field to `DarklyEngine`
- At lazy-init in `brush_stroke_to()`: create `StrokeBuffer`, save pre-stroke snapshot, clear stroke buffer
- Point `BrushGpuContext.canvas_view/canvas_texture` at `stroke_buffer.view()/texture()` instead of layer
- After `engine.move_to()`: call `stroke_buffer.composite_onto_layer()` to update layer for display
- In `end_stroke()`: destroy StrokeBuffer, proceed with existing undo diff

**A4. Save-point tracking in `StrokeEngine`**

- Add `save_points: SavePointStore` field
- In `place_dab()`: compute dab canvas bbox from position + dab_size, push to save_points
- Add accessor `save_points()` for external use

**A5. Tests**

- StrokeBuffer composite correctness
- SavePointStore cumulative bbox accumulation
- End-to-end: painting through stroke buffer produces same result as direct painting

### Phase B: Stabilizer Core (pure algorithm, no GPU dependency)

**B1. `crates/darkly/src/brush/stabilizer.rs`** (new)

```
Stabilizer {
    raw_points: Vec<PaintInformation>
    stabilized: Vec<PaintInformation>    // current frame
    prev_stabilized: Vec<[f32; 2]>       // previous frame positions (for diff)
    strength: f32                         // 0.0-1.0
}
```

Methods:
- `new(strength)` — create with given strength
- `push(point) -> StabilizeResult` — append raw point, run relaxation, diff, return result
- `clear()` — reset for new stroke

```
StabilizeResult {
    divergence_dab: usize       // earliest dab that needs re-rendering
    stabilized: &[PaintInformation]  // full stabilized vector
}
```

The relaxation loop:
```
let iterations = (strength * 5.0).ceil() as u32;
for _ in 0..iterations {
    for i in 1..len-1 {
        stabilized[i].pos = lerp(stabilized[i].pos, avg(stabilized[i-1].pos, stabilized[i+1].pos), strength);
        // same for pressure, tilt, speed, etc.
    }
}
```

Divergence detection: compare `stabilized[i].pos` vs `prev_stabilized[i]` walking backward from tip. First point where delta < EPSILON (0.5px) is the divergence boundary.

**B2. Tests**

- Straight line → remains straight
- Sharp turn → corner smoothed, radius increases with strength
- Divergence detection: straight then turn → divergence near turn, not at beginning
- Strength=0 → output identical to input
- First and last points always pinned (never move)
- Sensor values smoothed consistently with positions

### Phase C: Integration — Stabilizer + Rewind + Re-render

**C1. Modify `StrokeEngine` to use stabilizer**

Remove: `smoothing_weight`, `smoothed_pos`, `last_point` (the old forward-only smoothing)

New flow in `move_to()`:
1. `self.record.push(raw)`
2. `let result = self.stabilizer.push(raw)`
3. Return `result` to caller (painting.rs)

New method `render_along_polyline(points, start_index, gpu)`:
- Walk the stabilized polyline from `start_index` to tip
- Compute derived values (distance, angle, speed) between consecutive stabilized points
- Interpolate + place dabs at spacing intervals (reuse existing spacing logic)
- Track save points

**C2. Orchestration in `painting.rs`**

New `brush_stroke_to()` flow:
1. Lazy-init: create StrokeEngine + StrokeBuffer
2. Call `engine.stabilize(raw)` → `StabilizeResult { divergence_dab, stabilized }`
3. Rewind: `stroke_buffer.restore_region(encoder, save_points.full_bbox())`
4. Clear save points: `engine.save_points.clear()`
5. Reset dab state (dab_count, accumulated_distance, leftover_distance)
6. Re-render: `engine.render_along_polyline(stabilized, 0, gpu)` — renders all dabs from scratch along new stabilized vector
7. Composite: `stroke_buffer.composite_onto_layer()`

For v1, re-render ALL dabs on every input event. This is correct and bounded: typical stroke = 100-500 dabs, each = 2-3 GPU passes over small viewport. At 60fps, even 500 dabs is ~2ms on GPU. Optimize later if needed.

**C3. Dab-to-vector mapping**

Each dab stores `vector_index` (which polyline segment it was placed on). Polyline segment indices are stable (same count as raw input points). After re-stabilization, the positions at those indices change but the indices themselves don't. Divergence detection operates on indices.

**C4. Tests**

- Full pipeline: stroke with sharp turn → verify stroke buffer shows smooth curve
- Strength=0 → identical to pre-stabilizer pipeline
- Performance: 500-dab re-render < 50ms (assert timing bound)
- Rewind produces pixel-identical result to full re-render from scratch

### Phase D: Compositor Integration + Polish

**D1. Dirty rect tracking**

Track the union of rewind bbox + new dabs bbox per frame. Only copy/blend within that dirty region for the composite step. Reduces GPU bandwidth on long strokes where only the tip region changes.

**D2. Batched GPU submission**

Currently `submit_and_reset()` is called after every dab. For re-rendering 20+ dabs, batch all render passes into a single command buffer and submit once. Add a `batch_mode` flag to `BrushGpuContext` that defers submission.

## Stabilizer Algorithm Registry — Per-Preset Configuration

### Context

The stabilizer operates outside the per-dab node graph — it processes the full stroke history before any dabs are placed. It cannot be a graph node. But presets must be able to configure which stabilizer algorithm to use and its parameters.

The solution follows the same modular registry pattern as the veil system (`gpu/veil.rs` + `gpu/veils/*.rs`): each algorithm is a self-contained module that declares its own params and factory. A registry maps type_id → registration. The preset carries `stabilizer_type_id` + `stabilizer_params` and the engine constructs the algorithm at stroke start.

### Design

**`StabilizerRegistration`** — mirrors `VeilRegistration`:

```rust
pub struct StabilizerRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: &'static [ParamDef],
    pub from_params: fn(&[ParamValue]) -> Box<dyn StabilizerAlgorithm>,
}
```

**`StabilizerAlgorithm` trait** — replaces the current concrete `Stabilizer` struct:

```rust
pub trait StabilizerAlgorithm: Send {
    fn push(&mut self, point: PaintInformation) -> StabilizeResult;
    fn stabilized(&self) -> &[PaintInformation];
    fn len(&self) -> usize;
    fn clear(&mut self);
}
```

**`StabilizerRegistry`** — HashMap of type_id → `StabilizerRegistration`:

```rust
pub struct StabilizerRegistry {
    map: HashMap<String, StabilizerRegistration>,
}
```

Populated from auto-generated `registrations()` in `brush/stabilizers/mod.rs`.

### Per-algorithm modules

Directory: `crates/darkly/src/brush/stabilizers/`

Auto-discovered by `build.rs` (same mechanism as `gpu/veils/`, `brush/nodes/`, `tools/`).

**`laplacian.rs`** — the current algorithm, refactored into this system:

```rust
const PARAMS: &[ParamDef] = &[
    ParamDef::Float { name: "strength", min: 0.0, max: 1.0, default: 0.5 },
];

pub fn register() -> StabilizerRegistration {
    StabilizerRegistration {
        type_id: "laplacian",
        display_name: "Laplacian Relaxation",
        params: PARAMS,
        from_params: |params| {
            let strength = match params.first() {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.5,
            };
            Box::new(LaplacianStabilizer::new(strength))
        },
    }
}
```

Future algorithms (spring dynamics, spline re-fitting, etc.) are added by dropping a new `.rs` file here. No other files touched.

### Preset integration

**Add to `BrushPreset`:**

```rust
#[serde(default)]
pub stabilizer: StabilizerConfig,
```

**`StabilizerConfig`** (new struct in `brush/stabilizer.rs`):

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StabilizerConfig {
    /// Algorithm type_id. Empty string or "none" = pass-through.
    #[serde(default)]
    pub algorithm: String,
    /// Algorithm-specific parameter values.
    #[serde(default)]
    pub params: Vec<ParamValue>,
}

impl Default for StabilizerConfig {
    fn default() -> Self {
        Self { algorithm: String::new(), params: Vec::new() }
    }
}
```

Default is no stabilization (empty algorithm = pass-through). Presets opt in:

```json
{
  "stabilizer": {
    "algorithm": "laplacian",
    "params": [0.6]
  }
}
```

### Engine integration

**`DarklyEngine`** gains:
- `stabilizer_registry: StabilizerRegistry` — built once at startup
- `active_stabilizer_config: StabilizerConfig` — updated when a preset is loaded

**At stroke start** (`painting.rs`):
1. Look up `active_stabilizer_config.algorithm` in the registry
2. If found, call `registration.from_params(&config.params)` → `Box<dyn StabilizerAlgorithm>`
3. Pass to `StrokeEngine::new()`
4. If not found (empty string or "none"), pass a no-op pass-through stabilizer

**`StrokeEngine`** stores `Box<dyn StabilizerAlgorithm>` instead of a concrete `Stabilizer`.

### Builtin presets

`PresetBuilder` gains:

```rust
fn set_stabilizer(&mut self, algorithm: &str, params: Vec<ParamValue>)
```

Most presets: no call (default = no stabilization). Ink Pen / Calligraphy: `set_stabilizer("laplacian", vec![ParamValue::Float(0.6)])`.

### Implementation order

1. Create `StabilizerAlgorithm` trait + `StabilizerRegistration` + `StabilizerRegistry` in `brush/stabilizer.rs` (refactor existing `Stabilizer` struct)
2. Create `brush/stabilizers/` directory with `laplacian.rs` (move current algorithm)
3. Add `generate_registry` call to `build.rs` for `brush/stabilizers/`
4. Add `StabilizerConfig` to `BrushPreset`
5. Add `stabilizer_registry` + `active_stabilizer_config` to `DarklyEngine`
6. Wire `brush_preset_load()` to store the config
7. Wire `painting.rs` to construct algorithm from registry at stroke start
8. Update `StrokeEngine` to use `Box<dyn StabilizerAlgorithm>`
9. Update `PresetBuilder` + builtin presets
10. Tests

### Files

| File | Change |
|------|--------|
| `crates/darkly/src/brush/stabilizer.rs` | Refactor: add `StabilizerAlgorithm` trait, `StabilizerRegistration`, `StabilizerRegistry`, `StabilizerConfig`. Keep `StabilizeResult`. |
| `crates/darkly/src/brush/stabilizers/laplacian.rs` | **Create** — move current Laplacian algorithm here, export `register()` |
| `crates/darkly/src/brush/stabilizers/` | **Create directory** — auto-discovered by `build.rs` |
| `build.rs` | Add `generate_registry` call for `brush/stabilizers/` |
| `crates/darkly/src/brush/preset.rs` | Add `stabilizer: StabilizerConfig` field |
| `crates/darkly/src/brush/stroke_engine.rs` | Use `Box<dyn StabilizerAlgorithm>` instead of concrete `Stabilizer` |
| `crates/darkly/src/engine/mod.rs` | Add `stabilizer_registry` + `active_stabilizer_config` fields |
| `crates/darkly/src/engine/painting.rs` | Construct algorithm from registry at stroke start (replace hardcoded `0.3`) |
| `crates/darkly/src/engine/brush_preset.rs` | Read stabilizer config from preset on load |
| `crates/darkly/src/brush/builtin_presets.rs` | `PresetBuilder::set_stabilizer()`, set per-preset configs |
| `crates/darkly/src/brush/mod.rs` | Add `pub mod stabilizers;` |

### Verification

- `StabilizerRegistry` discovers `laplacian` algorithm at startup
- Preset save/load round-trips stabilizer algorithm + params
- Old presets (no stabilizer field) load with default (no stabilization) — serde default handles this
- Default preset: no stabilization, strokes identical to raw input
- Ink Pen preset: laplacian with strength 0.6, strokes visibly smoothed
- Adding a new algorithm = one new `.rs` file in `stabilizers/`, nothing else touched
- `cargo test` passes
