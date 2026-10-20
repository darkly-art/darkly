# Stroke Stabilization

## Lag investigation findings

Live in-browser perf instrumentation (a since-removed `[stab-perf]` summary at `end_stroke`, and the `[frame-perf]` slow-frame log in the WASM bridge) measured a small-brush, high-stabilization stroke. The numbers below are wall-clock host time only: GPU shader cost is not measured (`web_time::Instant` resolves to `performance.now()` on WASM, which only sees the CPU side).

### Where the frame budget goes

Across all observed slow frames, `drain` (command processing) is **>99% of the frame**; `render` (compositor + present) stays well under 1 ms. The lag is host-side, not GPU-shader-side. Inside `gpu_stroke_to`, `segments` (the segment-render loop) is ~98% of per-event time; `stabilize`, `rewind`, `restore`, `tail`, and `commit` are sub-millisecond.

### Where per-dab cost goes

A representative steady-state stroke:

```
per dab top-level (avg µs):  total=57  graph_eval=3  execute_gpu=53  
                             release_all=0  flush_submit=0  post_dab=0

execute_gpu breakdown:       stamp_pass=6  composite_pass=4  
                             read_mirror_copy=4  pool_acquire=0  other=40

runner:  steps/dab=3.0  gather_inputs=5  step_outputs=1  
         eval_gpu_call=43  eval_cpu_in_gpu=0  framework=4

evaluator hotspots:  prepare_canvas_copy=4 (footprint_math=0)  
                     write_composite_uniforms=1  write_stamp_uniforms=1  
                     ctx_input=1
```

Per-event cost scales linearly with dab count at ~57 µs/dab. Max events observed: ~50 ms with ~900 dabs. There is no single hot function; cost is distributed across the per-dab cycle.

### What's NOT the bottleneck

- **Render-pass setup overhead**: stamp + composite passes total 10 µs/dab. Even N×stamp + N×composite passes don't dominate.
- **`queue.write_buffer`**: stamp + composite uniform writes total ~2 µs/dab. The WebGPU IPC per dab is real but tiny.
- **HashMap-keyed `ctx.input` lookups** in color_output: 1 µs/dab for 3 lookups.
- **`prepare_dab_canvas_copy` footprint math** (`push_dab_write_bbox`, float clip, `DabFootprint` build): 0 µs after subtracting the read-mirror copy. Sub-resolution.
- **The brush-graph runner framework**: `gather_inputs`, evaluator lookup, `EvalContext` build, output write-back together cost 10 µs/dab. Real, not dominant.
- **`queue.submit` IPC**: ~7 submits/event at 0.07 ms each = 0.5 ms/event. Not the bottleneck.

### What IS the bottleneck

The 26 µs/dab that *no* timer attributes lives in the per-dab work that's individually too cheap to time but accumulates across the cycle:

- `dab_pool` lookups (`texture_size`, `view`, `bind_group`; 5+ per dab across stamp + color_output)
- `stamp::resolve_inputs` (~10 `ctx.input` HashMap reads)
- String allocations in stamp's `vec![("dab".into(), …), ("dab_size".into(), …)]`
- The third GPU step's `evaluate_gpu` body
- Small `Arc::clone`s, struct constructions, `as_deref`s

The pattern is **death by a thousand cuts**: dozens of sub-microsecond operations per dab, each individually irreducible. There is no big lever for local optimization; every operation is already cheap. Cost scales linearly with dab count, and a small brush at high stabilization can produce 800+ dabs per event.

### Implications

Localized timer hunting cannot close this gap. The fix is structural: **stop running the per-dab cycle per dab.** The only architectures with enough leverage are ones that pay the cycle once per event, over a buffer of N dab parameters:

- **Instanced render pipeline.** Build a `Vec<DabParams>` during the segment loop, one `queue.write_buffer` for the whole batch, one draw call with N instances reading by `instance_index`. Hardware blend stays. Requires premultiplied scratch.
- **Compute dispatch.** Same N-dab buffer, one compute pass partitioning output pixels across threads. No scratch-convention change, but requires ping-pong (or a feature-gated read-write storage texture) and Porter-Duff math in shader.

