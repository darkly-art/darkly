# Stroke Stabilization

```
TODO:
- Debug lag on long+fast strokes when stabilization is turned up high
     - When we doubled the max stabilization value, did we inadvertently reintroduce the possibility of fallback to full stroke-redraws? This existed in a careful balance.
     - When we get behind in rendering, does there become a backlog of queued inputs? Are we trying to render all of them? Is there a way to detect when rendering falls behind and discard all but the most recent input event?
- Is the stabilization engine in screen-space (dependent on inputs), or in canvas space (dependent on DPI)? Ideally it should look identical to the user no matter what zoom level they're at or whether the canvas size is 720p or 4K.
- A better solution to "higher" stabilization may be tweaking the time element instead of the length one. Right now, faster strokes = more stabilization. Is that dial hard-coded? Can it be customized? Or is it naturally this way, just as a function of the number of input points?
```

## Lag investigation findings

Live in-browser perf instrumentation (the `[stab-perf]` summary at `end_stroke` and the `[frame-perf]` slow-frame log in the WASM bridge) measured a small-brush, high-stabilization stroke. The numbers below are wall-clock host time only — GPU shader cost is not measured (`web_time::Instant` resolves to `performance.now()` on WASM, which only sees the CPU side).

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

- **Render-pass setup overhead** — stamp + composite passes total 10 µs/dab. Even N×stamp + N×composite passes don't dominate.
- **`queue.write_buffer`** — stamp + composite uniform writes total ~2 µs/dab. The WebGPU IPC per dab is real but tiny.
- **HashMap-keyed `ctx.input` lookups** in color_output — 1 µs/dab for 3 lookups.
- **`prepare_dab_canvas_copy` footprint math** (`push_dab_write_bbox`, float clip, `DabFootprint` build) — 0 µs after subtracting the read-mirror copy. Sub-resolution.
- **The brush-graph runner framework** — `gather_inputs`, evaluator lookup, `EvalContext` build, output write-back together cost 10 µs/dab. Real, not dominant.
- **`queue.submit` IPC** — ~7 submits/event at 0.07 ms each = 0.5 ms/event. Not the bottleneck.

### What IS the bottleneck

The 26 µs/dab that *no* timer attributes lives in the per-dab work that's individually too cheap to time but accumulates across the cycle:

- `dab_pool` lookups (`texture_size`, `view`, `bind_group` — 5+ per dab across stamp + color_output)
- `stamp::resolve_inputs` (~10 `ctx.input` HashMap reads)
- String allocations in stamp's `vec![("dab".into(), …), ("dab_size".into(), …)]`
- The third GPU step's `evaluate_gpu` body
- Small `Arc::clone`s, struct constructions, `as_deref`s

The pattern is **death by a thousand cuts**: dozens of sub-microsecond operations per dab, each individually irreducible. There is no big lever for local optimization — every operation is already cheap. Cost scales linearly with dab count, and a small brush at high stabilization can produce 800+ dabs per event.

### Implications

Localized timer hunting cannot close this gap. The fix is structural: **stop running the per-dab cycle per dab.** The only architectures with enough leverage are ones that pay the cycle once per event, over a buffer of N dab parameters:

- **Instanced render pipeline.** Build a `Vec<DabParams>` during the segment loop, one `queue.write_buffer` for the whole batch, one draw call with N instances reading by `instance_index`. Hardware blend stays. Requires premultiplied scratch.
- **Compute dispatch.** Same N-dab buffer, one compute pass partitioning output pixels across threads. No scratch-convention change, but requires ping-pong (or a feature-gated read-write storage texture) and Porter-Duff math in shader.

Both eliminate the per-dab dispatch of the brush graph runner, the per-dab dab_pool lookups, and the per-dab encoder operations. The current per-dab cost (57 µs) becomes per-event amortized.

### Catastrophic full re-render fallbacks (resolved)

The investigation surfaced that early-in-stroke divergence indices in `[1..spacing-1]` had no preceding checkpoint and triggered full-stroke re-renders. A **`vi=0` anchor** in `CheckpointRing::compute_segment_boundaries` cut this from ~15 fallbacks per stroke to ~2.

The remaining mid-stroke fallbacks traced to two compounding defects, both since fixed:

1. **`max_divergence_window` and `find_divergence` were not co-derived.** The Laplacian stabilizer advertised a bound (`iterations * 10 + 5`) that its detector did not respect — `find_divergence` could walk all the way to `Some(0)`. The ring's coverage invariant depends on the bound being a real ceiling on `tip_vi − div_idx`, not an aspirational one. The fix derives both from the relaxation's influence model (a Gauss-Seidel Laplacian sweep propagates backward by exactly one index, so `N` sweeps reach `N` indices back; plus the newly-interior previous tip adds one). `max_divergence_window = N + 1`, and `find_divergence` walks only this window — the bound is enforced by construction.

2. **`pick_slot` evicted the lowest-`vi` slot unconditionally.** Once the ring filled, this destroyed the anchor below the divergence boundary; over time only slots near the tip survived, and a divergence reaching back to `tip − max_div` found no slot below it. The fix is anchor-protected min-gap eviction: the lowest-`vi` slot is protected while it is the sole slot satisfying `vi < tip − max_div`; among non-protected candidates, eviction picks the slot whose removal leaves the smallest worst consecutive gap. A `debug_assert!` after every save validates the coverage invariant.

A populated-ring `restore_before(div_idx)` returning `None` is now impossible whenever the stabilizer's `max_divergence_window` bound holds. `full_rerender_events` counts only this case — the empty-ring "initialization fallback" on the first divergence event of a stroke is structurally unavoidable and cheap, so it is not counted.

Stabilization retroactively reshapes a stroke as the user draws. The tip is always pinned at the cursor (zero lag), but the path behind the pen continuously smooths — the "taffy" feel, like pulling a thread through honey.

The key insight: instead of re-rendering the entire stroke every frame when earlier positions shift, a ring of GPU checkpoints tracks the stroke at segment boundaries. On each frame, the system restores the nearest checkpoint before the divergence point and re-renders only the changed tail — typically ~1/7th of the smoothing window. This keeps stabilization O(window_slice) per frame rather than O(total_stroke), so a long stroke at full strength costs the same as a short one.

## Architecture

```
                                    ┌───────────────────────────┐
  Tablet event                      │     StrokeBuffer          │
       │                            │  ┌─────────────────────┐  │
       v                            │  │   stroke_texture    │──│──> composite onto layer
  StrokeEngine                      │  │   (dabs render here)│  │
       │                            │  └─────────────────────┘  │
       ├─> Stabilizer.push()        │  ┌─────────────────────┐  │
       │      │                     │  │  pre_stroke_texture │  │
       │      ├─> relax polyline    │  │  (layer snapshot)   │  │
       │      └─> find divergence   │  └─────────────────────┘  │
       │             │              └───────────────────────────┘
       │             v
       │      divergence_index ──────────> CheckpointRing
       │             │                          │
       │             v                          v
       │      [restore best checkpoint]   [8 bbox-sized GPU textures]
       │             │
       │             v
       └──> render_from_stabilized_range_to(start, end)
```

### Stabilizer (`stabilizer.rs`, `stabilizers/`)

Pluggable algorithm behind a `StabilizerAlgorithm` trait. Each frame:

1. Append the raw tablet point
2. Copy raw points to a working buffer
3. Run the smoothing algorithm
4. Diff against the previous frame's positions to find the **divergence index** — the earliest point that moved more than 0.5 pixels

The divergence index tells the rendering system "everything from here to the tip changed, re-render it."

**Laplacian relaxation** (`stabilizers/laplacian.rs`) is the current algorithm. It runs N iterations of Gauss-Seidel neighbor-averaging on interior points, with first and last points pinned. `iterations = ceil(strength * 10)`, so strength 0.0 is pass-through, strength 1.0 is 10 iterations.

Each stabilizer also reports `max_divergence_window()` — a *true* upper bound on `tip_vi − find_divergence().unwrap()`. The checkpoint ring depends on this being a real ceiling: violating it breaks coverage and degrades re-render to `O(total_stroke)`. The bound and the detector must be derived from the same algorithmic model.

For Laplacian relaxation: a Gauss-Seidel sweep propagates backward by exactly one index (forward in-sweep updates do not move information backward). `N` sweeps reach `N` indices back. The previous tip (formerly pinned, now interior) is itself a perturbation, so the earliest possibly-divergent index is `len − 2 − N`, giving `max_divergence_window = N + 1`. `find_divergence` walks only this window — the bound is enforced by construction.

### Stroke Buffer (`stroke_buffer.rs`)

