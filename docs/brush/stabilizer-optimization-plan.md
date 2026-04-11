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
