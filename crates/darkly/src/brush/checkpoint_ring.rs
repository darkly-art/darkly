//! Ring buffer of GPU texture checkpoints for partial stroke re-render.
//!
//! Each checkpoint captures the stroke buffer's bbox region at a specific
//! save point. On divergence, the best checkpoint before the divergence
//! index is restored (clear stroke buffer + copy bbox region back), and
//! only dabs after the checkpoint are re-rendered.
//!
//! The ring capacity is fixed (8 slots). Spacing between checkpoints
//! autoscales based on the stabilizer's max divergence window, so the
//! oldest checkpoint is typically just past the divergence boundary and
//! the remaining slots are densely packed in the volatile zone.

use super::stroke_engine::RenderCheckpoint;
use crate::coord::CanvasRect;
use crate::gpu::atlas::CanvasFrame;

const RING_CAPACITY: usize = 8;

/// Metadata returned when restoring from a checkpoint.
pub struct CheckpointRestore {
    pub save_point_index: usize,
    pub vector_index: usize,
    pub render_state: RenderCheckpoint,
}

/// A single checkpoint slot in the ring buffer.
struct CheckpointSlot {
    /// Bbox-sized GPU texture holding the stroke buffer snapshot.
    /// Lazily allocated; reallocated when the bbox outgrows it.
    texture: Option<wgpu::Texture>,
    /// Dimensions of the allocated texture (may be larger than bbox).
    tex_w: u32,
    tex_h: u32,
    /// The bbox region this checkpoint covers, in canvas pixel coords.
    /// Stable across mid-stroke layer growth.
    canvas_bbox: CanvasRect,
    /// Which save point this checkpoint was captured at.
    save_point_index: usize,
    /// The polyline vector index at that save point.
    vector_index: usize,
    /// Engine render state for resuming from this checkpoint.
    render_state: RenderCheckpoint,
    /// Whether this slot contains valid data.
    valid: bool,
}

impl CheckpointSlot {
    fn empty() -> Self {
        Self {
            texture: None,
            tex_w: 0,
            tex_h: 0,
            canvas_bbox: CanvasRect::from_xywh(0, 0, 0, 0),
            save_point_index: 0,
            vector_index: 0,
            render_state: RenderCheckpoint {
                last_point: None,
                accumulated_distance: 0.0,
                leftover_distance: 0.0,
                last_dab_size: [0.0, 0.0],
                dab_count: 0,
            },
            valid: false,
        }
    }

    /// Ensure the texture is at least `w × h`. Reallocate if needed.
    fn ensure_texture(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        if self.tex_w >= w && self.tex_h >= h && self.texture.is_some() {
            return;
        }
        // Allocate with some headroom to reduce reallocation frequency.
        let alloc_w = w.next_power_of_two().max(64);
        let alloc_h = h.next_power_of_two().max(64);
        self.texture = Some(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("checkpoint-slot"),
            size: wgpu::Extent3d {
                width: alloc_w,
                height: alloc_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));
        self.tex_w = alloc_w;
        self.tex_h = alloc_h;
    }
}

/// Ring buffer of checkpoint textures for O(divergence_window / N) re-render.
///
/// Three invariants keep the ring healthy (see stabilization.md for details):
///
/// 1. **Slot selection favors the tip**: overwrite the lowest vector_index
///    (furthest from tip, least useful).  Not FIFO, not "even spread."
///
/// 2. **Invalidation is scoped to divergence**: `invalidate_from(div_idx)`,
///    NOT `invalidate_from(restore_point + 1)`.  Checkpoints between the
///    restore point and the divergence index are still valid.
///
/// 3. **The restore point advances**: preserved intermediate checkpoints
///    let the next frame restore from a closer point, converging to
///    O(window/8) within a few frames of any disruption.
///
/// Violating any invariant degrades re-render cost from O(window/8) to
/// O(total_stroke) over time.  See stabilization.md § "Checkpoint Ring
/// Invariants" for the full failure modes.
pub struct CheckpointRing {
    slots: Vec<CheckpointSlot>,
}

