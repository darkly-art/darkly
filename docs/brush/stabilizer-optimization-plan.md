# Performance Fix: Long Stroke Framerate Degradation

## Context

Long brush strokes destroy framerate — starts smooth but becomes unusable after ~half a canvas of drawing. Root cause: on divergence (nearly every frame with stabilization), the entire stroke is re-rendered from scratch — every dab's brush graph is re-evaluated and re-submitted to the GPU.

The stabilizer's O(n) CPU smoothing is not the bottleneck — it's trivial arithmetic. The bottleneck is the O(n) dab re-render (brush graph eval + GPU draw per dab, per frame).

## What Went Wrong

The original save point system stored cumulative bounding boxes and rewound to the **pre-stroke** canvas state. This made partial re-render impossible: once you erase dabs back to pre-stroke, you must re-render ALL dabs from scratch. The divergence index was computed but thrown away — used only as a boolean trigger for full re-render.

## The Fix

Store **checkpoints** per dab — the actual pixel content of the stroke buffer at that point, plus the render state. On divergence at index D, find the nearest checkpoint at or before D that has pixel data, restore the stroke buffer from it, restore the render state, and re-render only from there to tip.

Checkpoint pixel readback is async (GPU -> CPU arrives next frame). When looking for a checkpoint, walk backward until you find one that has pixels. If none has pixels yet (e.g. first few frames of a stroke), fall back to pre-stroke + full re-render (current behavior).

## Implementation

### Step 1: RenderCheckpoint struct

**File:** `crates/darkly/src/brush/stroke_engine.rs`

```rust
#[derive(Clone)]
pub struct RenderCheckpoint {
    pub last_point: Option<PaintInformation>,
    pub accumulated_distance: f32,
    pub leftover_distance: f32,
    pub last_dab_size: [f32; 2],
    pub dab_count: u32,
}
```

Add methods to `StrokeEngine`:

```rust
/// Capture the current render state as a checkpoint.
pub fn capture_render_state(&self) -> RenderCheckpoint

/// Restore render state from a checkpoint.
pub fn restore_render_state(&mut self, checkpoint: &RenderCheckpoint)

/// Render dabs along the stabilized polyline starting from `start_vector_index`.
/// Used for partial re-render after checkpoint restoration.
pub fn render_from_stabilized_range(&mut self, gpu: &mut BrushGpuContext, start_vector_index: usize)
```

`render_from_stabilized_range` is the same as `render_from_stabilized` but starts walking from `start_vector_index` instead of 0. `render_from_stabilized` becomes a call to `render_from_stabilized_range(gpu, 0)`.

### Step 2: Checkpoints in save points

**File:** `crates/darkly/src/brush/save_points.rs`

`DabSavePoint` gains checkpoint data:

```rust
pub struct DabSavePoint {
    pub cumulative_bbox: [u32; 4],
    pub vector_index: usize,
    /// Checkpoint: stroke buffer pixels within cumulative_bbox at this dab.
    /// None until async GPU readback completes (arrives next frame).
    pub pixels: Option<Vec<u8>>,
    /// Checkpoint: render state at this dab.
    pub render_state: RenderCheckpoint,
}
```

New methods:

```rust
/// Find the nearest checkpoint at or before the given vector index
/// that has pixel data. Returns the save point index (not vector index).
/// Walk backward until we find one with pixels. Returns None if no
/// checkpoint has pixels (fall back to pre-stroke + full re-render).
pub fn checkpoint_at_or_before(&self, vector_index: usize) -> Option<usize>

/// Store pixel data for the save point at the given index.
/// Called when async GPU readback completes.
pub fn set_pixels(&mut self, dab_index: usize, pixels: Vec<u8>)
```

### Step 3: Checkpoint read/write in StrokeBuffer

**File:** `crates/darkly/src/brush/stroke_buffer.rs`

```rust
/// Request async readback of stroke buffer pixels within bbox.
/// Returns a ReadbackRequest to submit to the scheduler.
/// The caller stores the resulting pixels in the save point when the readback completes.
pub fn request_checkpoint_readback(
    &self,
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    bbox: [u32; 4],
) -> ReadbackRequest

/// Upload checkpoint pixels back into the stroke buffer at bbox.
/// Used to restore the stroke buffer to a checkpoint state.
pub fn write_checkpoint(
    &self,
    queue: &wgpu::Queue,
    bbox: [u32; 4],
    pixels: &[u8],
)
```

`request_checkpoint_readback` uses `readback::request_readback` on the stroke texture — same pattern as flood fill. `write_checkpoint` uses `queue.write_texture` to upload CPU pixels back to the stroke texture.

