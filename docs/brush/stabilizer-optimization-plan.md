# Performance Fix: Long Stroke Framerate Degradation

## Context

Long brush strokes destroy framerate — starts smooth but becomes unusable after ~half a canvas of drawing. Root cause: on divergence (nearly every frame with stabilization), the entire stroke is re-rendered from scratch — every dab's brush graph is re-evaluated and re-submitted to the GPU.

The stabilizer's O(n) CPU smoothing is not the bottleneck — it's trivial arithmetic. The bottleneck is the O(n) dab re-render (brush graph eval + GPU draw per dab, per frame).

## The Fix

On each frame, after rendering dabs into the stroke buffer, snapshot the stroke buffer into a **checkpoint texture** via a same-frame GPU→GPU texture copy. Store the corresponding render state alongside the checkpoint. On divergence at index D, find the most recent checkpoint strictly before D, restore the stroke buffer from the checkpoint texture, restore the render state, and re-render only from there to tip.

The checkpoint texture is a dedicated GPU texture that is never touched by dab rendering — it is only written to by explicit copy commands. This guarantees the snapshot contains exactly the dabs that were rendered at checkpoint time, with no contamination from later dabs.

## Render State Infrastructure (already implemented)

The following was built and tested during the first implementation attempt. It is correct and reusable:

- **`RenderCheckpoint`** struct in `stroke_engine.rs` — captures `last_point`, `accumulated_distance`, `leftover_distance`, `last_dab_size`, `dab_count`
- **`capture_render_state()` / `restore_render_state()`** on `StrokeEngine`
- **`render_from_stabilized_range(gpu, start_vector_index)`** — partial re-render from any point in the stabilized polyline
- **`finalize_render_state(vector_index, state)`** on `SavePointStore` — stamps end-of-segment state on ALL save points sharing a vector index (not just the last one), so any save point can serve as a valid checkpoint
- **`checkpoint_before(div_idx)`** on `SavePointStore` — finds the nearest save point with checkpoint data strictly before the divergence index (strict `<`, not `<=`)
- **`last_point.pos` snap** in `render_from_stabilized_range` — when resuming from a non-zero index, snaps `last_point.pos` to the current `stabilized[start - 1]` position, preventing tangent discontinuities from position drift between checkpoint capture and use

## Implementation

### Step 1: Checkpoint texture in StrokeBuffer

**File:** `crates/darkly/src/brush/stroke_buffer.rs`

Add a checkpoint texture alongside the existing stroke and pre-stroke textures:

```rust
/// Snapshot of the stroke buffer at the last checkpoint.
checkpoint_texture: wgpu::Texture,
```

Same format and dimensions as `stroke_texture`. Created in `StrokeBuffer::new()`.

New methods:

```rust
/// GPU-copy the stroke buffer into the checkpoint texture.
/// Same-frame, same encoder — no async delay.
pub fn save_checkpoint(&self, encoder: &mut wgpu::CommandEncoder)

/// GPU-copy the checkpoint texture back into the stroke buffer.
/// Restores the stroke buffer to the checkpoint state.
pub fn restore_checkpoint(&self, encoder: &mut wgpu::CommandEncoder)
```

Both are `encoder.copy_texture_to_texture` — full-texture copies, no bbox needed (the GPU cost of a full-texture copy is negligible compared to dab rendering).

### Step 2: Checkpoint flag in save points

**File:** `crates/darkly/src/brush/save_points.rs`

`DabSavePoint` gains a flag indicating the checkpoint texture was snapshotted at this save point:

```rust
pub struct DabSavePoint {
    pub cumulative_bbox: [u32; 4],
    pub vector_index: usize,
    pub render_state: RenderCheckpoint,
    /// True if the checkpoint texture contains a snapshot taken at this save point.
    pub has_checkpoint: bool,
}
```

Update `checkpoint_before` to check `has_checkpoint` instead of `pixels.is_some()`.

New method:

```rust
/// Mark the save point at `index` as having a checkpoint texture snapshot.
pub fn mark_checkpoint(&mut self, index: usize)

/// Clear checkpoint flags on all save points after `index` (used after truncation,
/// since those checkpoints are invalidated).
pub fn clear_checkpoints_after(&mut self, index: usize)
```

### Step 3: Remove async readback machinery

**Files:** `stroke_buffer.rs`, `save_points.rs`, `engine/mod.rs`, `engine/rendering.rs`

Remove:
- `request_checkpoint_readback` and `write_checkpoint` from `StrokeBuffer`
- `pixels`, `pixel_bbox` fields from `DabSavePoint`
- `set_pixels` from `SavePointStore`
- `ReadbackContext::StrokeCheckpoint` variant and its handlers in `mod.rs` and `rendering.rs`