impl Default for CheckpointRing {
    fn default() -> Self {
        Self::new()
    }
}

impl CheckpointRing {
    pub fn new() -> Self {
        let mut slots = Vec::with_capacity(RING_CAPACITY);
        for _ in 0..RING_CAPACITY {
            slots.push(CheckpointSlot::empty());
        }
        Self { slots }
    }

    /// The vector_index of the newest valid checkpoint, if any.
    pub fn newest_vector_index(&self) -> Option<usize> {
        self.slots
            .iter()
            .filter(|s| s.valid)
            .map(|s| s.vector_index)
            .max()
    }

    /// Choose which slot to overwrite for a new checkpoint.
    ///
    /// Priority: (1) an invalid slot, (2) the valid slot with the lowest
    /// vector_index — the one furthest from the tip.  Divergence only
    /// reaches max_divergence_window behind the tip, so the oldest
    /// checkpoint is the least useful and should be recycled first.
    fn pick_slot(&self) -> usize {
        if let Some(i) = self.slots.iter().position(|s| !s.valid) {
            return i;
        }
        self.slots
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| s.vector_index)
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// Save a checkpoint: copy the bbox region from the stroke texture into
    /// the next ring slot. `stroke` is the stroke buffer paired with the
    /// active layer's canvas extent (the stroke buffer is texture-aligned
    /// to the layer texture). `canvas_bbox` is the canvas-space rect to
    /// snapshot.
    pub fn save(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        stroke: &CanvasFrame<'_>,
        save_point_index: usize,
        vector_index: usize,
        canvas_bbox: CanvasRect,
        render_state: RenderCheckpoint,
    ) {
        let layer_rect = match stroke.canvas_to_layer_rect(canvas_bbox) {
            Some(r) if !r.is_empty() => r,
            _ => return,
        };
        // Use the clipped canvas rect (post-intersection) so the stored
        // bbox matches the texels actually copied.
        let clipped_canvas = match stroke.canvas_extent.intersect(canvas_bbox) {
            Some(r) => r,
            None => return,
        };

        let slot_idx = self.pick_slot();
        let slot = &mut self.slots[slot_idx];
        slot.ensure_texture(device, layer_rect.width, layer_rect.height);
        slot.canvas_bbox = clipped_canvas;
        slot.save_point_index = save_point_index;
        slot.vector_index = vector_index;
        slot.render_state = render_state;
        slot.valid = true;

        // Copy bbox region from stroke texture to slot texture.
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: stroke.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: layer_rect.x0(),
                    y: layer_rect.y0(),
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: slot.texture.as_ref().unwrap(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: layer_rect.width,
                height: layer_rect.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Find the best checkpoint strictly before `div_vector_index`.
    /// Returns the slot index of the valid checkpoint with the highest
    /// vector_index that is < div_vector_index.
    fn best_slot_before(&self, div_vector_index: usize) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None; // (slot_idx, vector_index)
        for (i, slot) in self.slots.iter().enumerate() {
            if slot.valid && slot.vector_index < div_vector_index {
                match best {
                    None => best = Some((i, slot.vector_index)),
                    Some((_, best_vi)) if slot.vector_index > best_vi => {
                        best = Some((i, slot.vector_index));
                    }
                    _ => {}
                }
            }
        }
        best.map(|(idx, _)| idx)
    }

    /// Find and restore the best checkpoint before `div_vector_index`.
    ///
    /// Copies the checkpoint's bbox region back onto the stroke buffer.
    /// **Does not clear outside the bbox** — the caller must establish the
    /// outside-bbox initial state before calling this (e.g. via
    /// `StrokeEngine::begin_stroke`, which delegates to the active
    /// terminal's lifecycle hook). For paint, that's a transparent clear;
    /// for a warp/smudge terminal, it's a copy of the pre-stroke layer; the
    /// ring doesn't care which — it only restores the mutated region.
    ///
    /// Returns the checkpoint metadata for the caller to restore engine
    /// state.
    ///
    /// `stroke` pairs the stroke buffer with the active layer's *current*
    /// canvas extent — used to translate the slot's canvas-coord bbox to
    /// texture-local coords (which may differ from save time if the layer
    /// has grown in the meantime; the stroke buffer's contents are rebased
    /// by `StrokeBuffer::grow_preserving` to track the new frame, so this
    /// translation produces the matching texture origin).
    pub fn restore_before(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        stroke: &CanvasFrame<'_>,
        div_vector_index: usize,
    ) -> Option<CheckpointRestore> {
        let slot_idx = self.best_slot_before(div_vector_index)?;
        let slot = &self.slots[slot_idx];
        let layer_rect = stroke.canvas_to_layer_rect(slot.canvas_bbox)?;
        if layer_rect.is_empty() {
            return None;
        }

        // Copy checkpoint bbox region back to stroke buffer. The caller has
        // already reset outside-bbox pixels to the terminal's starting
        // state, so only the mutated region needs restoring here.
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: slot.texture.as_ref().unwrap(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: stroke.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: layer_rect.x0(),
                    y: layer_rect.y0(),
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: layer_rect.width,
                height: layer_rect.height,
                depth_or_array_layers: 1,
            },
        );

        Some(CheckpointRestore {
            save_point_index: slot.save_point_index,
            vector_index: slot.vector_index,
            render_state: slot.render_state.clone(),
        })
    }