### Step 4: ReadbackContext variant

**File:** `crates/darkly/src/engine/mod.rs`

Add a variant to `ReadbackContext`:

```rust
StrokeCheckpoint {
    /// Which dab index this checkpoint belongs to.
    dab_index: usize,
},
```

When the readback completes (in `poll_pending`), call `engine.save_points.set_pixels(dab_index, pixels)`.

### Step 5: Orchestration in painting.rs

**File:** `crates/darkly/src/engine/painting.rs`

Replace the current divergence handling:

```rust
let result = engine.stabilize(info);

if let Some(div_idx) = result.divergence_index {
    if let Some(cp_idx) = engine.save_points.checkpoint_at_or_before(div_idx) {
        // Restore stroke buffer from checkpoint pixels.
        let cp = &engine.save_points[cp_idx];
        stroke_buffer.write_checkpoint(queue, cp.cumulative_bbox, cp.pixels.as_ref().unwrap());

        // Restore render state and truncate save points after checkpoint.
        let render_state = cp.render_state.clone();
        let start_vector_index = cp.vector_index;
        engine.save_points.truncate(cp_idx + 1);
        engine.restore_render_state(&render_state);

        // Re-render only from checkpoint to tip.
        engine.render_from_stabilized_range(gpu, start_vector_index);
    } else {
        // No checkpoint has pixels yet — fall back to pre-stroke + full re-render.
        stroke_buffer.restore_region(encoder, engine.save_points.full_bbox().unwrap_or([0,0,0,0]));
        engine.reset_render_state();
        engine.render_from_stabilized_range(gpu, 0);
    }
} else {
    // No divergence — render tail only.
    engine.render_from_stabilized_tail(gpu);
}

// After rendering, request async checkpoint readback for the latest save point.
// Pixel data arrives next frame via ReadbackContext::StrokeCheckpoint.
let dab_index = engine.save_points.len() - 1;
let bbox = engine.save_points.full_bbox().unwrap();
let request = stroke_buffer.request_checkpoint_readback(device, encoder, bbox);
// Submit encoder, then submit readback request to scheduler.
readbacks.submit(request, ReadbackContext::StrokeCheckpoint { dab_index });
```

### Step 6 (later): Memory optimization

Deferred. Each checkpoint stores a cumulative bbox of pixels. For long strokes this grows. The stabilizer can provide a `window_size()` — the max divergence reach. Checkpoints older than `len - window_size` can be discarded. This bounds memory to `window_size` checkpoints regardless of stroke length.

## Implementation Order

1. `RenderCheckpoint` struct + `capture/restore_render_state` + `render_from_stabilized_range` in stroke_engine.rs
2. Extend `DabSavePoint` with `pixels` + `render_state`, add `checkpoint_at_or_before` + `set_pixels` in save_points.rs
3. `request_checkpoint_readback` + `write_checkpoint` in stroke_buffer.rs
4. `ReadbackContext::StrokeCheckpoint` variant + completion handler in engine/mod.rs + engine/rendering.rs
5. Rewrite divergence handling in painting.rs

## Files Modified

| File | Change |
|------|--------|
| `crates/darkly/src/brush/stroke_engine.rs` | `RenderCheckpoint`, `capture/restore_render_state`, `render_from_stabilized_range` |
| `crates/darkly/src/brush/save_points.rs` | `pixels` + `render_state` in `DabSavePoint`, `checkpoint_at_or_before`, `set_pixels` |
| `crates/darkly/src/brush/stroke_buffer.rs` | `request_checkpoint_readback`, `write_checkpoint` |
| `crates/darkly/src/engine/mod.rs` | `ReadbackContext::StrokeCheckpoint` variant |
| `crates/darkly/src/engine/rendering.rs` | Handle `StrokeCheckpoint` readback completion |
| `crates/darkly/src/engine/painting.rs` | Checkpoint-based divergence handling |

## Verification

- Existing stabilizer tests pass unchanged
- Checkpoint save/restore produces pixel-identical stroke buffer
- Partial re-render from checkpoint matches full re-render output
- Fallback to pre-stroke + full re-render works when no checkpoint has pixels
- Performance: long strokes maintain constant framerate

## Implementation Attempt (2026-04-11) — Failed

The plan above was implemented end-to-end. All existing tests passed at every step, but live testing with an actual pen revealed a cascade of visual artifacts that proved difficult to resolve. The optimization code is all active, but in practice the readback cancel (Bug 7 fix B) prevents checkpoints from ever accumulating pixels — divergence cancels every in-flight readback before it completes, so `checkpoint_before()` always returns `None` and the full-re-render fallback runs every frame. Below is a record of every bug encountered and every fix attempted.

