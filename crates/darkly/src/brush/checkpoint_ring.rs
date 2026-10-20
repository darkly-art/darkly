//! Ring buffer of GPU texture checkpoints for partial stroke re-render.
//!
//! Each slot holds the stroke scratch (and the terminal's channels) as of
//! one save point, over a canvas-anchored frame. A save copies only the
//! region in which the scratch changed since the slot's textures were
//! last brought up to date, and a restore copies back only the region the
//! rewind undoes (the footprint of every dab after the checkpoint), so
//! the per-event cost follows the dabs between two indices rather than
//! the stroke's cumulative bbox. On divergence, the best checkpoint before
//! the divergence index is restored and only dabs after it are
//! re-rendered.
//!
//! The ring capacity is fixed (8 slots). Spacing between checkpoints
//! autoscales based on the stabilizer's max divergence window, so the
//! oldest checkpoint is typically just past the divergence boundary and
//! the remaining slots are densely packed in the volatile zone.

use super::save_points::SavePointStore;
use super::stroke_engine::RenderCheckpoint;
use crate::coord::CanvasRect;
use crate::gpu::atlas::CanvasFrame;

const RING_CAPACITY: usize = 8;

/// What a rewind undoes: the restore point's save-point index and the
/// canvas-space footprint of every dab after it, under the history being
/// discarded.
#[derive(Clone, Copy, Debug)]
pub struct Rewind {
    pub save_point_index: usize,
    pub region: CanvasRect,
}

/// The checkpoint [`CheckpointRing::find_before`] chose, and what restoring
/// it takes.
pub struct CheckpointRestore {
    /// The polyline vector index at the checkpoint; re-rendering resumes
    /// from the next one.
    pub vector_index: usize,
    /// Engine render state at the checkpoint.
    pub render_state: RenderCheckpoint,
    pub rewind: Rewind,
    /// The rewound region when the slot's frame does not cover all of it:
    /// the caller resets it to the terminal's baseline before
    /// [`CheckpointRing::restore`] copies the covered part back. `None`
    /// when the frame covers the region and no reset is needed.
    pub reset: Option<CanvasRect>,
    slot: usize,
}

/// What a slot's textures are known to hold.
struct SlotContent {
    /// The textures equal the scratch as of this save point, relative to
    /// the current dab list, everywhere in the frame outside `stale`.
    save_point_index: usize,
    /// Where they do not: the footprint of dabs a rewind discarded after
    /// this slot was written. Copied along with the dirtied range on the
    /// next save into the slot.
    stale: CanvasRect,
}

/// A single checkpoint slot in the ring buffer.
struct CheckpointSlot {
    /// Frame-sized GPU texture holding the stroke buffer snapshot. Lazily
    /// allocated; reallocated when the layer grows or the formats change.
    texture: Option<wgpu::Texture>,
    /// Format the slot was allocated in. The ring snapshots the stroke
    /// scratch, whose format is the terminal's business (color for most
    /// brushes, a float displacement field for warp terminals), and
    /// `copy_texture_to_texture` requires the two to match.
    tex_format: wgpu::TextureFormat,
    /// Snapshots of the stroke's channels, parallel to
    /// `Scratch::channel_textures`. Empty for terminals that declare none.
    ///
    /// Captured and restored with the stroke buffer because a channel is
    /// what dabs *read* to decide what to deposit. Rewinding the pixels
    /// but not the channel would leave it holding contributions from
    /// discarded dabs, and the dabs replayed over those pixels would read
    /// values describing a stroke that no longer exists.
    extra: Vec<wgpu::Texture>,
    /// The canvas rect the textures cover, the layer's extent when the
    /// slot was allocated: texel `(0, 0)` is the frame's origin. Canvas
    /// coordinates are stable across mid-stroke layer growth, so a frame
    /// never moves; a save under a grown extent reallocates. Sized to the
    /// layer rather than to the stroke's bbox so that a stroke never
    /// reallocates its slots mid-way (every slot would do so in the same
    /// event, each a fresh texture plus a frame-sized copy, which showed
    /// as a dropped frame), at the price of one layer-sized copy per slot
    /// the first time a stroke uses it.
    frame: CanvasRect,
    /// What the textures hold; `None` when nothing in them can be trusted
    /// (never written, reallocated, or left over from a previous stroke).
    content: Option<SlotContent>,
    /// Which save point this checkpoint was captured at.
    save_point_index: usize,
    /// The polyline vector index at that save point.
    vector_index: usize,
    /// Engine render state for resuming from this checkpoint. Captured at
    /// the save call site, not read from the save points: a segment that
    /// placed no dab finalizes no save point, so the store can lag behind
    /// the state the resume needs.
    render_state: RenderCheckpoint,
    /// Whether this slot contains valid data.
    valid: bool,
}