Both eliminate the per-dab dispatch of the brush graph runner, the per-dab dab_pool lookups, and the per-dab encoder operations. The current per-dab cost (57 µs) becomes per-event amortized.

### Catastrophic full re-render fallbacks (resolved)

The investigation surfaced that early-in-stroke divergence indices in `[1..spacing-1]` had no preceding checkpoint and triggered full-stroke re-renders. A **`vi=0` anchor** in `CheckpointRing::compute_segment_boundaries` cut this from ~15 fallbacks per stroke to ~2.

The remaining mid-stroke fallbacks traced to two compounding defects, both since fixed:

1. **`max_divergence_window` and `find_divergence` were not co-derived.** The Laplacian stabilizer advertised a bound (`iterations * 10 + 5`) that its detector did not respect; `find_divergence` could walk all the way to `Some(0)`. The ring's coverage invariant depends on the bound being a real ceiling on `tip_vi − div_idx`, not an aspirational one. The fix derives both from the relaxation's influence model (a Gauss-Seidel Laplacian sweep propagates backward by exactly one index, so `N` sweeps reach `N` indices back; plus the newly-interior previous tip adds one). `max_divergence_window = N + 1`, and `find_divergence` walks only this window: the bound is enforced by construction.

2. **`pick_slot` evicted the lowest-`vi` slot unconditionally.** Once the ring filled, this destroyed the anchor below the divergence boundary; over time only slots near the tip survived, and a divergence reaching back to `tip − max_div` found no slot below it. The fix is anchor-protected min-gap eviction: the lowest-`vi` slot is protected while it is the sole slot satisfying `vi < tip − max_div`; among non-protected candidates, eviction picks the slot whose removal leaves the smallest worst consecutive gap. A `debug_assert!` after every save validates the coverage invariant.

A populated-ring `find_before(div_idx)` returning `None` is now impossible whenever the stabilizer's `max_divergence_window` bound holds. `full_rerender_events` counts only this case; the "initialization fallback" on the first divergence event of a stroke (an empty ring, or a divergence index of 0, which no slot can precede) is structurally unavoidable and cheap, so it is not counted.

Stabilization retroactively reshapes a stroke as the artist draws. The tip is always pinned at the cursor (zero lag), but the path behind the pen continuously smooths (the "taffy" feel, like pulling a thread through honey).

The key insight: instead of re-rendering the entire stroke every frame when earlier positions shift, a ring of GPU checkpoints tracks the stroke at segment boundaries. On each frame, the system restores the nearest checkpoint before the divergence point and re-renders only the changed tail, typically ~1/7th of the smoothing window. This keeps stabilization O(window_slice) per frame rather than O(total_stroke), so a long stroke at full strength costs the same as a short one.

## Architecture

```
  Tablet event (any number per frame)
       │
       v
  StrokeEngine.stabilize()  (records the event)
                                    ┌───────────────────────────┐
  Frame (flush_stroke)              │     StrokeBuffer          │
       │                            │  ┌─────────────────────┐  │
       v                            │  │   stroke_texture    │──│──> composite onto layer
  StrokeEngine.take_divergence()    │  │   (dabs render here)│  │
       │   Stabilizer.push_all(     │  └─────────────────────┘  │
       │    events since the last   │                           │
       │    flush): resample, relax │                           │
       │    once, predict tail; then│                           │
       │   (diff against where      │                           │
       │    each vertex was last    │  ┌─────────────────────┐  │
       │    rendered)               │  │  pre_stroke_texture │  │
       │                            │  │  (layer snapshot)   │  │
       │                            │  └─────────────────────┘  │
       │                            └───────────────────────────┘
       v
  divergence_index ──────────> CheckpointRing
       │                            │
       v                            v
  [restore best checkpoint]   [8 bbox-sized GPU textures]
       │
       v
  render_from_stabilized_range_to(start, end)
```