### Bug 1: `write_texture` size mismatch

**Symptom:** WebGPU error: "Required size for texture data layout exceeds the linear data size." Artifacts inside the bbox.

**Cause:** Async readback is kicked off on frame N for dab_index=len-1 with full_bbox(). By frame N+1, divergence truncates save points and re-renders. The old dab_index now points to a rebuilt save point with a different cumulative_bbox. The readback delivers pixels sized for the old bbox, but `write_checkpoint` uses the new (different-sized) cumulative_bbox.

**Fix:** Added `pixel_bbox` field to `DabSavePoint`. The actual clamped readback bbox is stored alongside the pixels and carried through `ReadbackContext::StrokeCheckpoint`. `write_checkpoint` uses `pixel_bbox` (not `cumulative_bbox`) so pixel data dimensions always match.

### Bug 2: Half-circle dab artifacts along stroke edges

**Symptom:** Circular dabs cut in half by a bbox boundary along the edges of the stroke.

**Cause:** When restoring from a checkpoint, `write_checkpoint` only overwrites the `pixel_bbox` region. Dabs placed after the checkpoint that extend outside `pixel_bbox` survive in the stroke buffer as orphaned fragments.

**Fix:** Clear the entire stroke buffer to transparent before writing checkpoint pixels. Applied to both the checkpoint path and the fallback path.

### Bug 3: Dabs diverging from the main stroke at corners

**Symptom:** At corners where the stabilizer shifts positions most, visible forking — old dabs at old positions alongside new dabs at new positions.

**Cause (part A — double rendering):** The checkpoint pixels already contain dabs for `cp_vector_index`. `render_from_stabilized_range(gpu, cp_vector_index)` re-renders from that same index, placing dabs on top of the checkpoint pixels. For changed positions (at/near divergence), both old (pixel) and new (re-rendered) dabs are visible.

**Cause (part B — stale render state):** `capture_render_state()` was called inside `place_dab`, which runs mid-segment. At that point `last_point` is from the previous segment and `leftover_distance` hasn't been updated. Restoring this state and re-running the loop double-counts `accumulated_distance` and replays the segment incorrectly.

**Fix:** Moved render state capture to end-of-segment boundaries (after `leftover_distance` and `last_point` updates). Added `finalize_render_state(vector_index, state)` to stamp the correct state on all save points sharing that vector index. Changed re-render to start from `cp_vector_index + 1`.

### Bug 4: Huge gaps in the stroke at every curve

**Symptom:** After the Bug 3 fix, large sections of the stroke were missing wherever the path curved.

**Cause:** `finalize_render_state` only updated the LAST save point (via `update_last_render_state`). Multiple dabs share the same vector_index. An async readback could land on a mid-segment save point that still had placeholder render state. Restoring garbage state caused the engine to skip dabs.

**Fix:** Changed `finalize_render_state` to walk backward and update ALL save points sharing the same vector_index, not just the last one.

### Bug 5: Stale dabs around corners (didn't rewind far enough)

**Symptom:** Slightly less severe than Bug 3, but still visible — stale dabs from old positions at corners.

**Cause:** `checkpoint_at_or_before` used `<=` for the divergence index comparison. When `cp_vector_index == divergence_index`, the checkpoint pixels contain dabs at the old (stale) positions for that index. Re-rendering starts from `cp_vector_index + 1`, leaving the stale dabs visible.

**Fix:** Renamed to `checkpoint_before` with strict `<` comparison. Checkpoints must be from strictly before the divergence point so their pixels only contain unchanged positions.

### Bug 6: Orphaned dabs from pre-truncation renders (gaps at fast corners)

**Symptom:** Gaps in the stroke at sharp corners when moving the pen quickly.

**Cause:** The fallback (full re-render) path used `restore_region(full_bbox())` to clear the stroke buffer. But after a previous checkpoint truncation, `full_bbox()` was smaller than the full stroke extent. Old dabs outside the reduced bbox survived from pre-truncation renders.

**Fix:** Changed the fallback path to use `stroke_buffer.clear(encoder)` (full-texture clear) instead of `restore_region(full_bbox())`, matching the checkpoint path's behavior.

### Bug 7: Broken chain — tangent discontinuity at corners

**Symptom:** The stroke looks like a broken chain link at corners — two ends that don't face each other.

**Cause (part A — drifted last_point):** The checkpoint's `last_point.pos` reflects the stabilized position at `cp_vector_index` from the frame the checkpoint was captured. Between capture and use, intermediate frames may have shifted that position. The first re-rendered segment bridges from the old position to the current next point, creating a tangent discontinuity.