Dabs render into a dedicated `stroke_texture` instead of directly onto the layer. A `pre_stroke_texture` holds the layer state before the stroke began. Each frame, the stroke buffer is composited over the pre-stroke snapshot onto the layer via a fullscreen composite pass.

This separation is what makes rewind possible: clearing the stroke texture and re-rendering dabs produces a clean result without contamination from previous frames.

### Save Points (`save_points.rs`)

Every dab records a `DabSavePoint` — cheap per-dab CPU metadata (a few numbers, no allocations):
- **cumulative_bbox**: union of all dab bounding boxes from the start of the stroke through this dab
- **vector_index**: which polyline point this dab was placed on
- **render_state**: a `RenderCheckpoint` snapshot of the engine's interpolation state (last_point, accumulated_distance, leftover_distance, dab_size, dab_count)

The render state is finalized at the end of each vector index segment (not per-dab), so any save point for a given vector index can serve as a valid resume point.

**Why save points exist alongside checkpoints:** Save points are the *index*, checkpoints are the *data*. The index is cheap (a few fields per dab), so we keep one per dab. The data is expensive (GPU texture copies), so we only keep 8 spread across the divergence window. The checkpoint ring depends on save points for three things:

1. **What region to snapshot** — `save_points.full_bbox()` tells the ring what bbox to GPU-copy when saving a checkpoint
2. **Where to truncate on restore** — when restoring from a checkpoint, we `save_points.truncate(cp.save_point_index + 1)` to discard invalidated save points, then re-rendering builds them fresh
3. **What engine state to resume with** — the engine's interpolation state (spacing, accumulated distance, last position) is mutated by every dab and can't be reconstructed from position alone; the save point's `render_state` is the only way to resume mid-stroke without starting from scratch

### Checkpoint Ring (`checkpoint_ring.rs`)

A ring buffer of 8 GPU texture slots, each storing the stroke buffer's **bbox region** (not the full canvas) at a specific save point.

**Saving**: GPU copies just the cumulative bbox region from the stroke texture into the slot's texture. Textures are lazily allocated with power-of-two sizing to minimize reallocation. The cumulative bbox grows monotonically as the stroke extends, so each checkpoint's bbox is larger than the previous one. Since `create_texture` is just a VRAM allocation (microseconds, no data transfer), and the power-of-two sizing means each slot reallocates at most ~log2(canvas_dimension) times over a stroke, the slots quickly stabilize at the current bbox size and stop reallocating entirely.

**Restoring**: Clear the stroke buffer to transparent, then GPU-copy the checkpoint's bbox region back. Since the stroke buffer only contains dab pixels (no background), clear + patch is an exact reconstruction.

**Spacing**: Checkpoints are nominally spaced `max_divergence_window / 7` vector indices apart. The intent: 8 slots at spacing-distance positions cover the divergence window with one slot just past the lower boundary and the rest packed in the volatile zone. The eviction policy doesn't strictly hold to this layout — see "Slot selection" — but uses spacing to break ties when choosing what to evict.

**Slot selection**: Anchor-protected min-gap eviction. When saving a new checkpoint and all 8 slots are occupied:

- Sort slots by `vector_index`. The lowest-`vi` slot is the *anchor*.
- The anchor is **protected** while it is the sole slot satisfying `vi < tip_vi − max_divergence_window`. Evicting it would drop the lower edge of the divergence window uncovered, forcing a full re-render fallback on the next deep divergence.
- The anchor becomes **releasable** once the second-lowest slot also satisfies that strict inequality. The original anchor is now redundant; eviction is allowed.
- Among non-protected candidates, the policy picks the slot whose removal leaves the smallest worst consecutive gap (sorted by `vi`). This keeps slot density even.

A `debug_assert!` after every save checks the coverage invariant. Naive "evict the lowest" (the prior policy) destroys the anchor as soon as the ring fills and is what produced the residual ~2 mid-stroke fallbacks per stroke before the redesign.

**Invalidation**: When restoring from a checkpoint, only checkpoints **at or after the divergence index** are invalidated — not checkpoints after the restore point. This distinction is critical:

- Checkpoints between the restore point and the divergence index are **still valid** — the stroke buffer content there didn't change (only positions >= `div_idx` diverged). Preserving them allows the restore point to advance forward on subsequent frames.

- If you invalidate from the restore point instead, those intermediate checkpoints are destroyed. New checkpoints saved during re-render land within the divergence zone and get invalidated next frame. The restore point never advances — it's stuck at the same old checkpoint while the tip moves further away, causing the re-render range to grow linearly over time.

