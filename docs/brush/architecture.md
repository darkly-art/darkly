# Brush System — Runtime Architecture

This is the runtime side of the brush system: what actually happens from the
moment the user puts a stylus down to the moment the layer texture changes
on-screen. For *authoring* (how to add a node or build a preset), see
[node-system.md](node-system.md).

## 30-second mental model

> A brush is a **compiled graph of nodes**. The **stroke engine** feeds the
> graph a sequence of **dabs** spaced along the pen path. Each dab runs the
> graph once: CPU nodes compute scalars, GPU nodes record render passes. All
> dab passes land in a **stroke scratch** (an RGBA texture the same size as
> the layer). At the end of each input event the scratch is pushed to the
> layer by the active terminal via its **`commit`** lifecycle hook —
> `color_output` source-over blends it onto the pre-stroke snapshot, `liquify`
> replaces the layer with the warped scratch, and future terminals do
> whatever their semantics require. The engine never decides how the commit
> works; it only provides the resources.

Four pieces to keep in mind:

1. **Graph** = static description. Nodes + wires + ports + params.
2. **Runner** = compiled graph. Knows the topological order and slot layout.
3. **Stroke engine** = per-stroke state. Pen smoothing, spacing, save points.
4. **Stroke buffer** = per-stroke scratch. Where dabs actually land during the
   stroke. The layer itself is only written once per event (at the composite).

## File map

| Responsibility | Path |
|---|---|
| Stroke lifecycle, stabilizer hookup, event plumbing | [`engine/painting.rs`](../../crates/darkly/src/engine/painting.rs) |
| Graph-side preview regen | [`engine/brush_graph.rs`](../../crates/darkly/src/engine/brush_graph.rs) |
| Stabilizer + dab spacing + save points | [`brush/stroke_engine.rs`](../../crates/darkly/src/brush/stroke_engine.rs) |
| Pre-stroke snapshot + scratch RT + composite | [`brush/stroke_buffer.rs`](../../crates/darkly/src/brush/stroke_buffer.rs) |
| Graph compile + per-dab eval | [`brush/eval.rs`](../../crates/darkly/src/brush/eval.rs) |
| GPU context passed into `evaluate_gpu` | [`brush/gpu_context.rs`](../../crates/darkly/src/brush/gpu_context.rs) |
| Pipelines, uniform rings, shared bind groups | [`brush/pipelines.rs`](../../crates/darkly/src/brush/pipelines.rs) |
| Pre-allocated dab RTs + brush-tip textures | [`brush/dab_pool.rs`](../../crates/darkly/src/brush/dab_pool.rs) |
| Node types (auto-discovered by `build.rs`) | [`brush/nodes/`](../../crates/darkly/src/brush/nodes/) |
| WGSL shaders | [`shaders/brush/`](../../shaders/brush/) |
| Built-in presets | [`brush/builtin_presets.rs`](../../crates/darkly/src/brush/builtin_presets.rs) |

## Stroke lifecycle

```
 begin_stroke(layer_id)
   │
   ├─► compile active graph → BrushGraphRunner
   ├─► create StrokeBuffer (scratch + pre-stroke snapshot of the layer)
   └─► first event only: runner.begin_stroke(gpu_ctx)
          // every terminal's begin_stroke hook fires:
          //   color_output → clear scratch to transparent
          //   liquify      → copy layer into scratch

 for each pen event (brush_stroke_to):
   │
   ├─► stabilizer.feed(event) → smoothed polyline, maybe a divergence index
   ├─► StrokeEngine.render_from_stabilized_tail(gpu_ctx)
   │      │
   │      ├─► for each dab position on the spline:
   │      │     ├─► runner.seed_sensors(PaintInformation)
   │      │     ├─► runner.execute_cpu()     // scalars (size, opacity, …)
   │      │     └─► runner.execute_gpu(ctx)  // render passes recorded in ctx.encoder
   │      │             - stamp: render tip into a dab-pool texture
   │      │             - color_output: composite dab onto stroke_scratch_view
   │      │             - liquify: sample scratch with displaced UVs, write back
   │      └─► submit encoder
   │
   └─► runner.commit(gpu_ctx)
          // every terminal's commit hook fires:
          //   color_output → source-over (scratch, pre_stroke) → layer
          //   liquify      → copy scratch → layer (replace)

 end_stroke:
   │
   ├─► save_point → undo ring (bbox + checkpoint)
   └─► drop StrokeBuffer
```