Tablet events are only recorded. Stabilizing and rendering happen once per frame and at pen-up, over every event that arrived since, as one batch: a tablet that samples faster than the display refreshes would otherwise rewind and replay the stroke once per event, with nothing presented in between. The frame's flush runs first in `DarklyEngine::render`, and its time is the `stroke` sub-phase of the bridge's `[frame-perf]` slow-frame log. Krita's stabilizer has the same shape with a timer in place of the frame: `paintEvent` only adds the event to a sampler, and `stabilizerPollAndPaint` drains it on each tick and once more at pen-up (`krita/libs/ui/tool/kis_tool_freehand_helper.cpp`). A stroke no frame ran during, as a headless embedder paints, is stabilized and rendered once, at pen-up.

### Stabilizer (`stabilizer.rs`, `resampler.rs`, `stabilizers/`)

`stroke_stabilizer_stack` builds what a stroke runs through: the configured algorithm, fed by the resampler and wrapped in prediction when a look-ahead horizon is set. Each batch of tablet events (`push_all`):

1. The resampler turns the raw points into vertices at a fixed arc-length spacing, retracts the previous provisional tip once, and hands the algorithm every committed vertex plus the new tip in one call
2. The algorithm smooths its polyline once, committed vertices and tip alike, with the tip pinned
3. Prediction appends a short extrapolated tail

A batch leaves exactly the polyline pushing its events one at a time leaves; it only skips the intermediate polylines nothing renders.

The stabilizer is pure geometry. At each frame the stroke engine diffs its output against the positions each vertex was last rendered at (`DivergenceDiff`, in `stroke_engine.rs`) to find the **divergence index**: the earliest rendered vertex that has since moved more than 0.1 CSS pixels (a wider tolerance shows as ripple on quick wide curves, since neighbouring vertices are re-rendered at different moments).

The divergence index tells the rendering system "everything from here to the tip changed, re-render it." `None` means no rendered vertex moved: the new vertices are appended and rendered with no rewind. Dabs lie on the straight segment between consecutive vertices, so a segment is final once both its endpoints exist. An unstabilized stroke therefore never rewinds and never saves a checkpoint.

Diffing against rendered positions rather than the previous event bounds how stale a rendered vertex can get: a vertex that drifts a little on every event is re-rendered once its accumulated drift reaches the epsilon.

**Resampling** (`resampler.rs`). Smoothing algorithms work on vertex indices, while raw samples arrive at whatever rate the platform delivers, so the spacing of raw samples depends on the event rate and the pen speed. The resampler commits a vertex every 6 CSS pixels of raw arc length (`RESAMPLE_SPACING_CSS_PX`), converted to canvas pixels through the device pixel ratio and the zoom at stroke start. An index therefore means the same distance on screen on every platform, at every pen speed, zoom and display density. The latest raw sample is pushed into the algorithm's polyline as a provisional tip (zero lag, and the algorithm smooths right up to it) and is retracted and replaced until the pen has travelled one spacing. At most 8 vertices commit per event (`MAX_COMMITS_PER_SAMPLE`); a longer jump gets 8 vertices spread evenly along it, which is the one place the output still depends on input density. The resampler's divergence window is the algorithm's: after the provisional tip is retracted, the first vertex an event pushes lands where that tip was, so the event moves nothing further than the algorithm's window behind the tip as it stood before the event, and the commits that follow start further on.

**Prediction** (`PredictingStabilizer`). Appends 3 points past the real tip along the recent heading, spanning the look-ahead horizon at the recent pen speed. Its window is the inner window plus 3.