### Checkpoint Ring Invariants

Two invariants — one for correctness, one for performance — together make full-stroke re-render fallback impossible by construction whenever the stabilizer's `max_divergence_window` bound holds.

1. **Coverage (correctness).** After every save, there exists a valid slot with `vi < tip_vi − max_divergence_window`. That slot is what `restore_before(div_idx)` returns for the worst-case `div_idx = tip_vi − max_divergence_window`; the slot's existence guarantees no fallback. The anchor-protected min-gap eviction policy in `pick_slot` is the load-bearing mechanism — it never evicts the sole anchor and never picks a victim that breaks the invariant. A `debug_assert!` after every save enforces it in debug builds.

2. **Density (performance).** Consecutive valid slot gaps (sorted by `vi`) stay close to `spacing = max_divergence_window / 7`. The min-gap eviction picks the most-clustered slot, which keeps the layout even. Density bounds per-event re-render cost at roughly `spacing` dabs.

3. **Scoped invalidation.** `invalidate_from(div_idx)`, not `invalidate_from(restore_point + 1)`. The stroke buffer content between the restore point and the divergence index is identical before and after the re-render (same raw positions → same dabs). Over-invalidating destroys these valid checkpoints, preventing the restore point from advancing toward the tip on subsequent frames. With scoped invalidation, the new checkpoints saved during segment-by-segment re-render survive the next frame, and the restore point converges toward the tip within a few frames of any disruption.

### Per-Frame Flow (`painting.rs`)

Each tablet event follows one of three paths:

**Divergence with checkpoint available:**
1. `checkpoint_ring.restore_before(div_idx)` — clear stroke buffer + copy best checkpoint back
2. Truncate save points and restore engine render state
3. Invalidate stale checkpoints
4. Compute segment boundaries based on divergence window
5. Render each segment, saving a checkpoint at each boundary
6. Composite stroke buffer onto layer

**Divergence without checkpoint (beginning of stroke):**
1. Clear stroke buffer entirely
2. Reset render state and save points
3. Full re-render from index 0 in segments, saving checkpoints along the way
4. Composite

**No divergence (straight-line drawing, or strength=0):**
1. Render only the new tail point
2. Save a checkpoint if enough distance has passed since the last one
3. Composite

## Performance Characteristics

| Metric | Naive approach | With checkpoint ring |
|--------|---------------|---------------------|
| Re-render cost per frame | O(total_stroke_dabs) | O(divergence_window / 8) |
| VRAM per checkpoint | N/A | bbox_area * 4 bytes |
| Total checkpoint VRAM | N/A | 8 * bbox_area * 4 bytes |
| CPU overhead | Minimal | Minimal (ring bookkeeping) |
| GPU overhead per checkpoint | N/A | 1 bbox-sized texture copy |

The bbox grows monotonically as the stroke extends, but for typical brushes it's much smaller than the full canvas (especially early in the stroke).

## File Map

| File | Role |
|------|------|
| `brush/stabilizer.rs` | `StabilizerAlgorithm` trait, `PassThrough`, `StabilizerConfig`, `StabilizerRegistry` |
| `brush/stabilizers/laplacian.rs` | Laplacian relaxation implementation |
| `brush/stroke_engine.rs` | `StrokeEngine` — drives stabilizer + dab placement + render state |
| `brush/stroke_buffer.rs` | `StrokeBuffer` — stroke and pre-stroke GPU textures, composite |
| `brush/save_points.rs` | `SavePointStore` — per-dab cumulative bbox + render state |
| `brush/checkpoint_ring.rs` | `CheckpointRing` — ring buffer of bbox-sized GPU texture checkpoints |
| `engine/painting.rs` | Orchestration — divergence handling, segmented rendering, checkpoint lifecycle |

## Adding a New Stabilizer Algorithm

1. Create `brush/stabilizers/my_algorithm.rs`
2. Implement `StabilizerAlgorithm` — `push()`, `stabilized()`, `max_divergence_window()`, `clear()`
3. Export `register() -> StabilizerRegistration` with params and factory
4. Done. `build.rs` auto-discovers it; the registry picks it up.

The checkpoint system is algorithm-agnostic. The only contract: `push()` returns a `divergence_index`, and `max_divergence_window()` returns a conservative upper bound. The ring spaces checkpoints accordingly.