    /// Invalidate all checkpoints with vector_index >= threshold.
    pub fn invalidate_from(&mut self, vector_index: usize) {
        for slot in &mut self.slots {
            if slot.valid && slot.vector_index >= vector_index {
                slot.valid = false;
            }
        }
    }

    /// Invalidate all checkpoints.
    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            slot.valid = false;
        }
    }

    /// Compute the ideal checkpoint spacing for the given divergence window.
    pub fn spacing(max_divergence_window: usize) -> usize {
        if max_divergence_window == 0 {
            return 1;
        }
        (max_divergence_window / (RING_CAPACITY - 1)).max(1)
    }

    /// Compute segment boundary vector indices for a re-render from
    /// `start_vi` to `tip_vi`. Returns positions where checkpoints
    /// should be saved; includes `start_vi` only when it equals `0`
    /// (the coverage anchor described below), and always includes
    /// `tip_vi`.
    ///
    /// **Coverage invariant.** The ring must hold at least one checkpoint
    /// with `vi < div_idx` for every reachable divergence index — that's
    /// what makes partial restore possible. The stabilizer's
    /// `max_divergence_window()` bounds how far back divergence can reach
    /// from the tip, so spacing-distance checkpoints near the tip cover
    /// any `div_idx` further than `spacing` from `vi=0`. The remaining
    /// range `[1..spacing]` is only covered if a checkpoint exists at
    /// `vi=0` itself. We anchor by prepending `0` whenever `start_vi=0`,
    /// so the first event of every stroke saves the empty-scratch state
    /// at `vi=0` and all subsequent events can restore from it.
    ///
    /// Without this anchor, the first ~`spacing` events of every stroke
    /// fall back to full re-render (`restore_before` finds nothing for
    /// `div_idx ∈ [1..spacing]`), the ring clears on fallback, and the
    /// cycle repeats until `tip_vi` crosses `spacing`. Empirically, that
    /// produced ~15 catastrophic full re-renders per stroke at high
    /// stabilization.
    pub fn compute_segment_boundaries(
        start_vi: usize,
        tip_vi: usize,
        max_divergence_window: usize,
    ) -> Vec<usize> {
        let spacing = Self::spacing(max_divergence_window);
        let range = tip_vi.saturating_sub(start_vi);
        if range == 0 {
            return vec![];
        }

        let mut boundaries = Vec::new();
        // Coverage anchor: see invariant above.
        if start_vi == 0 {
            boundaries.push(0);
        }
        let mut pos = start_vi + spacing;
        while pos < tip_vi {
            boundaries.push(pos);
            pos += spacing;
        }
        // Always include the tip.
        boundaries.push(tip_vi);
        boundaries
    }
}