**Cause (part B — stale readback pixels):** Async readbacks from previous frames can land on rebuilt save points (after full re-render). The render state is correct (from `finalize`), but the pixels show old positions, creating inconsistency between pixel content and re-rendered content.

**Fix (part A):** Added position snap in `render_from_stabilized_range`: when starting from a non-zero index, update `last_point.pos` to the current `stabilized[start - 1]` position.

**Fix (part B):** Cancel all pending `StrokeCheckpoint` readbacks on divergence. **This fix disabled the optimization entirely** — divergence happens nearly every frame, so every readback is canceled before it completes, no checkpoint ever accumulates pixels, and the fallback (full re-render) runs every frame.

### Current State

The checkpoint infrastructure is implemented and compiles, but the optimization is effectively disabled. The `last_point` snap (Bug 7 fix A) remains active. The readback cancel (Bug 7 fix B) ensures correctness but prevents checkpoints from ever having pixel data.

### Root Cause Analysis

The plan had a fundamental flaw: **it assumed checkpoint pixels could cleanly represent "all dabs up to index N."**

The stroke buffer is a single alpha-blended texture. When you snapshot a rectangle, you get the composited result of every dab that touched that region — not a separable per-dab history. The plan says "restore the stroke buffer from checkpoint pixels and re-render only from there to tip." But the checkpoint pixels were read from the stroke buffer at a moment when it contained dabs *beyond* the checkpoint index (because the readback is async — by the time it completes, more dabs have been rendered). You can't un-composite those extra dabs from the pixel snapshot.

This single flaw cascaded into every bug encountered:

- **Bugs 1, 2, 3, 5** are all variants of "the checkpoint pixels contain content they shouldn't" — extra dabs beyond the checkpoint, dabs at stale positions, dabs that extend outside the bbox. Every fix tried to carve out the right subset of pixel data, but you can't — it's a flat rasterized snapshot.

- **Bug 7** is the async timing problem. The readback is always 1+ frames behind. The stabilizer changes positions every frame. By the time pixels arrive, they're stale. Canceling stale readbacks is correct but kills the optimization.

- **Bugs 4, 6** are collateral damage from fixes to the above — truncation and clearing logic that was needed to work around the pixel contamination problem.

The plan's implicit assumption was that the readback would complete *before* any more dabs were rendered into the stroke buffer, so the pixel snapshot would contain exactly dabs 0..N. On native with synchronous readback, this might work. With async readback (mandatory for WebGPU), it's a race condition by design.

The render state checkpointing work (Bug 3/4 fixes — end-of-segment capture, `finalize_render_state`, `last_point` snap) was actually correct and sound. The problem was never the render state. It was the pixels.

### Lessons Learned

1. **Pixel snapshots of alpha-blended content cannot be partially reused.** The stroke buffer accumulates dabs with alpha blending. A rectangular pixel snapshot captures the composited result of all dabs within the bbox — there is no way to separate "dabs before index N" from "dabs after index N" in a pixel snapshot. This makes it impossible to cleanly splice checkpoint pixels with re-rendered content without either double-blending or visible seams.

2. **Async readback + retroactive mutation = stale data.** The stabilizer retroactively changes the polyline on nearly every frame. Readbacks are 1+ frames behind. By the time pixels arrive, save points may have been truncated and rebuilt. The dab_index in the readback context points to a different save point than the one the readback was initiated for. Every attempted fix for this (pixel_bbox, render state finalization, selective cancellation) addressed one symptom while leaving others.

3. **The divergence window is too wide for per-frame checkpoints.** The stabilizer's divergence reach at corners can extend back many vector indices. `checkpoint_before(div_idx)` with strict `<` rarely finds a usable checkpoint because the most recent checkpoint (from the previous frame) typically has a vector_index at or after the divergence point.

4. **The render state is deeply entangled with the traversal order.** `last_point`, `accumulated_distance`, `leftover_distance`, and `dab_count` form an incremental state machine that depends on processing points in exact order. Checkpointing and resuming from the middle introduces subtle inconsistencies (double-counted distance, stale derived values, mid-segment vs end-of-segment state) that are difficult to reason about and test.

5. **Unit tests didn't catch any of these bugs.** All 224 tests passed at every step. The bugs were only visible during live pen input with the stabilizer active. The test suite lacks integration tests that exercise the stabilizer divergence + checkpoint restore + async readback pipeline end-to-end with position verification.

6. **A correct pixel checkpoint requires a dedicated texture.** To snapshot "exactly dabs 0..N" you would need to render those dabs into a separate texture that is never touched by later dabs. The single shared stroke buffer cannot serve this purpose because it is continuously mutated by subsequent rendering.