The `begin_stroke` / `commit` pair is the generic mechanism. The engine owns
no policy about what paint strokes vs warp strokes do — each terminal
declares its semantics in its own file, and `BrushGraphRunner` dispatches the
hooks at the right moments.

## Why the stroke buffer exists

The engine always creates a pair of stroke-scoped textures at stroke start:

- **`stroke_scratch_texture`** — the stroke's working surface. What it
  *means* is up to the active terminal: paint terminals fill it with
  accumulated dab contributions, warp terminals fill it with a progressively-
  deformed copy of the layer, smudge / blur / future terminals do something
  else.
- **`pre_stroke_texture`** — a snapshot of the layer at stroke start. Used
  both by the engine (as the rewind source) and by terminals that need the
  untouched canvas at commit time (e.g. `color_output` blends its scratch
  over this to avoid over-darkening overlaps).

Three independent reasons every stroke needs this pair:

1. **Alpha accumulation semantics.** Two overlapping paint dabs at full alpha
   must not read-modify-write each other — that would produce a darker
   overlap than a continuous stroke. Accumulating the *contributions* in a
   scratch and compositing once at commit time gets this right.
2. **Rewind / divergence handling.** The stabilizer can retroactively move
   previously-seen samples. The stroke engine rewinds to a save point and
   re-renders forward. On full rewind the engine calls `runner.begin_stroke`
   again, so each terminal re-initialises its scratch however it wants —
   clear, re-copy the layer, whatever.
3. **Atomic commit per event.** The user only sees changes land when the
   active terminal's `commit` hook writes to the layer. That boundary lets
   commit apply blend modes (paint/erase), replace wholesale (warp), or
   anything else — without the per-dab render path knowing anything about
   it.

For a deeper trace look at
[`engine/painting.rs::brush_stroke_to`](../../crates/darkly/src/engine/painting.rs).

## Terminal nodes

The graph is free-form, but a stroke only produces visible output if at
least one **terminal** node is reachable. A terminal is any GPU node whose
job is to put something on the layer (stroke mode) or on the preview mask
(preview mode). Terminals participate in the stroke *lifecycle* by
overriding `begin_stroke` / `commit` in addition to per-dab `evaluate_gpu`.

Non-terminal nodes (`stamp`, `circle`, `user_input`, …) don't override the
lifecycle hooks — their default impls are no-ops.

### `color_output` (paint terminal)

- `begin_stroke`: clears `stroke_scratch_view` to transparent.
- `evaluate_gpu` (per dab):
  1. `gpu.ensure_canvas_copy(rect)` copies the current *scratch* region
     into `canvas_copy_texture`. That's the background for the shader's
     Porter-Duff math (why we need the copy: WebGPU can't read and write
     the same texture in one pass).
  2. Render into `stroke_scratch_view`, reading `canvas_copy` as bg. Manual
     source-over in `composite.wgsl` — REPLACE blend at the hardware level.
- `commit`: source-over composite `stroke_scratch_texture` over
  `pre_stroke_texture`, write to `layer_view`. Applies `gpu.blend_mode`
  (paint / erase toggle).

It bails immediately in `render_mode == Preview` at every hook.

### `liquify` (warp terminal)

- `begin_stroke`: `copy_texture_to_texture(layer_texture → stroke_scratch_texture)`.
  The scratch starts as a copy of the real canvas.
- `evaluate_gpu` (per dab): `ensure_canvas_copy` snapshots the current
  scratch; the liquify shader samples the copy with displaced UVs (`pos -
  motion * falloff * strength`) and writes the warped value back to the
  scratch. Each dab sees the cumulative warp from the prior dabs.
- `commit`: `copy_texture_to_texture(stroke_scratch_texture → layer_texture)`.
  Replace — the scratch already represents the finished image.

`gpu.blend_mode` is ignored; a warp isn't paint.

### `preview_output` (preview-only terminal)

- `evaluate_gpu` (per dab): if `render_mode == Preview`, blits the upstream
  dab texture into `preview_mask_view`. Otherwise bails.