### Step 4: Orchestration in painting.rs

**File:** `crates/darkly/src/engine/painting.rs`

Replace the current divergence handling:

```rust
let result = engine.stabilize(info);

if let Some(div_idx) = result.divergence_index {
    if let Some(cp_idx) = engine.save_points.checkpoint_before(div_idx) {
        // Restore stroke buffer from checkpoint texture (same-frame GPU copy).
        self.gpu.encode("stroke-checkpoint-restore", |encoder| {
            stroke_buffer.restore_checkpoint(encoder);
        });

        // Restore render state and truncate save points after checkpoint.
        let cp = &engine.save_points.points()[cp_idx];
        let cp_render_state = cp.render_state.clone();
        let cp_vector_index = cp.vector_index;
        engine.save_points.truncate(cp_idx + 1);
        engine.restore_render_state(&cp_render_state);

        // Re-render only from after checkpoint to tip.
        let mut gpu_ctx = BrushGpuContext { ... };
        engine.render_from_stabilized_range(&mut gpu_ctx, cp_vector_index + 1);
    } else {
        // No checkpoint before divergence — full re-render.
        self.gpu.encode("stroke-rewind", |encoder| {
            stroke_buffer.clear(encoder);
        });
        engine.reset_render_state();
        let mut gpu_ctx = BrushGpuContext { ... };
        engine.render_from_stabilized_range(&mut gpu_ctx, 0);
    }
} else {
    // No divergence — render tail only.
    engine.render_from_stabilized_tail(gpu);
}

// After rendering, snapshot the stroke buffer into the checkpoint texture
// and mark the latest save point.
if !engine.save_points.is_empty() {
    self.gpu.encode("stroke-checkpoint-save", |encoder| {
        stroke_buffer.save_checkpoint(encoder);
    });
    let last = engine.save_points.len() - 1;
    engine.save_points.mark_checkpoint(last);
}
```

### Step 5 (later): Memory optimization

Deferred. A single checkpoint texture uses one canvas-worth of VRAM. For extremely long strokes where the divergence window is wide, the single checkpoint might be too recent (vector_index >= divergence_index). A ring buffer of N checkpoint textures with staggered save points would guarantee a checkpoint exists within any divergence window. Start with one checkpoint and measure before adding complexity.

## Implementation Order

1. Add `checkpoint_texture` + `save_checkpoint` + `restore_checkpoint` to `StrokeBuffer`
2. Replace `pixels`/`pixel_bbox` with `has_checkpoint` flag in `DabSavePoint`, update `checkpoint_before`
3. Remove async readback machinery (`ReadbackContext::StrokeCheckpoint`, `request_checkpoint_readback`, `write_checkpoint`, `set_pixels`, `pixel_bbox`)
4. Rewrite divergence handling in `painting.rs` to use GPU texture copy

## Files Modified

| File | Change |
|------|--------|
| `crates/darkly/src/brush/stroke_buffer.rs` | `checkpoint_texture`, `save_checkpoint`, `restore_checkpoint`; remove `request_checkpoint_readback`, `write_checkpoint` |
| `crates/darkly/src/brush/save_points.rs` | Replace `pixels`/`pixel_bbox` with `has_checkpoint`; add `mark_checkpoint`, `clear_checkpoints_after`; remove `set_pixels` |
| `crates/darkly/src/brush/stroke_engine.rs` | No changes (render state infra already done) |
| `crates/darkly/src/engine/mod.rs` | Remove `ReadbackContext::StrokeCheckpoint` |
| `crates/darkly/src/engine/rendering.rs` | Remove `StrokeCheckpoint` handler |
| `crates/darkly/src/engine/painting.rs` | Checkpoint-based divergence handling with GPU texture copy |

## Verification

- Existing stabilizer tests pass unchanged
- Checkpoint save/restore produces pixel-identical stroke buffer (GPU copy is lossless)
- Partial re-render from checkpoint matches full re-render output
- Fallback to full re-render works when no checkpoint exists before divergence
- Performance: long strokes maintain constant framerate
- No async timing issues (everything is same-frame GPU commands)

---

## First Implementation Attempt (2026-04-11) — Failed

The approach above replaces an earlier plan that used **async GPU→CPU→GPU readback** for checkpoint pixels. That plan was implemented end-to-end but failed due to a fundamental flaw. The full record is preserved below as a reference.

### Original Approach

Store checkpoint pixels per dab via async GPU readback. On divergence, upload the pixels back to the stroke buffer via `queue.write_texture`, restore render state, re-render from the checkpoint.

### Fundamental Flaw

