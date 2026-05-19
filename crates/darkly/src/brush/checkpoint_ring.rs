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
                last_dab_pos: None,
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
/// Two invariants — one for correctness, one for performance — together
/// make full-stroke re-render fallback impossible by construction whenever
/// the stabilizer's `max_divergence_window` bound holds.
///
/// 1. **Coverage (correctness).** After every save, there exists a valid
///    slot with `vi ≤ tip_vi − max_divergence_window`. That single slot
///    guarantees `restore_before(div_idx)` finds something for every
///    reachable `div_idx ∈ [tip_vi − max_div, tip_vi]`.
///
/// 2. **Density (performance).** Consecutive valid slot gaps (sorted by
///    `vi`) are `≤ spacing = max_div / 7`. This bounds per-event re-render
///    cost at ~`spacing` dabs.
///
/// 3. **Scoped invalidation.** `invalidate_from(div_idx)`, not
///    `invalidate_from(restore_point + 1)`. Checkpoints between the restore
///    point and the divergence index are still valid (the stroke buffer
///    content there didn't change). Preserving them lets the restore point
///    advance toward the tip on subsequent frames.
///
/// The eviction policy in [`pick_slot`] protects the sole anchor while it
/// is the only slot satisfying the coverage invariant, then picks the
/// non-anchor slot whose removal leaves the smallest worst-case gap.
/// `save()` ends with a `debug_assert!` that the coverage invariant holds.
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

    /// Choose which slot to overwrite for a new checkpoint at `new_vi`.
    ///
    /// Anchor-protected min-gap eviction:
    ///
    /// 1. Prefer any invalid slot.
    /// 2. Otherwise, simulate inserting `new_vi` among the current valid vi
    ///    values. For each existing slot, compute the resulting max
    ///    consecutive gap if it were evicted; pick the slot that minimizes
    ///    that max gap. The slot with the lowest `vi` is *protected* while
    ///    it is the sole anchor — i.e., while no other slot satisfies
    ///    `vi ≤ tip_vi − max_div_window`.
    ///
    /// Naively evicting the lowest `vi` slot (the prior policy) destroys
    /// the anchor as soon as the ring fills, leaving the bottom of the
    /// divergence window uncovered and forcing a full re-render fallback.
    /// Protecting the sole anchor and otherwise compressing the densest
    /// cluster keeps both invariants satisfiable for as long as the
    /// spacing and ring capacity admit.
    ///
    /// Cost is O(n²) on the ring size — n is 8 — which is negligible
    /// compared with the GPU work each save triggers.
    fn pick_slot(&self, tip_vi: usize, max_div_window: usize, new_vi: usize) -> usize {
        // 1) any invalid slot wins immediately.
        if let Some(i) = self.slots.iter().position(|s| !s.valid) {
            return i;
        }

        let n = self.slots.len();
        let mut by_vi: Vec<(usize, usize)> =
            (0..n).map(|i| (i, self.slots[i].vector_index)).collect();
        by_vi.sort_by_key(|&(_, v)| v);

        let anchor_boundary = tip_vi.saturating_sub(max_div_window);
        // `restore_before(div_idx)` returns the slot with the largest
        // `vi < div_idx`. The worst-case reachable `div_idx` is
        // `anchor_boundary`, so coverage requires `vi < anchor_boundary`.
        // The anchor is "redundant" — and the lowest slot may be evicted —
        // only when the second-lowest slot already satisfies that strict
        // inequality. While `by_vi[1].vi >= anchor_boundary`, the anchor is
        // the sole carrier of coverage and must be protected.
        let anchor_protected = by_vi.len() < 2 || by_vi[1].1 >= anchor_boundary;
        let anchor_slot = by_vi[0].0;

        // Sort the candidate set including `new_vi` so we can compute max
        // gaps after each hypothetical eviction.
        let mut all: Vec<(usize, usize)> = by_vi.clone();
        let new_pos = all.partition_point(|&(_, v)| v <= new_vi);
        // Sentinel slot index: never evict the slot we're about to write.
        all.insert(new_pos, (usize::MAX, new_vi));

        let mut best: Option<(usize, usize)> = None; // (slot_idx, resulting_max_gap)
        for (k, &(cand, _)) in all.iter().enumerate() {
            if cand == usize::MAX {
                continue;
            }
            if anchor_protected && cand == anchor_slot {
                continue;
            }
            // Compute the max consecutive gap with `all[k]` removed.
            let mut max_gap = 0usize;
            let mut prev_v: Option<usize> = None;
            for (j, &(_, v)) in all.iter().enumerate() {
                if j == k {
                    continue;
                }
                if let Some(p) = prev_v {
                    max_gap = max_gap.max(v.saturating_sub(p));
                }
                prev_v = Some(v);
            }
            if best.is_none_or(|(_, g)| max_gap < g) {
                best = Some((cand, max_gap));
            }
        }

        // If anchor protection rejected every candidate (n=1 only), or some
        // future state we haven't anticipated, fall back to evicting the
        // anchor — the post-save assertion will surface any real coverage
        // loss in debug builds.
        best.map(|(i, _)| i).unwrap_or(anchor_slot)
    }

    /// Save a checkpoint: copy the bbox region from the stroke texture into
    /// a ring slot chosen by [`pick_slot`]. `stroke` is the stroke buffer
    /// paired with the active layer's canvas extent (the stroke buffer is
    /// texture-aligned to the layer texture). `canvas_bbox` is the
    /// canvas-space rect to snapshot. `tip_vi` and `max_div_window` are the
    /// stabilizer's current tip index and bound — used by the eviction
    /// policy and the post-save coverage assertion.
    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        stroke: &CanvasFrame<'_>,
        save_point_index: usize,
        vector_index: usize,
        canvas_bbox: CanvasRect,
        render_state: RenderCheckpoint,
        tip_vi: usize,
        max_div_window: usize,
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

        let slot_idx = self.pick_slot(tip_vi, max_div_window, vector_index);
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

        // Coverage invariant: after every save, at least one valid slot
        // must sit at or below the divergence boundary. If this fires, the
        // eviction policy lost the anchor or the stabilizer's bound was
        // violated upstream.
        debug_assert!(
            self.has_anchor(tip_vi, max_div_window),
            "checkpoint ring lost anchor coverage: tip={tip_vi}, max_div={max_div_window}, \
             slots={:?}",
            self.slots
                .iter()
                .filter(|s| s.valid)
                .map(|s| s.vector_index)
                .collect::<Vec<_>>()
        );
    }

    /// Whether the ring has at least one valid slot. Used by the engine to
    /// distinguish "expected initialization fallback" (empty ring at stroke
    /// start) from "coverage defect fallback" (populated ring failed to
    /// cover a reachable divergence index).
    pub fn has_any_valid(&self) -> bool {
        self.slots.iter().any(|s| s.valid)
    }

    /// Whether the ring satisfies the coverage invariant for the given
    /// stabilizer state: a valid slot exists with `vi < tip_vi − max_div`.
    ///
    /// Strict inequality because `restore_before(div_idx)` returns the slot
    /// with the largest `vi < div_idx`, and the worst-case `div_idx` is
    /// `tip_vi − max_div`. At stroke start (when `tip_vi ≤ max_div`), the
    /// reachable divergence window includes `vi = 0` and no anchor below it
    /// can exist — full re-render from `vi = 0` is bounded and intended.
    pub fn has_anchor(&self, tip_vi: usize, max_div_window: usize) -> bool {
        if tip_vi <= max_div_window {
            return true;
        }
        let boundary = tip_vi - max_div_window;
        self.slots
            .iter()
            .any(|s| s.valid && s.vector_index < boundary)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed the ring's valid slots with the given `vi` values. Test-only;
    /// `pick_slot` and `has_anchor` only read `vector_index` and `valid`, so
    /// the other slot fields can stay at their defaults.
    fn seed(ring: &mut CheckpointRing, vis: &[usize]) {
        assert!(
            vis.len() <= ring.slots.len(),
            "more values than slots in the ring"
        );
        for slot in &mut ring.slots {
            slot.valid = false;
        }
        for (slot, &vi) in ring.slots.iter_mut().zip(vis.iter()) {
            slot.vector_index = vi;
            slot.valid = true;
        }
    }

    fn vis_sorted(ring: &CheckpointRing) -> Vec<usize> {
        let mut v: Vec<usize> = ring
            .slots
            .iter()
            .filter(|s| s.valid)
            .map(|s| s.vector_index)
            .collect();
        v.sort();
        v
    }

    /// `pick_slot` should grab any invalid slot first regardless of vi
    /// layout. Sanity check.
    #[test]
    fn pick_slot_invalid_first() {
        let mut ring = CheckpointRing::new();
        seed(&mut ring, &[0, 9, 18]); // 5 invalid slots remain
        let picked = ring.pick_slot(/*tip*/ 30, /*max_div*/ 20, /*new_vi*/ 25);
        assert!(!ring.slots[picked].valid, "should pick an invalid slot");
    }

    /// Regression for defect 2. With `{0, 9, 18, …, 63}` filling all 8
    /// slots and `tip=72, max_div=65`, the anchor boundary is 7. The
    /// only slot with `vi ≤ 7` is `vi=0`; evicting it would lose
    /// coverage. The old `min_by_key(vector_index)` policy did exactly
    /// that; the new policy must protect the anchor.
    #[test]
    fn pick_slot_preserves_sole_anchor() {
        let mut ring = CheckpointRing::new();
        seed(&mut ring, &[0, 9, 18, 27, 36, 45, 54, 63]);
        let tip = 72;
        let max_div = 65;
        let new_vi = 72;
        let picked = ring.pick_slot(tip, max_div, new_vi);
        assert_ne!(
            ring.slots[picked].vector_index,
            0,
            "must not evict the sole anchor at vi=0 \
             (slots={:?}, tip={tip}, max_div={max_div})",
            vis_sorted(&ring)
        );
    }

    /// Once a non-anchor slot has crossed below the divergence boundary,
    /// the original anchor becomes redundant and is allowed to be evicted.
    /// `{9,18,…,72,81}`, `tip=90, max_div=65`: boundary=25, slot[1]=18≤25,
    /// anchor releasable.
    #[test]
    fn pick_slot_releases_redundant_anchor() {
        let mut ring = CheckpointRing::new();
        seed(&mut ring, &[9, 18, 27, 36, 45, 54, 63, 72]);
        let tip = 90;
        let max_div = 65;
        let new_vi = 90;
        let picked = ring.pick_slot(tip, max_div, new_vi);
        // The lowest slot is now a candidate. We don't pin which slot wins
        // (any eviction that keeps coverage is acceptable), but the
        // resulting ring must still have an anchor.
        let evicted_vi = ring.slots[picked].vector_index;
        // Simulate the save: replace evicted with new_vi.
        ring.slots[picked].vector_index = new_vi;
        assert!(
            ring.has_anchor(tip, max_div),
            "anchor invariant lost after evicting vi={evicted_vi}, slots={:?}",
            vis_sorted(&ring)
        );
    }

    /// Long simulation: walk the tip forward, save at every spacing step,
    /// and assert the coverage invariant holds after every save. This
    /// catches both the original "evict-lowest" failure mode and any
    /// future eviction regressions.
    #[test]
    fn coverage_invariant_holds_over_long_run() {
        let mut ring = CheckpointRing::new();
        let max_div = 65;
        let spacing = CheckpointRing::spacing(max_div); // 9
        for step in 0..1000 {
            let new_vi = step * spacing;
            let tip = new_vi;
            let picked = ring.pick_slot(tip, max_div, new_vi);
            ring.slots[picked].vector_index = new_vi;
            ring.slots[picked].valid = true;
            assert!(
                ring.has_anchor(tip, max_div),
                "anchor invariant lost at step={step}, tip={tip}, slots={:?}",
                vis_sorted(&ring)
            );
        }
    }

    /// Edge cases: tiny max_div windows.
    #[test]
    fn coverage_invariant_holds_with_small_window() {
        for &max_div in &[0usize, 1, 2, 3, 5] {
            let mut ring = CheckpointRing::new();
            let spacing = CheckpointRing::spacing(max_div).max(1);
            for step in 0..200 {
                let new_vi = step * spacing;
                let tip = new_vi;
                let picked = ring.pick_slot(tip, max_div, new_vi);
                ring.slots[picked].vector_index = new_vi;
                ring.slots[picked].valid = true;
                assert!(
                    ring.has_anchor(tip, max_div),
                    "anchor invariant lost at max_div={max_div}, step={step}, tip={tip}, \
                     slots={:?}",
                    vis_sorted(&ring)
                );
            }
        }
    }

    /// Realistic save pattern: divergence at random points within the
    /// window triggers a restore + segmented re-render. Each segment
    /// boundary is a save. The ring must keep coverage across the
    /// restore + re-save cycle.
    #[test]
    fn coverage_invariant_holds_with_segment_boundary_pattern() {
        let mut ring = CheckpointRing::new();
        let max_div = 65usize;
        let _spacing = CheckpointRing::spacing(max_div);

        for step in 1usize..400 {
            let tip = step * 3; // grow tip steadily
                                // Divergence: rewind to some recent index inside the window.
            let div_idx = tip.saturating_sub(max_div / 2);
            // Find restore checkpoint: best slot with vi < div_idx.
            let start_vi = ring
                .slots
                .iter()
                .filter(|s| s.valid && s.vector_index < div_idx)
                .map(|s| s.vector_index)
                .max()
                .map(|v| v + 1)
                .unwrap_or(0);
            // Invalidate slots at or after div_idx (mirrors painting.rs).
            ring.invalidate_from(div_idx);
            // Replay segment boundaries.
            let boundaries = CheckpointRing::compute_segment_boundaries(start_vi, tip, max_div);
            let mut seg_start = start_vi;
            for &boundary in &boundaries {
                if boundary < seg_start || boundary > tip {
                    continue;
                }
                let picked = ring.pick_slot(tip, max_div, boundary);
                ring.slots[picked].vector_index = boundary;
                ring.slots[picked].valid = true;
                seg_start = boundary + 1;
            }
            assert!(
                ring.has_anchor(tip, max_div),
                "anchor invariant lost at step={step}, tip={tip}, div_idx={div_idx}, \
                 start_vi={start_vi}, slots={:?}",
                vis_sorted(&ring)
            );
            // Density: every reachable div_idx in [tip-max_div, tip] should
            // find a slot strictly before it (no `restore_before` returning
            // None within the window).
            for d in tip.saturating_sub(max_div)..=tip {
                let has = ring.slots.iter().any(|s| s.valid && s.vector_index < d);
                if d > 0 {
                    assert!(
                        has,
                        "no slot with vi < {d} after step={step}, tip={tip}, slots={:?}",
                        vis_sorted(&ring)
                    );
                }
            }
        }
    }
}