**Laplacian relaxation** (`stabilizers/laplacian.rs`) is the current algorithm. It runs N Gauss-Seidel sweeps over the interior points, with first and last points pinned; each sweep moves a point onto the chord between its neighbours at its own arc-length proportion (the midpoint when they are equally spaced), so smoothing changes shape without sliding points along the stroke. Each point is pulled toward that chord in proportion to the pen's speed as it passed (full at 500 CSS px/s and above), so fast motion is smoothed in full, a slow pivot keeps its corner and a stopped pen is exact. Repeated averaging is a diffusion whose radius grows with the square root of the sweep count, so `sweeps = ceil(strength^2 * 160)`: strength 0 is pass-through and the visible smoothing grows about linearly with the slider, reaching roughly ten vertex spacings (60 CSS px of corner cut) at strength 1, the same at any pen speed.

The result is defined as relaxing the whole polyline from its raw points after every batch, but a batch computes only the vertices the influence bound below says can change, from `N + 1` behind the previous tip. In a forward Gauss-Seidel sweep, vertex `i` after sweep `k` depends only on raw points `0..=i + k`, so vertex `len - N - 2` of a relaxed polyline depends only on `0..=len - 2`: no later point, nor retracting the provisional tip, changes it. Its per-sweep values are recorded and replayed as the next window's left neighbour. The windowed result is bit-identical to the from-scratch one, at `N x (M + N)` vertex updates for a batch of `M` vertices whatever the stroke's length: `N x (N + 1)` for one vertex, and never more than relaxing a batch's vertices one at a time. Relaxing the whole polyline every push made a long stroke at high strength lag further the longer it got; relaxing a window per vertex spent most of a headless stroke's CPU on polylines nothing rendered. A naive window that reads its left neighbour at its settled value is not equivalent: the from-scratch run reads that neighbour while it is still converging, and at `N = 160` the two differ by up to 17 px.

Each stabilizer also reports `max_divergence_window()`: a *true* upper bound on how far behind the tip as it stood before a push that push can move a vertex. Measured from the earlier tip, the bound holds over any number of pushes, so the stroke engine's diff walks only the window behind the tip as last rendered, however many events arrived since. The checkpoint ring depends on this being a real ceiling: violating it breaks coverage and degrades re-render to `O(total_stroke)`. A debug assertion in the diff checks that nothing behind the window moved.

For Laplacian relaxation: a Gauss-Seidel sweep propagates backward by exactly one index (forward in-sweep updates do not move information backward). `N` sweeps reach `N` indices back. The previous tip (formerly pinned, now interior) is itself a perturbation, so the earliest possibly-divergent index is `len − 2 − N`, giving `max_divergence_window = N + 1`. The bound is conservative by one: the previous tip's first sweep reads only unchanged inputs, so `len − 1 − N` is the tight index. The windowed relaxation touches exactly the bounded range, so the bound is enforced by construction.

### Stroke Buffer (`stroke_buffer.rs`)

Dabs render into a dedicated `stroke_texture` instead of directly onto the layer. A `pre_stroke_texture` holds the layer state before the stroke began. Each frame, the stroke buffer is composited over the pre-stroke snapshot onto the layer via a fullscreen composite pass.

This separation is what makes rewind possible: clearing the stroke texture and re-rendering dabs produces a clean result without contamination from previous frames.

### Save Points (`save_points.rs`)

Every dab records a `DabSavePoint`, cheap per-dab CPU metadata (a few numbers, no allocations):
- **cumulative_bbox**: union of all dab bounding boxes from the start of the stroke through this dab
- **vector_index**: which polyline point this dab was placed on
- **render_state**: a `RenderCheckpoint` snapshot of the engine's interpolation state (last_point, accumulated_distance, leftover_distance, dab_size, dab_count)

The render state is finalized at the end of each vector index segment (not per-dab), so any save point for a given vector index can serve as a valid resume point.

**Why save points exist alongside checkpoints:** Save points are the *index*, checkpoints are the *data*. The index is cheap (a few fields per dab), so we keep one per dab. The data is expensive (GPU texture copies), so we only keep 8 spread across the divergence window. The checkpoint ring depends on save points for three things:

1. **What region to copy**: each save point carries its dab's own footprint as well as the cumulative bbox, so `dirty_between(a, b)` and `dirty_after(a)` give the region in which the scratch differs between two save points. That is what a checkpoint save or restore copies; the cumulative bbox (`full_bbox`) is the undo damage rect
2. **Where to truncate on restore**: when restoring from a checkpoint, we `save_points.truncate(cp.save_point_index + 1)` to discard invalidated save points, then re-rendering builds them fresh
3. **What engine state to resume with**: the engine's interpolation state (spacing, accumulated distance, last position) is mutated by every dab and can't be reconstructed from position alone; the save point's `render_state` is the only way to resume mid-stroke without starting from scratch

### Checkpoint Ring (`checkpoint_ring.rs`)

A ring buffer of 8 GPU texture slots, each holding the stroke buffer (and the terminal's channels) as of a specific save point over a **frame**: the layer's canvas extent when the slot was allocated. Canvas coordinates are stable across mid-stroke layer growth, so a frame never moves; a save under a grown extent reallocates. Frames are layer-sized rather than bbox-sized so a stroke never reallocates its slots mid-way (every slot would do so in the same event, each a fresh texture plus a frame-sized copy, which measured as a dropped frame), at the price of one layer-sized copy per slot the first time a stroke uses it.

**Slot content**: a slot records which save point its textures equal and a *stale* rect where they do not. The invariant: with `content = Some(c)`, the textures equal the scratch as of save point `c.save_point_index` (relative to the current dab list) everywhere in the frame outside `c.stale`; a valid slot's content index is its own and its stale rect is empty. A reallocation, or `clear()` at stroke end, sets the content to unknown, and the next save into the slot copies its whole frame.

**Saving**: the slot keeps its textures across saves, and the copy is only what they lack: the stale rect plus the region dirtied between the save point the slot holds and the one being saved (`dirty_between`). A slot whose frame is not the current layer extent, or whose formats differ from the terminal's, is reallocated and takes one frame-sized copy, as does a slot a stroke uses for the first time. Between two save points the scratch differs only inside the footprints of the dabs between them (every write a dab makes lies inside the footprint it publishes through `push_write_bbox`), so this copy makes the slot exact over its whole frame.

**Restoring**: the region a rewind to checkpoint `k` undoes is the footprint of every dab after `k` (`dirty_after`, read before the save points are truncated). Outside it the scratch already equals the checkpoint. Inside it, the part the slot's frame covers is copied back from the slot; the rest, if any, is reset to the terminal's baseline first (`begin_stroke` over just that rect: a zero fill for paint, a re-seed from the pre-stroke layer for a warp or smudge terminal), since no dab at or before `k` could have written there. The reset and the copy go in one submission.

**Invalidation keeps the content truthful**: when a rewind to `k` invalidates the slots at or after the divergence index, each of them equals the restore point everywhere outside the rewound region (its dabs after `k` were discarded, and all of them lie inside that region), so its content is rewritten to `k` with the region added to its stale rect. Valid slots sit at or below the restore point and keep both their texels and their index.

**Spacing**: Checkpoints are nominally spaced `max_divergence_window / 7` vector indices apart. The intent: 8 slots at spacing-distance positions cover the divergence window with one slot just past the lower boundary and the rest packed in the volatile zone. The eviction policy doesn't strictly hold to this layout (see "Slot selection") but uses spacing to break ties when choosing what to evict.

**Slot selection**: Anchor-protected min-gap eviction. When saving a new checkpoint and all 8 slots are occupied:

- Sort slots by `vector_index`. The lowest-`vi` slot is the *anchor*.
- The anchor is **protected** while it is the sole slot satisfying `vi < tip_vi − max_divergence_window`. Evicting it would drop the lower edge of the divergence window uncovered, forcing a full re-render fallback on the next deep divergence.
- The anchor becomes **releasable** once the second-lowest slot also satisfies that strict inequality. The original anchor is now redundant; eviction is allowed.
- Among non-protected candidates, the policy picks the slot whose removal leaves the smallest worst consecutive gap (sorted by `vi`). This keeps slot density even.

A `debug_assert!` after every save checks the coverage invariant. Naive "evict the lowest" (the prior policy) destroys the anchor as soon as the ring fills and is what produced the residual ~2 mid-stroke fallbacks per stroke before the redesign.

**Invalidation**: When restoring from a checkpoint, only checkpoints **at or after the divergence index** are invalidated, not checkpoints after the restore point. This distinction is critical:

- Checkpoints between the restore point and the divergence index are **still valid**: the stroke buffer content there didn't change (only positions >= `div_idx` diverged). Preserving them allows the restore point to advance forward on subsequent frames.

- If you invalidate from the restore point instead, those intermediate checkpoints are destroyed. New checkpoints saved during re-render land within the divergence zone and get invalidated next frame. The restore point never advances; it's stuck at the same old checkpoint while the tip moves further away, causing the re-render range to grow linearly over time.

### Checkpoint Ring Invariants

Two invariants (one for correctness, one for performance) together make full-stroke re-render fallback impossible by construction whenever the stabilizer's `max_divergence_window` bound holds.

1. **Coverage (correctness).** After every save, there exists a valid slot with `vi < tip_vi − max_divergence_window`. That slot is what `find_before(div_idx)` returns for the worst-case `div_idx = tip_vi − max_divergence_window`; the slot's existence guarantees no fallback. The anchor-protected min-gap eviction policy in `pick_slot` is the load-bearing mechanism; it never evicts the sole anchor and never picks a victim that breaks the invariant. A `debug_assert!` after every save enforces it in debug builds.

2. **Density (performance).** Consecutive valid slot gaps (sorted by `vi`) stay close to `spacing = max_divergence_window / 7`. The min-gap eviction picks the most-clustered slot, which keeps the layout even. Density bounds per-event re-render cost at roughly `spacing` dabs.

3. **Scoped invalidation.** `invalidate_from(div_idx)`, not `invalidate_from(restore_point + 1)`. The stroke buffer content between the restore point and the divergence index is identical before and after the re-render (same raw positions → same dabs). Over-invalidating destroys these valid checkpoints, preventing the restore point from advancing toward the tip on subsequent frames. With scoped invalidation, the new checkpoints saved during segment-by-segment re-render survive the next frame, and the restore point converges toward the tip within a few frames of any disruption.

### Per-Frame Flow (`painting.rs`)

A tablet event only feeds the stabilizer (`brush_stroke_to`). Each frame, and pen-up, runs `flush_stroke` once over every event since the last flush. The stroke's first flush runs the terminal's whole-scratch prologue, then the flush follows one of three paths:

**Divergence with checkpoint available:**
1. `checkpoint_ring.find_before(div_idx)`: pick the best checkpoint and the region its rewind undoes (every dab after it)
2. Reset that region to the terminal's baseline where the checkpoint's frame does not cover it (`begin_stroke` over the rect), then `checkpoint_ring.restore`: copy the region back from the checkpoint; one submission for both
3. Truncate save points and restore engine render state
4. Invalidate stale checkpoints and mark what their textures still equal
5. Compute segment boundaries based on divergence window
6. Render each segment and save a checkpoint at its boundary in the same submission (a copy recorded after the segment's pass reads the pass's result, so a save never submits on its own)
7. Composite stroke buffer onto layer

**Divergence without checkpoint (an emptied ring, or a divergence at index 0):**
1. Clear stroke buffer entirely
2. Reset render state and save points
3. Full re-render from index 0 in segments, saving checkpoints along the way
4. Composite

**No divergence (strength 0, the stroke's first flush, or no rendered vertex moved):**
1. Render the newly appended vertices, continuing from the engine's render state
2. Save checkpoints in the same submissions, only when the stabilizer can diverge at all (`max_divergence_window > 0`): the first flush lays them across what it renders, from the `vi = 0` anchor up, even when it renders a single vertex; a later one saves at the tip if enough distance has passed since the newest
3. Composite

The pen-up flush takes the same paths but saves no checkpoint, since no later sample can diverge, and renders its range as one segment. A stroke no frame ran during, such as a headless embedder's, therefore costs the prologue, one segment and the commit at any strength, plus a submission per `MAX_DABS_PER_PHASE` dabs.

For the Laplacian, a rendered vertex stays put only when its neighbours are collinear and equally spaced, so at strength above 0 most frames still take a divergence path; the pinned tip being replaced in place is itself a divergence at the tip's own index.

## Performance Characteristics

| Metric | Naive approach | With checkpoint ring |
|--------|---------------|---------------------|
| Re-render cost per frame | O(total_stroke_dabs) | O(divergence_window / 8), once per frame however many events arrived |
| Relaxation cost per event | O(sweeps × total_stroke_vertices) from scratch | O(sweeps²), the window only |
| VRAM per checkpoint | N/A | frame_area * bytes per texel, per ground |
| Total checkpoint VRAM | N/A | 8 * frame_area * bytes per texel, per ground |
| CPU overhead | Minimal | Minimal (ring bookkeeping, a rect union per dab in the replay window) |
| GPU overhead per save | N/A | one copy of the region dirtied since the slot was last written (one layer-sized copy the first time a stroke uses the slot) |
| GPU overhead per restore | N/A | one copy of the region the rewind undoes, plus a zero fill of it when the slot's frame does not cover it |

The frame is the layer (8 x 1920 x 1080 texels per ground at 1080p, allocated once per layer size), and the per-event GPU work follows the dabs between two save points rather than the frame: for a stroke that crosses the canvas the cumulative bbox reaches most of it within a second, and copying it on every save and restore was most of a stroke's per-event GPU time.

## File Map

| File | Role |
|------|------|
| `brush/stabilizer.rs` | `StabilizerAlgorithm` trait, `PassThrough`, `PredictingStabilizer`, `stroke_stabilizer_stack`, `StabilizerConfig`, `StabilizerRegistry` |
| `brush/resampler.rs` | `ResamplingStabilizer` - fixed arc-length spacing ahead of the algorithm |
| `brush/stabilizers/laplacian.rs` | Laplacian relaxation implementation |
| `brush/stroke_engine.rs` | `StrokeEngine` - drives stabilizer + dab placement + render state; `DivergenceDiff` - where each vertex was last rendered, and the per-frame diff against it |
| `brush/stroke_buffer.rs` | `StrokeBuffer` - stroke and pre-stroke GPU textures, composite |
| `brush/save_points.rs` | `SavePointStore` - per-dab cumulative bbox + render state |
| `brush/checkpoint_ring.rs` | `CheckpointRing` - ring buffer of bbox-sized GPU texture checkpoints |
| `engine/painting.rs` | Orchestration - `brush_stroke_to` records events, `flush_stroke` stabilizes them as one batch and runs divergence handling, segmented rendering and the checkpoint lifecycle once per frame |

## Adding a New Stabilizer Algorithm

1. Create `brush/stabilizers/my_algorithm.rs`
2. Implement `StabilizerAlgorithm`: `push_all()` (run once per batch; `push()` wraps it), `retract_tip()`, `stabilized()`, `max_divergence_window()`, `clear()`
3. Export `register() -> StabilizerRegistration` with params and factory
4. Done. `build.rs` auto-discovers it; the registry picks it up.

The checkpoint system is algorithm-agnostic. The only contract: `max_divergence_window()` returns a conservative upper bound on how far behind the tip before a push that push can move a vertex. The stroke engine's `DivergenceDiff` finds what moved within it, and the ring spaces checkpoints accordingly. An algorithm whose push costs grow with the stroke will lag on long strokes: it can afford to recompute only its window.

An algorithm in a stroke receives vertices at a fixed arc-length spacing from the resampler, so it can reason about its reach in spacing units.