The plan assumed checkpoint pixels could cleanly represent "all dabs up to index N." They cannot. The stroke buffer is a single alpha-blended texture — a rectangular pixel snapshot captures the composited result of every dab that touched that region, not a separable per-dab history. Because the readback is async (completes next frame), the snapshot always contains dabs beyond the checkpoint index. You cannot un-composite those extra dabs from a flat rasterized snapshot.

This single flaw cascaded into 7 bugs:

### Bug 1: `write_texture` size mismatch

**Symptom:** WebGPU error: "Required size for texture data layout exceeds the linear data size."

**Cause:** Async readback kicked off for dab_index N with full_bbox(). By next frame, divergence truncates and rebuilds save points. Dab_index N now has a different cumulative_bbox. Readback delivers pixels sized for the old bbox.

**Fix:** Added `pixel_bbox` field to track the actual readback bbox separately from `cumulative_bbox`.

### Bug 2: Half-circle dab artifacts along stroke edges

**Symptom:** Circular dabs cut in half by a bbox boundary.

**Cause:** `write_checkpoint` only overwrites the `pixel_bbox` region. Dabs placed after the checkpoint that extend outside survive as orphaned fragments.

**Fix:** Clear the entire stroke buffer before writing checkpoint pixels.

### Bug 3: Dabs diverging from the main stroke at corners

**Symptom:** Visible forking — old dabs at old positions alongside new dabs at new positions.

**Cause (A):** Checkpoint pixels contain dabs for `cp_vector_index`. Re-rendering from the same index places dabs on top — both old and new positions visible. **Cause (B):** `capture_render_state()` was called mid-segment inside `place_dab`. Restoring this state double-counts `accumulated_distance`.

**Fix:** Moved render state capture to end-of-segment. Added `finalize_render_state`. Changed re-render to start from `cp_vector_index + 1`.

### Bug 4: Huge gaps in the stroke at every curve

**Symptom:** Large missing sections wherever the path curved.

**Cause:** `finalize_render_state` only updated the last save point. Async readback could land on a mid-segment save point with placeholder render state.

**Fix:** `finalize_render_state` now updates ALL save points sharing the same vector_index.

### Bug 5: Stale dabs around corners (didn't rewind far enough)

**Symptom:** Stale dabs from old positions visible at corners.

**Cause:** `checkpoint_at_or_before` used `<=`. When `cp_vector_index == divergence_index`, checkpoint pixels contain stale dabs for that index.

**Fix:** Renamed to `checkpoint_before` with strict `<` comparison.

### Bug 6: Orphaned dabs from pre-truncation renders

**Symptom:** Gaps at sharp corners when moving quickly.

**Cause:** Fallback path used `restore_region(full_bbox())`. After checkpoint truncation, `full_bbox()` was smaller than the full stroke extent. Old dabs outside survived.

**Fix:** Changed fallback to `stroke_buffer.clear(encoder)` (full-texture clear).

### Bug 7: Broken chain — tangent discontinuity at corners

**Symptom:** Stroke looks like a broken chain link at corners.

**Cause (A):** Checkpoint's `last_point.pos` drifted — intermediate frames shifted the position. **Cause (B):** Stale readbacks landed on rebuilt save points with correct render state but wrong pixels.

**Fix (A):** Added `last_point.pos` snap in `render_from_stabilized_range`. **Fix (B):** Cancel all pending `StrokeCheckpoint` readbacks on divergence. **This disabled the optimization entirely** — divergence every frame means every readback is canceled before completion.

### Lessons Learned

1. **Pixel snapshots of alpha-blended content cannot be partially reused.** A rectangular snapshot captures the composited result of all dabs in the region. There is no way to separate "dabs before index N" from "dabs after index N." Splicing checkpoint pixels with re-rendered content produces double-blending or visible seams.

2. **Async readback + retroactive mutation = stale data.** The stabilizer changes the polyline every frame. Readbacks are 1+ frames behind. By the time pixels arrive, they're stale. Every fix (pixel_bbox, finalize, selective cancel) addressed one symptom while leaving others.

3. **The divergence window is too wide for per-frame checkpoints.** The stabilizer's divergence reach at corners extends many indices back. `checkpoint_before(div_idx)` with strict `<` rarely finds a usable checkpoint because the most recent one is typically at or after the divergence point.

4. **The render state is deeply entangled with traversal order.** Checkpointing and resuming from the middle introduces subtle inconsistencies that are difficult to reason about and test.

5. **Unit tests didn't catch any of these bugs.** All 224 tests passed at every step. The bugs were only visible during live pen input. The test suite lacks integration tests that exercise the full divergence + checkpoint + async pipeline.

6. **A correct pixel checkpoint requires a dedicated texture.** The shared stroke buffer is continuously mutated by subsequent rendering. Same-frame GPU→GPU copy into a dedicated texture is the only way to get an exact snapshot.