impl CheckpointSlot {
    fn empty() -> Self {
        Self {
            texture: None,
            tex_format: crate::brush::node::COLOR_SCRATCH_FORMAT,
            extra: Vec::new(),
            frame: CanvasRect::empty(),
            content: None,
            save_point_index: 0,
            vector_index: 0,
            render_state: RenderCheckpoint {
                last_point: None,
                accumulated_distance: 0.0,
                leftover_distance: 0.0,
                last_dab_size: [0.0, 0.0],
                last_dab_pos: None,
                dab_count: 0,
                stamp_angle: None,
            },
            valid: false,
        }
    }

    /// Make the slot's frame the layer's `extent`, in `format`, with one
    /// channel snapshot per entry in `extra_formats`. Reallocates, and
    /// returns `true`, when the extent differs from the frame (the layer
    /// grew), on a format change (a slot cached from a color stroke cannot
    /// receive a warp field), or when the slot holds nothing yet. The
    /// whole set is reallocated together so a slot's snapshots always
    /// share a frame. A reallocated slot's content is unknown.
    fn ensure_frame(
        &mut self,
        device: &wgpu::Device,
        extent: CanvasRect,
        format: wgpu::TextureFormat,
        extra_formats: &[wgpu::TextureFormat],
    ) -> bool {
        // Slots outlive strokes (`clear()` only flips `valid`), so a slot
        // allocated for one terminal is reused by the next. Comparing the
        // formats, not just the count, is what stops a `paint` stroke's
        // empty slot (or a differently-typed channel set) being reused as
        // though it held this terminal's snapshots.
        let formats_match = self.extra.len() == extra_formats.len()
            && self
                .extra
                .iter()
                .zip(extra_formats)
                .all(|(t, f)| t.format() == *f);
        if self.texture.is_some()
            && self.tex_format == format
            && formats_match
            && self.frame == extent
        {
            return false;
        }
        self.content = None;
        self.tex_format = format;
        self.frame = extent;
        if self.frame.is_empty() {
            self.texture = None;
            self.extra.clear();
            return true;
        }
        let make = |format: wgpu::TextureFormat| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("checkpoint-slot"),
                size: wgpu::Extent3d {
                    width: self.frame.width,
                    height: self.frame.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        self.texture = Some(make(format));
        self.extra = extra_formats.iter().copied().map(make).collect();
        true
    }

    /// The region a save at `save_point_index` copies into this slot to
    /// make its textures exact over the whole frame: where they are stale,
    /// plus where the scratch changed since the save point they hold.
    ///
    /// The content's index is never above the one being saved: every
    /// valid slot sits at or below a rewind's restore point, every save
    /// during the replay is above it, and a rewritten content carries the
    /// restore point's own index.
    fn copy_region(&self, save_points: &SavePointStore, save_point_index: usize) -> CanvasRect {
        match &self.content {
            Some(c) if c.save_point_index <= save_point_index => c
                .stale
                .union(save_points.dirty_between(c.save_point_index, save_point_index))
                .intersect(self.frame)
                .unwrap_or(CanvasRect::empty()),
            _ => {
                debug_assert!(
                    self.content.is_none(),
                    "checkpoint slot content is ahead of the save point being written"
                );
                self.frame
            }
        }
    }

    fn frame<'a>(&'a self, texture: &'a wgpu::Texture) -> CanvasFrame<'a> {
        CanvasFrame {
            texture,
            canvas_extent: self.frame,
        }
    }
}

/// Ring buffer of checkpoint textures for O(divergence_window / N) re-render.
///
/// Two invariants (one for correctness, one for performance) together
/// make full-stroke re-render fallback impossible by construction whenever
/// the stabilizer's `max_divergence_window` bound holds.
///
/// 1. **Coverage (correctness).** After every save, there exists a valid
///    slot with `vi ≤ tip_vi − max_divergence_window`. That single slot
///    guarantees `find_before(div_idx)` finds something for every
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
/// A fourth, on what the textures hold, makes the region copies sound:
///
/// 4. **Content.** A slot with `content = Some(c)` equals the scratch as
///    of save point `c.save_point_index` (relative to the current dab
///    list) everywhere in its frame outside `c.stale`; a valid slot has
///    `c.save_point_index == save_point_index` and an empty `c.stale`.
///    A save copies `c.stale` plus the range dirtied since `c`; a rewind
///    that discards dabs after a slot's index rewrites `c` to the restore
///    point and adds the discarded footprint to `c.stale`; a reallocation
///    or `clear()` sets `content = None`.
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
    ///    it is the sole anchor, i.e., while no other slot satisfies
    ///    `vi ≤ tip_vi − max_div_window`.
    ///
    /// Naively evicting the lowest `vi` slot (the prior policy) destroys
    /// the anchor as soon as the ring fills, leaving the bottom of the
    /// divergence window uncovered and forcing a full re-render fallback.
    /// Protecting the sole anchor and otherwise compressing the densest
    /// cluster keeps both invariants satisfiable for as long as the
    /// spacing and ring capacity admit.
    ///
    /// Cost is O(n²) on the ring size (n is 8), which is negligible
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
        // `find_before(div_idx)` returns the slot with the largest
        // `vi < div_idx`. The worst-case reachable `div_idx` is
        // `anchor_boundary`, so coverage requires `vi < anchor_boundary`.
        // The anchor is "redundant" (and the lowest slot may be evicted)
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
        // anchor; the post-save assertion will surface any real coverage
        // loss in debug builds.
        best.map(|(i, _)| i).unwrap_or(anchor_slot)
    }

    /// Save a checkpoint at `save_point_index` into a ring slot chosen by
    /// [`pick_slot`], copying only what the slot's textures lack
    /// ([`CheckpointSlot::copy_region`]). `stroke` is the stroke buffer
    /// paired with the active layer's canvas extent (the stroke buffer is
    /// texture-aligned to the layer texture); `extra` are its channels.
    /// `tip_vi` and `max_div_window` are the stabilizer's current tip
    /// index and bound (used by the eviction policy and the post-save
    /// coverage assertion).
    ///
    /// A checkpoint at which nothing has been painted yet is a *valid*
    /// checkpoint: it records exactly that, and a rewind to it restores
    /// the baseline. Claiming the slot keeps the `vi = 0` anchor present
    /// when a stroke's first dab is an identity write (a stationary
    /// smudge, say); without it every early divergence falls back to a
    /// full re-render.
    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        stroke: &CanvasFrame<'_>,
        extra: &[&wgpu::Texture],
        save_point_index: usize,
        vector_index: usize,
        save_points: &SavePointStore,
        render_state: RenderCheckpoint,
        tip_vi: usize,
        max_div_window: usize,
    ) {
        let extra_formats: Vec<wgpu::TextureFormat> = extra.iter().map(|t| t.format()).collect();
        let slot_idx = self.pick_slot(tip_vi, max_div_window, vector_index);
        let slot = &mut self.slots[slot_idx];
        let reallocated = slot.ensure_frame(
            device,
            stroke.canvas_extent,
            stroke.texture.format(),
            &extra_formats,
        );
        let region = if reallocated {
            slot.frame
        } else {
            slot.copy_region(save_points, save_point_index)
        };

        if let Some(texture) = slot.texture.as_ref() {
            stroke.copy_rect_to(encoder, &slot.frame(texture), region);
            // Same region, same coordinates: the channels are layer-sized
            // and grown in lockstep with the stroke buffer, so one rect
            // addresses all of them.
            for (src, dst) in extra.iter().zip(&slot.extra) {
                CanvasFrame {
                    texture: src,
                    canvas_extent: stroke.canvas_extent,
                }
                .copy_rect_to(encoder, &slot.frame(dst), region);
            }
        }

        slot.content = Some(SlotContent {
            save_point_index,
            stale: CanvasRect::empty(),
        });
        slot.save_point_index = save_point_index;
        slot.vector_index = vector_index;
        slot.render_state = render_state;
        slot.valid = true;

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
    /// Strict inequality because `find_before(div_idx)` returns the slot
    /// with the largest `vi < div_idx`, and the worst-case `div_idx` is
    /// `tip_vi − max_div`. At stroke start (when `tip_vi ≤ max_div`), the
    /// reachable divergence window includes `vi = 0` and no anchor below it
    /// can exist; full re-render from `vi = 0` is bounded and intended.
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

    /// Find the best checkpoint strictly before `div_vector_index` and
    /// work out what restoring it takes: the region a rewind to it undoes
    /// is the footprint of every dab after it, read from `save_points`
    /// *before* the caller truncates them.
    ///
    /// Returns `None` when no valid slot precedes the index (the caller
    /// then re-renders the whole stroke).
    pub fn find_before(
        &self,
        div_vector_index: usize,
        save_points: &SavePointStore,
    ) -> Option<CheckpointRestore> {
        let slot_idx = self.best_slot_before(div_vector_index)?;
        let slot = &self.slots[slot_idx];
        let region = save_points.dirty_after(slot.save_point_index);
        Some(CheckpointRestore {
            vector_index: slot.vector_index,
            render_state: slot.render_state.clone(),
            rewind: Rewind {
                save_point_index: slot.save_point_index,
                region,
            },
            reset: (!slot.frame.contains(region)).then_some(region),
            slot: slot_idx,
        })
    }

    /// Copy the rewound region back from the checkpoint `find_before`
    /// chose, for the stroke buffer and each channel. Outside the region
    /// the scratch already equals the checkpoint; inside it, the part the
    /// slot's frame covers is exact by the content invariant and the rest
    /// (`CheckpointRestore::reset`) is the terminal's baseline, which the
    /// caller must have established first (`StrokeEngine::begin_stroke`
    /// over that rect: a transparent clear for paint, a copy of the
    /// pre-stroke layer for a warp or smudge terminal; the ring does not
    /// care which).
    ///
    /// `stroke` pairs the stroke buffer with the active layer's *current*
    /// canvas extent, which may be larger than at save time if the layer
    /// has grown in the meantime; the stroke buffer's contents are rebased
    /// by `StrokeBuffer::grow_preserving` to track the new frame, and the
    /// slot's frame is in canvas coordinates, so the copy lands where it
    /// should, and `reset` covers the grown part. Must run before any
    /// save changes the ring.
    pub fn restore(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        stroke: &CanvasFrame<'_>,
        extra: &[&wgpu::Texture],
        cp: &CheckpointRestore,
    ) {
        let slot = &self.slots[cp.slot];
        debug_assert!(
            slot.valid && slot.save_point_index == cp.rewind.save_point_index,
            "checkpoint slot changed between find_before and restore"
        );
        let Some(texture) = slot.texture.as_ref() else {
            return;
        };
        slot.frame(texture)
            .copy_rect_to(encoder, stroke, cp.rewind.region);
        // Restore the accumulators the stroke buffer's pixels were derived
        // from, or the next resolve recomputes this region from state
        // describing dabs that were just discarded.
        for (dst, src) in extra.iter().zip(&slot.extra) {
            slot.frame(src).copy_rect_to(
                encoder,
                &CanvasFrame {
                    texture: dst,
                    canvas_extent: stroke.canvas_extent,
                },
                cp.rewind.region,
            );
        }
    }

    /// Invalidate all checkpoints with vector_index >= threshold, and
    /// record on every invalid slot written after the rewind's restore
    /// point that it now equals that point everywhere outside the rewound
    /// region (content invariant 4): its dabs after the restore point were
    /// discarded, and all of them lie inside `rewind.region`.
    pub fn invalidate_from(&mut self, vector_index: usize, rewind: Rewind) {
        for slot in &mut self.slots {
            if slot.valid && slot.vector_index >= vector_index {
                slot.valid = false;
            }
            if slot.valid {
                continue;
            }
            if let Some(c) = slot.content.as_mut() {
                if c.save_point_index > rewind.save_point_index {
                    c.save_point_index = rewind.save_point_index;
                    c.stale = c.stale.union(rewind.region);
                }
            }
        }
    }

    /// Invalidate all checkpoints. The textures survive for reuse but
    /// nothing in them is trusted: the next save into a slot copies its
    /// whole frame.
    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            slot.valid = false;
            slot.content = None;
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
    /// with `vi < div_idx` for every reachable divergence index; that's
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
    /// fall back to full re-render (`find_before` finds nothing for
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
        let mut boundaries = Vec::new();
        // Coverage anchor: see invariant above. Pushed even when the range
        // holds only `vi = 0`: a stroke's first flush can render a single
        // vertex, and the anchor is what its later rewinds restore to.
        if start_vi == 0 {
            boundaries.push(0);
        }
        if tip_vi <= start_vi {
            return boundaries;
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

    fn cp() -> RenderCheckpoint {
        RenderCheckpoint {
            last_point: None,
            accumulated_distance: 0.0,
            leftover_distance: 0.0,
            last_dab_size: [0.0, 0.0],
            last_dab_pos: None,
            dab_count: 0,
            stamp_angle: None,
        }
    }

    fn r(x: i32, y: i32, w: u32, h: u32) -> CanvasRect {
        CanvasRect::from_xywh(x, y, w, h)
    }

    /// Put `slot` at save point `sp` (vector index `sp` too) with exact
    /// content over `frame`, as a save leaves it.
    fn seed_content(ring: &mut CheckpointRing, slot: usize, sp: usize, frame: CanvasRect) {
        let s = &mut ring.slots[slot];
        s.frame = frame;
        s.content = Some(SlotContent {
            save_point_index: sp,
            stale: CanvasRect::empty(),
        });
        s.save_point_index = sp;
        s.vector_index = sp;
        s.valid = true;
    }

    fn content_of(ring: &CheckpointRing, slot: usize) -> Option<(usize, CanvasRect)> {
        ring.slots[slot]
            .content
            .as_ref()
            .map(|c| (c.save_point_index, c.stale))
    }

    /// Content invariant 4 through two rewinds: what a save copies is the
    /// slot's stale rect plus the range dirtied since its content index,
    /// clipped to the frame; a slot with unknown content copies its whole
    /// frame.
    #[test]
    fn copy_region_tracks_stale_and_dirty_through_rewinds() {
        // Dab `i` at x = 10 i, one per vector index.
        let mut store = SavePointStore::new();
        for i in 0..6 {
            store.push(r(i as i32 * 10, 0, 10, 10), i, cp());
        }
        let frame = r(0, 0, 256, 256);
        let mut ring = CheckpointRing::new();
        seed_content(&mut ring, 0, 1, frame);
        seed_content(&mut ring, 2, 4, frame);
        ring.slots[1].frame = frame; // content unknown

        // A save at 4 into the slot at 1 copies dabs 2..=4 only.
        assert_eq!(ring.slots[0].copy_region(&store, 4), r(20, 0, 30, 10));
        assert_eq!(ring.slots[1].copy_region(&store, 4), frame);

        // Rewind to save point 2 from tip 5 with divergence at 3: the slot
        // at 4 is invalidated and now equals point 2 outside dabs 3..=5.
        let region = store.dirty_after(2);
        assert_eq!(region, r(30, 0, 30, 10));
        ring.invalidate_from(
            3,
            Rewind {
                save_point_index: 2,
                region,
            },
        );
        assert!(!ring.slots[2].valid);
        assert_eq!(content_of(&ring, 2), Some((2, region)));
        assert!(
            ring.slots[0].valid,
            "a slot below the divergence keeps its content"
        );
        assert_eq!(content_of(&ring, 0), Some((1, CanvasRect::empty())));

        // The replay rewrites dabs 3 and 4 elsewhere.
        store.truncate(3);
        store.push(r(100, 0, 10, 10), 3, cp());
        store.push(r(110, 0, 10, 10), 4, cp());
        // A save at 4 into the invalidated slot copies stale plus (2, 4].
        assert_eq!(ring.slots[2].copy_region(&store, 4), r(30, 0, 90, 10));
        // Into the valid slot at 1: (1, 4] only.
        assert_eq!(ring.slots[0].copy_region(&store, 4), r(20, 0, 100, 10));

        // A second rewind to an earlier point (0) widens the already-stale
        // slot and catches the slot at 1 as well.
        let region2 = store.dirty_after(0);
        assert_eq!(region2, r(10, 0, 110, 10));
        ring.invalidate_from(
            1,
            Rewind {
                save_point_index: 0,
                region: region2,
            },
        );
        assert_eq!(content_of(&ring, 2), Some((0, region.union(region2))));
        assert_eq!(content_of(&ring, 0), Some((0, region2)));
        assert_eq!(content_of(&ring, 1), None);

        // The copy is clipped to the frame.
        ring.slots[3].frame = r(0, 0, 64, 64);
        ring.slots[3].content = Some(SlotContent {
            save_point_index: 0,
            stale: CanvasRect::empty(),
        });
        assert_eq!(ring.slots[3].copy_region(&store, 4), r(10, 0, 54, 10));

        // `clear()` forgets everything: every slot copies its whole frame.
        ring.clear();
        for i in 0..4 {
            assert_eq!(content_of(&ring, i), None);
            assert_eq!(ring.slots[i].copy_region(&store, 4), ring.slots[i].frame);
        }
    }

    /// On a device: a slot invalidated by a rewind still holds the texels
    /// of the discarded dabs. The next save into it must copy its stale
    /// rect along with the newly dirtied range, or a later restore from
    /// it resurrects a dab that no longer exists.
    #[test]
    fn resave_of_an_invalidated_slot_overwrites_discarded_dabs() {
        use crate::gpu::test_utils::{readback_texture, test_device};
        let (device, queue) = test_device();
        let extent = r(0, 0, 128, 128);
        let scratch = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scratch"),
            size: wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let frame = CanvasFrame {
            texture: &scratch,
            canvas_extent: extent,
        };
        // A "dab": a solid rect written straight into the scratch.
        let paint = |rect: CanvasRect, value: u8| {
            let bytes = vec![value; (rect.width * rect.height * 4) as usize];
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &scratch,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: rect.x0() as u32,
                        y: rect.y0() as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(rect.width * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: rect.width,
                    height: rect.height,
                    depth_or_array_layers: 1,
                },
            );
        };
        let mut store = SavePointStore::new();
        let mut ring = CheckpointRing::new();
        let save = |ring: &mut CheckpointRing, store: &SavePointStore, index: usize| {
            let mut encoder = device.create_command_encoder(&Default::default());
            ring.save(
                &device,
                &mut encoder,
                &frame,
                &[],
                index,
                index,
                store,
                cp(),
                index,
                0,
            );
            queue.submit([encoder.finish()]);
        };

        let (a, b, c, d) = (
            r(10, 10, 8, 8),
            r(60, 60, 8, 8),
            r(100, 100, 8, 8),
            r(56, 56, 16, 16),
        );
        paint(a, 0xAA);
        store.push(a, 0, cp());
        save(&mut ring, &store, 0);
        paint(b, 0xBB);
        store.push(b, 1, cp());
        save(&mut ring, &store, 1);

        // Rewind to index 0: dab 1 is discarded. The frame covers the
        // region, so no reset; the restore writes the slot's zeros over B.
        let found = ring.find_before(1, &store).unwrap();
        assert_eq!(found.rewind.region, b);
        assert_eq!(found.reset, None);
        let mut encoder = device.create_command_encoder(&Default::default());
        ring.restore(&mut encoder, &frame, &[], &found);
        queue.submit([encoder.finish()]);
        store.truncate(1);
        ring.invalidate_from(1, found.rewind);

        // The replay puts dab 1 at C instead, and the save at index 1 lands
        // in the invalidated slot, which still holds dab 1 at B.
        paint(c, 0xCC);
        store.push(c, 1, cp());
        save(&mut ring, &store, 1);
        // Dab 2 covers B, so a rewind to index 1 restores B from that slot.
        paint(d, 0xDD);
        store.push(d, 2, cp());
        let found = ring.find_before(2, &store).unwrap();
        assert_eq!(found.rewind.region, d);
        let mut encoder = device.create_command_encoder(&Default::default());
        ring.restore(&mut encoder, &frame, &[], &found);
        queue.submit([encoder.finish()]);

        let out = readback_texture(
            &device,
            &queue,
            &scratch,
            wgpu::TextureFormat::Rgba8Unorm,
            extent.width,
            extent.height,
        );
        let px = |x: i32, y: i32| out[((y as u32 * extent.width + x as u32) * 4) as usize];
        assert_eq!(px(12, 12), 0xAA, "dab 0 is untouched");
        assert_eq!(px(102, 102), 0xCC, "the replayed dab 1 is untouched");
        assert_eq!(px(58, 58), 0, "dab 2 is undone");
        assert_eq!(
            px(62, 62),
            0,
            "the discarded dab 1 must not come back through the re-saved slot"
        );
    }

    /// `find_before` reports the dabs after the checkpoint as the region
    /// to restore, and asks for a reset only when the slot's frame does
    /// not cover it.
    #[test]
    fn find_before_reports_region_and_reset() {
        let mut store = SavePointStore::new();
        for i in 0..4 {
            store.push(r(i as i32 * 100, 0, 10, 10), i, cp());
        }
        let mut ring = CheckpointRing::new();
        seed_content(&mut ring, 0, 1, r(0, 0, 256, 256));
        let found = ring.find_before(2, &store).expect("slot at 1 precedes 2");
        assert_eq!(found.vector_index, 1);
        assert_eq!(found.rewind.save_point_index, 1);
        assert_eq!(found.rewind.region, r(200, 0, 110, 10));
        assert_eq!(found.reset, Some(r(200, 0, 110, 10)));

        ring.slots[0].frame = r(0, 0, 512, 256);
        let found = ring.find_before(2, &store).unwrap();
        assert_eq!(found.reset, None, "the frame covers the rewound region");

        assert!(ring.find_before(1, &store).is_none());
    }

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
            ring.invalidate_from(
                div_idx,
                Rewind {
                    save_point_index: 0,
                    region: CanvasRect::empty(),
                },
            );
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
            // find a slot strictly before it (no `find_before` returning
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