- `begin_stroke` / `commit`: no-op (preview doesn't participate in strokes).

Graphs without a `preview_output` have no hover preview — the overlay falls
back to the tool's generic cursor ring.

### One graph, two render modes

Presets typically wire both a stroke-writing terminal (`color_output` or
`liquify`) and `preview_output` from shared upstream nodes:

```
stamp.dab ─┬─► color_output.dab    (render_mode = Stroke)
           └─► preview_output.dab  (render_mode = Preview)
```

Running the graph with `render_mode == Stroke` fires `color_output`'s
lifecycle hooks and `evaluate_gpu`; `preview_output` is a no-op. Running
with `render_mode == Preview` flips the roles. Non-terminal nodes are
mode-agnostic and run identically in either pass.

## The per-dab GPU context

`BrushGpuContext` ([`gpu_context.rs`](../../crates/darkly/src/brush/gpu_context.rs))
bundles everything `evaluate_gpu` needs. Every durable surface is exposed
by its *real identity* — nodes pick what they need, the engine never
secretly swaps resources behind a single misleading name.

| Field | Purpose |
|---|---|
| `encoder` | Command encoder shared across all dabs in a segment |
| `device`, `queue` | Standard wgpu handles |
| `dab_pool` | Pre-allocated 512×512 RTs for stamp / circle outputs |
| `pipelines` | `BrushPipelines` with shaders + uniform rings |
| `layer_view`, `layer_texture` | The actual layer.  Warp terminals read/write it; paint terminals use it only at `commit`. |
| `stroke_scratch_view`, `stroke_scratch_texture` | Stroke-scoped scratch.  `Some` during a stroke, `None` in preview mode. |
| `pre_stroke_texture` | Layer snapshot taken at stroke start.  `color_output::commit` uses it as the source-over background. |
| `scratch_bind_group`, `pre_stroke_bind_group` | Pre-built bind groups exposing the scratch/snapshot for reuse with the composite pipeline (used by `color_output::commit`). |
| `preview_mask_view`, `preview_mask_size` | Preview-mode render target.  `Some` in preview, `None` in stroke. |
| `selection_bind_group` | Active selection mask (or 1×1 white when unset) |
| `resource_handles` | Named texture handles for `image` nodes |
| `blend_mode` | Engine-level paint/erase toggle.  Honoured by `color_output::commit`; ignored by warp terminals. |
| `canvas_copy_origin` | Per-dab cache for `ensure_canvas_copy`. Reset to `None` in `place_dab` before each dab. |
| `render_mode` | `Stroke` or `Preview` — terminals switch on this. |

### Uniform batching

Each pipeline owns a `DynamicUniformRing` (~256 slots). A dab's uniform block
is written to the next slot; the dynamic offset is passed to `set_bind_group`.
This means all dabs in a stroke segment go through **one** encoder and **one**
`queue.submit()`, instead of per-dab submission. When any ring nears capacity
the engine flushes mid-stroke (cheap — a few per 1000 dabs).

### `ensure_canvas_copy`

WebGPU disallows sampling and writing the same texture in one pass. So before
sampling the scratch (for Porter-Duff bg, or for liquify warp source), we do
`copy_texture_to_texture` from `stroke_scratch_texture` into
`canvas_copy_texture`. Keyed on the integer copy origin so multiple nodes
within one dab don't re-copy. Reset by `place_dab` before each dab so the
next dab sees fresh data.

Sampler is **linear**: composite reads at pixel centres (equivalent to
nearest), liquify reads with arbitrary UV displacement and needs bilinear.

## Graph compilation and evaluation

`compile_graph(&graph)` → `BrushGraphRunner`. Compilation does:

- Topological sort of nodes respecting wire dependencies.
- Slot-table allocation: one flat `Vec<Option<ScalarValue>>` entry per output
  port in the graph. No per-node HashMaps on the hot path.
- Pre-resolve the `pen_input` slots (so `seed_sensors()` can write directly
  without any lookup) and the `paint_color` slot.
- Precompute curve LUTs for nodes with `Curve` params.

Per dab:

1. `clear_slots()` — `None` every slot.
2. `seed_sensors(&paint_info)` — direct writes to `pen_input` slots.
3. `execute_cpu()` — walk steps, gather inputs by slot, dispatch to evaluator,
   write outputs.
4. `execute_gpu(ctx)` — same walk for GPU nodes; each records render passes.

Evaluator dispatch is a `HashMap<type_id, Box<dyn BrushNodeEvaluator>>` lookup
per step — ~5-15 steps per dab, so the HashMap cost is noise compared to the
render pass.

## Dab spacing

The stroke engine places dabs at a fixed distance along the Catmull-Rom
interpolated polyline. The distance is derived from the brush's **own
reported `dab_size`**:

```rust
for node_type in &["procedural", "stamp", "liquify"] {
    if let Some(slot) = runner.find_output_slot(node_type, "dab_size") { ... }
}
```

Any terminal-ish node that wants its footprint to drive spacing must expose
a `dab_size: Vec2` output **and** have its `type_id` listed here. Forgetting
to list it means dabs get placed one per input event instead of at uniform
intervals — strokes will look choppy.

## Undo and save points

Two cooperating structures:

- **`StrokeBuffer::save_pre_stroke`** — a full snapshot of the layer taken at
  `begin_stroke`. Owned by the stroke buffer; the layer's original pixels can
  be read back from here during undo.
- **`SavePoints`** — a per-dab log of bounding boxes + render-state
  checkpoints. Its `full_bbox()` gives the total damage rect for the stroke.

At `end_stroke`, the damage rect is registered with the undo ring. Undo
restores only the damaged region from the pre-stroke snapshot.

## Preview regen

When the brush tool is active and the cursor hovers without pressing, the
engine calls `regenerate_brush_preview()`
([`engine/brush_graph.rs`](../../crates/darkly/src/engine/brush_graph.rs)). It:

1. Short-circuits if `runner.has_preview_terminal() == false`.
2. Allocates (or re-uses) a 128×128 overlay preview mask texture.
3. Builds a `BrushGpuContext` with `render_mode: Preview`, `canvas_view`
   pointing at the preview mask.
4. Runs `seed_sensors` with a synthetic paint event, `execute_cpu`,
   `execute_gpu`.
5. Reads `BrushPreviewInfo` back from the `preview_output` node's resolved
   input slots (so the overlay knows the canvas-space half-extent and
   rotation).

If the graph has no `preview_output`, the preview mask is cleared and the
overlay draws nothing brush-shaped — the cursor just gets the tool's generic
ring.

## Warp brushes (and other non-paint terminals)

Terminals that transform the layer rather than depositing pigment —
liquify, smudge, blur, displacement, future effects — fit the system
through the **same** `begin_stroke` / `evaluate_gpu` / `commit` lifecycle
as paint, without any warp-specific code in the engine.

**Liquify as the worked example** ([`nodes/liquify.rs`](../../crates/darkly/src/brush/nodes/liquify.rs)):

- `begin_stroke`: copy `layer_texture` → `stroke_scratch_texture`. The
  scratch now starts as the *real* canvas, not a transparent surface.
- `evaluate_gpu`: `ensure_canvas_copy` snapshots the current scratch into
  `canvas_copy_texture`. The liquify shader samples the copy with a
  displaced UV (`canvas_pos - motion * falloff * strength`) and writes the
  warped value back into the scratch. Successive dabs compound because
  each one reads the scratch after the previous dab has mutated it.
- `commit`: `copy_texture_to_texture(scratch → layer_texture)`. The layer
  atomically becomes the warped image. No blend — the scratch already
  holds the finished pixels.

Stabilizer rewind works out of the box: on a full rewind the engine calls
`runner.begin_stroke` again, re-copying the layer and wiping prior warps.
Partial rewind from a checkpoint restores the scratch's bytes directly
(the checkpoint doesn't care whether those bytes represent accumulated
pigment or a warped layer).

**Designing a new non-paint terminal:**

1. Add a node under [`crates/darkly/src/brush/nodes/`](../../crates/darkly/src/brush/nodes/)
   with `is_gpu: true`.
2. Implement `evaluate_gpu` for the per-dab work. Read whatever it needs
   (scratch via `ensure_canvas_copy`, layer directly, dab_pool textures,
   …) and write to `stroke_scratch_view`.
3. Implement `begin_stroke` to initialise the scratch however the effect
   wants: clear, layer-copy, something else entirely.
4. Implement `commit` to push the scratch onto the layer: source-over,
   replace, destination-out, custom blend — whatever matches the effect.
5. Register the evaluator in [`brush/mod.rs::default_evaluators`](../../crates/darkly/src/brush/mod.rs).
6. Write a preset that wires `pen_input` to the new terminal, plus a
   `preview_output` subtree so hover feedback works.

No engine changes needed.

## Performance anchors

- One `queue.submit` per stroke segment, not per dab (dynamic uniform ring).
- `canvas_copy` cached per-dab (not per-node within a dab).
- Dab pool returns RTs to a free list after each dab — zero allocation during
  a stroke.
- Stabilizer divergence triggers partial re-render from the nearest save
  point, not full stroke re-render.

See [gpu-lessons-learned.md](../../gpu-lessons-learned.md) for the specific
pitfalls (pixel-centre offsets, readback deadlock on WebGPU, NDC stretch on
padded textures, copy-origin UV math).

## Extending the system

- **New node type** → [node-system.md](node-system.md).
- **New stabilizer algorithm** → [stabilization.md](stabilization.md).
- **New terminal behaviour** (warp, smudge, filter) → re-read the "Warp
  brushes" section above and design the engine-level routing *first*, not the
  node.
