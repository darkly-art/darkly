# Stroke Stabilization

Stabilization retroactively reshapes a stroke as the user draws. The tip is always pinned at the cursor (zero lag), but the path behind the pen continuously smooths — the "taffy" feel, like pulling a thread through honey.

This creates a problem: every frame, the stabilizer changes positions of points behind the pen, so those dabs need to be re-rendered. Without optimization, every frame re-renders the *entire* stroke from scratch, which destroys framerate after a few hundred dabs.

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

**Laplacian relaxation** (`stabilizers/laplacian.rs`) is the current algorithm. It runs N iterations of neighbor-averaging on interior points, with first and last points pinned. `iterations = ceil(strength * 5)`, so strength 0.0 is pass-through, strength 1.0 is 5 iterations.

Each stabilizer also reports `max_divergence_window()` — a conservative upper bound on how far back divergence can reach. This drives checkpoint spacing.

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

**Spacing**: Checkpoints are spaced `max_divergence_window / 7` vector indices apart. This means:
- The oldest checkpoint sits just past the maximum divergence boundary
- The remaining 7 checkpoints are densely packed in the volatile zone
- Worst-case re-render per frame: ~1/7th of the divergence window

At max stabilization strength (window = 55), that's ~8 vector indices of dabs re-rendered per frame instead of the entire stroke.

**Invalidation**: When restoring from a checkpoint, all checkpoints after it are invalidated (their positions are now stale). The re-render pass saves fresh checkpoints at segment boundaries as it goes.

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

| Metric | Without optimization | With checkpoint ring |
|--------|---------------------|---------------------|
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
