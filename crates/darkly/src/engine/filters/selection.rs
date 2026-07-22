//! Engine-level selection ops (shape fills, booleans, active toggle, undo).
//!
//! The global selection is a typed [`crate::document::Filter`] attached at
//! the document root. Selection state splits cleanly across:
//!
//! - **Document model**: `doc.selection: Option<Filter>` carries name,
//!   visibility (active toggle), lock, [`crate::layer::PixelBuffer`] bounds,
//!   tight pixel bounds, and the CPU readback cache via [`SelectionFilter`].
//! - **Compositor**: `compositor.selection_state` carries the ping-pong R8
//!   textures, the shared selection-mask bind group, and the filter id used
//!   for region-store / undo keying.
//! - **Engine**: this file. The high-level ops the user invokes (select_rect,
//!   apply_selection_mask, invert, clear, magic wand, …) plus the bridge
//!   helpers consumers reach for (`selection_active`, `selection_cpu_cache`,
//!   `selection_pixel_bounds`, …).

use darkly_macros::handlers;

use super::super::rendering::commit_undo_region;
use super::super::{DarklyEngine, OverlayChannel, ReadbackContext};
use crate::coord::{CanvasRect, WindowRect};
use crate::document::SelectionMode;
use crate::gpu::flood_fill::{self, LayerFloodFillExtent};
use crate::gpu::overlay::{OverlayPrimitive, FLAG_CANVAS_SPACE, KIND_DASHED_LINE};
use crate::gpu::readback;
use crate::gpu::selection::{CombineMode, MorphOp};
use crate::layer::LayerId;
use crate::mask::RasterizedMask;
use crate::undo::SelectionAction;

/// Concatenate every overlay channel bottom-to-top in [`OverlayChannel::ALL`]
/// order (`Selection`, `CloneSource`, `Tool`). Pure — the z-order is the one
/// behaviour worth pinning down in a test, decoupled from the GPU push in
/// [`DarklyEngine::push_merged_overlay`].
pub(crate) fn merge_overlay_channels(
    channels: &[Vec<OverlayPrimitive>; OverlayChannel::COUNT],
) -> Vec<OverlayPrimitive> {
    let total: usize = channels.iter().map(|c| c.len()).sum();
    let mut merged = Vec::with_capacity(total);
    for channel in OverlayChannel::ALL {
        merged.extend_from_slice(&channels[channel.index()]);
    }
    merged
}

/// Emit marching-ants overlay primitives from contour polylines. Each edge
/// produces a pair of dashed-line primitives — black backing + white dashes —
/// carrying cumulative arc length in `dash_offset` so dash phase flows
/// continuously around the contour. Without that, each edge resets phase at
/// its start: invisible at high zoom, glitchy flicker when zoomed out.
///
/// Two passes (all backings, then all dashes) preserve the original draw order
/// so white dashes always render on top of any backing at corner overlaps.
///
/// Contour points are **window-local** (the selection texture is window-sized).
/// `win_ox` / `win_oy` place the contour buffer within that window-local frame;
/// `canvas_origin` then lifts the result into **plane** space, which is what the
/// overlay's `FLAG_CANVAS_SPACE` (plane) primitives are interpreted in. Omitting
/// `canvas_origin` is what left the ants short by the crop offset.
fn emit_marching_ants(
    polylines: &[Vec<[f32; 2]>],
    win_ox: f32,
    win_oy: f32,
    canvas_origin: crate::coord::CanvasPoint,
    out: &mut Vec<OverlayPrimitive>,
) {
    let ox = win_ox + canvas_origin.x as f32;
    let oy = win_oy + canvas_origin.y as f32;
    for polyline in polylines {
        for w in polyline.windows(2) {
            let a = [w[0][0] + ox, w[0][1] + oy];
            let b = [w[1][0] + ox, w[1][1] + oy];
            let mut bg = OverlayPrimitive::new(KIND_DASHED_LINE, FLAG_CANVAS_SPACE, a, b);
            bg.color = [0.0, 0.0, 0.0, 1.0];
            bg.thickness = 1.5;
            bg.dash_len = 0.0;
            out.push(bg);
        }
    }
    for polyline in polylines {
        let mut arc = 0.0_f32;
        for w in polyline.windows(2) {
            let a = [w[0][0] + ox, w[0][1] + oy];
            let b = [w[1][0] + ox, w[1][1] + oy];
            let mut fg = OverlayPrimitive::new(KIND_DASHED_LINE, FLAG_CANVAS_SPACE, a, b);
            fg.color = [1.0, 1.0, 1.0, 1.0];
            fg.thickness = 1.0;
            fg.dash_len = 8.0;
            fg.dash_offset = arc;
            out.push(fg);

            let dx = b[0] - a[0];
            let dy = b[1] - a[1];
            arc += (dx * dx + dy * dy).sqrt();
        }
    }
}

/// Reinterpret window-local selection bounds as a rect in the selection
/// texture's **undo frame**. That frame is itself window-local-at-origin-0
/// (`SelectionState::canvas_frame`), so the numbers carry over directly — this
/// is *not* a `canvas_origin` lift to plane (use `WindowRect::to_canvas` for
/// that). Kept as one named conversion so the undo path stays in `CanvasRect`
/// (the shared region API) without leaking the window-local fact.
fn selection_undo_frame_rect(b: WindowRect) -> CanvasRect {
    CanvasRect::from_xywh(b.x0(), b.y0(), b.width, b.height)
}

#[handlers]
impl DarklyEngine {
    // ========================================================================
    // Bridge helpers — read/write the selection's split state through one
    // facade so consumers don't have to know whether a fact lives on the
    // document filter or the compositor's GPU state.
    // ========================================================================

    /// True when the selection filter is allocated AND its visibility flag
    /// is set. Equivalent to the old `gpu_selection.active`.
    #[handler]
    pub fn has_selection(&self) -> bool {
        self.doc.selection_active()
    }

    /// CPU mirror of the selection's R8 texture, if present. Populated by the
    /// async `SelectionReadback` and by the `Replace` upload paths (which have
    /// the data in hand). Cleared after combine/invert until the next
    /// readback lands. Read-only access — engine helpers above mutate.
    pub fn selection_cpu_cache(&self) -> Option<&[u8]> {
        let id = self.doc.selection?;
        self.doc
            .find_filter(id)
            .and_then(|m| m.as_selection())
            .and_then(|s| s.cpu_cache.data.as_deref())
    }

    /// Cached tight bounds of the non-zero selection region, in
    /// **window-local** coords (the selection texture is window-sized; lift to
    /// plane with `.to_canvas(self.doc.canvas_origin)`).
    pub(crate) fn selection_pixel_bounds(&self) -> Option<WindowRect> {
        let id = self.doc.selection?;
        self.doc
            .find_filter(id)
            .and_then(|m| m.as_selection())
            .and_then(|s| s.pixel_bounds)
    }

    /// Selection filter id, if allocated.
    pub(crate) fn selection_modifier_id(&self) -> Option<LayerId> {
        self.doc.selection_id()
    }

    /// Set / clear the selection's tight pixel bounds (called after Replace
    /// or after an async readback recomputes them).
    pub(crate) fn set_selection_pixel_bounds(&mut self, bounds: Option<WindowRect>) {
        let id = match self.doc.selection {
            Some(id) => id,
            None => return,
        };
        if let Some(s) = self
            .doc
            .find_filter_mut(id)
            .and_then(|m| m.as_selection_mut())
        {
            s.pixel_bounds = bounds;
        }
    }

    /// Replace the CPU mirror of the selection texture.
    pub(crate) fn set_selection_cpu_cache(&mut self, data: Vec<u8>) {
        let id = match self.doc.selection {
            Some(id) => id,
            None => return,
        };
        if let Some(s) = self
            .doc
            .find_filter_mut(id)
            .and_then(|m| m.as_selection_mut())
        {
            s.cpu_cache.set(data);
        }
    }

    /// Invalidate the CPU mirror — called after combine/invert.
    pub(crate) fn invalidate_selection_cpu_cache(&mut self) {
        let id = match self.doc.selection {
            Some(id) => id,
            None => return,
        };
        if let Some(s) = self
            .doc
            .find_filter_mut(id)
            .and_then(|m| m.as_selection_mut())
        {
            s.cpu_cache.invalidate();
        }
    }

    /// Toggle the active flag (mapped onto `common.visible`). Engine internal —
    /// public visibility toggling is via [`Self::set_layer_visible`].
    pub(crate) fn set_selection_active(&mut self, active: bool) {
        let id = match self.doc.selection {
            Some(id) => id,
            None => return,
        };
        if let Some(filter) = self.doc.find_filter_mut(id) {
            filter.common.visible = active;
        }
    }

    // ========================================================================
    // Selection ops — the user-facing shape fills, booleans, invert, clear.
    // ========================================================================

    #[handler]
    pub fn select_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        // Tool input is in plane space; the selection mask is window-sized, so
        // shift into window-local coords before rasterizing (identity for an
        // un-cropped canvas where `canvas_origin == (0, 0)`).
        let (x, y) = self.plane_to_window_local(x, y);
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let half_w = w * 0.5;
        let half_h = h * 0.5;

        let mask = crate::mask::rasterize_sdf_r8(
            self.doc.width,
            self.doc.height,
            (x as i32, y as i32, w.ceil() as i32, h.ceil() as i32),
            |px, py| crate::sdf::sdf_rect(px, py, cx, cy, half_w, half_h),
            antialias,
            feather,
        );
        self.apply_selection_mask(mask, mode);
    }

    #[handler]
    pub fn select_ellipse(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let (x, y) = self.plane_to_window_local(x, y);
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let rx = w * 0.5;
        let ry = h * 0.5;

        let mask = crate::mask::rasterize_sdf_r8(
            self.doc.width,
            self.doc.height,
            (x as i32, y as i32, w.ceil() as i32, h.ceil() as i32),
            |px, py| crate::sdf::sdf_ellipse(px, py, cx, cy, rx, ry),
            antialias,
            feather,
        );
        self.apply_selection_mask(mask, mode);
    }

    pub fn select_lasso(
        &mut self,
        vertices: &[[f32; 2]],
        mode: SelectionMode,
        antialias: bool,
        _feather: f32,
    ) {
        if vertices.len() < 3 {
            return;
        }

        // Plane → window-local for the window-sized mask (see `select_rect`),
        // through the same chokepoint as the other selection shapes.
        let local: Vec<[f32; 2]> = vertices
            .iter()
            .map(|&[vx, vy]| {
                let (lx, ly) = self.plane_to_window_local(vx, vy);
                [lx, ly]
            })
            .collect();
        let mask =
            crate::mask::rasterize_polygon_r8(self.doc.width, self.doc.height, &local, antialias);
        self.apply_selection_mask(mask, mode);
    }

    /// Convert a plane-space point to the window-local coordinates the
    /// window-sized selection mask is rasterized in. Identity for an
    /// un-cropped canvas (`canvas_origin == (0, 0)`).
    fn plane_to_window_local(&self, x: f32, y: f32) -> (f32, f32) {
        let o = self.doc.canvas_origin;
        (x - o.x as f32, y - o.y as f32)
    }

    #[handler]
    pub fn select_magic_wand(
        &mut self,
        id: LayerId,
        seed_canvas: crate::coord::CanvasPoint,
        tolerance: u8,
        mode: SelectionMode,
    ) {
        if self.paint_target(id).is_none() {
            return;
        }

        let was_active = self.has_selection();
        // Magic wand operates on full-canvas data — reserve full-canvas undo rect.
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);

        let pt = self.paint_target(id).unwrap();
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("magic-wand-readback"),
            });
        let (request, extent) =
            flood_fill::request_layer_flood_fill_readback(&self.gpu.device, &mut encoder, &pt);
        self.gpu.queue.submit([encoder.finish()]);
        self.readbacks.submit(
            request,
            ReadbackContext::MagicWand {
                was_active,
                node_id: id,
                seed_canvas,
                tolerance,
                mode,
                extent,
            },
        );
    }

    pub(crate) fn complete_magic_wand(
        &mut self,
        was_active: bool,
        _node_id: LayerId,
        seed_canvas: crate::coord::CanvasPoint,
        tolerance: u8,
        mode: SelectionMode,
        extent: LayerFloodFillExtent,
        pixels: Vec<u8>,
    ) {
        let fill_mask = extent.flood_fill_to_canvas_mask(&pixels, seed_canvas, tolerance);
        self.apply_selection_full(fill_mask, mode, was_active);
    }

    #[handler]
    pub fn clear_selection(&mut self) {
        if !self.resolve_transform_conflict() {
            return;
        }
        if let Some((was_active, entry)) = self.clear_selection_collecting_undo() {
            self.push_undo(Box::new(SelectionAction::new(was_active, entry)));
        }
    }

    /// Clear the active selection and return its undo data `(was_active,
    /// entry)` **without** pushing an undo step — the caller records it. The
    /// public [`clear_selection`](Self::clear_selection) wraps it in a
    /// `SelectionAction`; image rescale folds the pair into its single undo
    /// step (so a rescale-with-active-selection undoes in one operation).
    /// Returns `None` when there is no active selection.
    pub(crate) fn clear_selection_collecting_undo(
        &mut self,
    ) -> Option<(bool, crate::gpu::region_store::UndoRegionEntry)> {
        if !self.has_selection() {
            return None;
        }
        let rect = self
            .selection_pixel_bounds()
            .map(selection_undo_frame_rect)
            .unwrap_or_else(|| self.selection_full_canvas_rect());
        self.save_selection_for_undo(rect);
        let was_active = self.has_selection();

        let bounds = self.selection_pixel_bounds();
        if let Some(state) = self.compositor.selection_state_mut() {
            state.clear_region(&self.gpu.queue, bounds);
        }
        self.set_selection_pixel_bounds(None);
        self.set_selection_active(false);
        self.invalidate_selection_cpu_cache();

        let entry = self.commit_selection_undo_entry(rect)?;
        self.clear_channel_overlay(OverlayChannel::Selection);
        Some((was_active, entry))
    }

    #[handler]
    pub fn select_all(&mut self) {
        if !self.resolve_transform_conflict() {
            return;
        }
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);
        let was_active = self.has_selection();

        let w = self.doc.width;
        let h = self.doc.height;
        let mask = RasterizedMask {
            data: vec![255u8; (w * h) as usize],
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        self.upload_selection_replace(&mask);

        self.commit_selection_undo(was_active, rect);
        self.generate_contours_from_mask(&mask);
    }

    #[handler]
    pub fn invert_selection(&mut self) {
        if !self.resolve_transform_conflict() || !self.has_selection() {
            return;
        }
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);
        let was_active = self.has_selection();

        if let Some(state) = self.compositor.selection_state_mut() {
            self.gpu.encode("invert-sel", |encoder| {
                self.selection_pipelines
                    .invert(encoder, &self.gpu.device, &self.gpu.queue, state);
            });
        }
        self.set_selection_pixel_bounds(None);
        self.invalidate_selection_cpu_cache();
        self.commit_selection_undo(was_active, rect);
        self.kick_selection_readback();
    }

    /// Grow (dilate) the active selection by `radius` pixels. No-op without an
    /// active selection or for `radius == 0`.
    #[handler]
    pub fn grow_selection(&mut self, radius: u32) {
        self.morph_selection(MorphOp::Dilate, radius);
    }

    /// Shrink (erode) the active selection by `radius` pixels.
    #[handler]
    pub fn shrink_selection(&mut self, radius: u32) {
        self.morph_selection(MorphOp::Erode, radius);
    }

    /// Replace the active selection with a band of width `radius` straddling
    /// its edge (`dilate − erode`).
    #[handler]
    pub fn border_selection(&mut self, radius: u32) {
        if radius == 0 || !self.has_selection() {
            return;
        }
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);
        let was_active = self.has_selection();
        if let Some(state) = self.compositor.selection_state_mut() {
            self.selection_pipelines.border(&self.gpu, state, radius);
        }
        self.finish_selection_edit(was_active, rect);
    }

    /// Smooth the active selection: a morphological open then close (`erode N →
    /// dilate 2N → erode N`), removing specks and pinholes and rounding jagged
    /// edges.
    #[handler]
    pub fn smooth_selection(&mut self, radius: u32) {
        if radius == 0 || !self.has_selection() {
            return;
        }
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);
        let was_active = self.has_selection();
        self.run_morph_steps(MorphOp::Erode, radius);
        self.run_morph_steps(MorphOp::Dilate, 2 * radius);
        self.run_morph_steps(MorphOp::Erode, radius);
        self.finish_selection_edit(was_active, rect);
    }

    /// Feather (Gaussian-blur) the active selection edge by `radius` pixels.
    #[handler]
    pub fn feather_selection(&mut self, radius: f32) {
        if radius <= 0.0 || !self.has_selection() {
            return;
        }
        self.blur_selection(radius);
    }

    /// Antialias the active selection — a fixed small-radius Gaussian blur that
    /// softens the staircase of a hard-edged selection.
    #[handler]
    pub fn antialias_selection(&mut self) {
        if !self.has_selection() {
            return;
        }
        self.blur_selection(1.0);
    }

    /// Grow or shrink the active selection by `radius` single-pixel morphology
    /// steps, wrapped in one undo step. Shared by [`grow_selection`] /
    /// [`shrink_selection`].
    fn morph_selection(&mut self, op: MorphOp, radius: u32) {
        if radius == 0 || !self.has_selection() {
            return;
        }
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);
        let was_active = self.has_selection();
        self.run_morph_steps(op, radius);
        self.finish_selection_edit(was_active, rect);
    }

    /// Run `count` single-pixel morphology steps over the selection mask,
    /// alternating 4-/8-connectivity for a rounder structuring element. Each
    /// step is its own submit (the per-step uniform varies). No undo handling —
    /// the caller brackets the whole edit.
    fn run_morph_steps(&mut self, op: MorphOp, count: u32) {
        for i in 0..count {
            let eight = i % 2 == 1;
            if let Some(state) = self.compositor.selection_state_mut() {
                self.gpu.encode("sel-morph", |encoder| {
                    self.selection_pipelines.morph_step(
                        encoder,
                        &self.gpu.device,
                        &self.gpu.queue,
                        state,
                        op,
                        eight,
                    );
                });
            }
        }
    }

    /// Separable Gaussian blur (H then V) of the selection mask, wrapped in one
    /// undo step. σ = radius / 2, kernel extent ±ceil(radius) — matching
    /// [`crate::mask::gaussian_kernel`]. Shared by feather / antialias.
    fn blur_selection(&mut self, radius: f32) {
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);
        let was_active = self.has_selection();
        let sigma = radius * 0.5;
        let krad = radius.ceil() as i32;
        for horizontal in [true, false] {
            if let Some(state) = self.compositor.selection_state_mut() {
                self.gpu.encode("sel-blur", |encoder| {
                    self.selection_pipelines.blur_pass(
                        encoder,
                        &self.gpu.device,
                        &self.gpu.queue,
                        state,
                        horizontal,
                        krad,
                        sigma,
                    );
                });
            }
        }
        self.finish_selection_edit(was_active, rect);
    }

    /// Post-edit bookkeeping shared by the GPU selection-modify ops: drop the
    /// cached pixel bounds and CPU mirror (an async readback repopulates them),
    /// commit the undo step, and kick the readback for marching ants.
    fn finish_selection_edit(&mut self, was_active: bool, rect: CanvasRect) {
        self.set_selection_pixel_bounds(None);
        self.invalidate_selection_cpu_cache();
        self.commit_selection_undo(was_active, rect);
        self.kick_selection_readback();
    }

    #[handler]
    pub fn clear_selection_contents(&mut self, id: LayerId) {
        self.auto_commit_floating();
        if !self.has_selection() {
            return;
        }
        self.gpu_clear_selection(id);
    }

    // --- Core selection application ---

    /// Apply a tight-bounds rasterized mask (from SDF tools).
    pub(crate) fn apply_selection_mask(&mut self, mask: RasterizedMask, mode: SelectionMode) {
        if !self.resolve_transform_conflict() {
            return;
        }
        let was_active = self.has_selection();
        let rect = self.selection_undo_rect_for_shape([mask.x, mask.y, mask.width, mask.height]);
        self.save_selection_for_undo(rect);

        match mode {
            SelectionMode::Replace => {
                self.upload_selection_replace(&mask);
                self.generate_contours_from_mask(&mask);
            }
            _ => {
                let cw = self.doc.width;
                let ch = self.doc.height;
                let mut full = vec![0u8; (cw * ch) as usize];
                for y in 0..mask.height {
                    let src = (y * mask.width) as usize;
                    let dst = ((mask.y + y) * cw + mask.x) as usize;
                    full[dst..dst + mask.width as usize]
                        .copy_from_slice(&mask.data[src..src + mask.width as usize]);
                }
                self.apply_combine(&full, mode);
                self.kick_selection_readback();
            }
        }

        self.commit_selection_undo(was_active, rect);
    }

    /// Generate marching ants contours directly from a RasterizedMask (no readback).
    fn generate_contours_from_mask(&mut self, mask: &RasterizedMask) {
        self.overlays[OverlayChannel::Selection.index()].clear();

        if mask.width == 0 || mask.height == 0 {
            self.push_merged_overlay();
            return;
        }

        let pad = 1u32;
        let bw = mask.width + 2 * pad;
        let bh = mask.height + 2 * pad;
        let mut buf = vec![0u8; (bw * bh) as usize];
        for y in 0..mask.height {
            let src = (y * mask.width) as usize;
            let dst = ((y + pad) * bw + pad) as usize;
            buf[dst..dst + mask.width as usize]
                .copy_from_slice(&mask.data[src..src + mask.width as usize]);
        }

        let polylines = crate::mask::contour_polylines_r8(&buf, bw, bh, 127);

        let ox = mask.x as f32 - pad as f32;
        let oy = mask.y as f32 - pad as f32;
        let origin = self.doc.canvas_origin;
        emit_marching_ants(
            &polylines,
            ox,
            oy,
            origin,
            &mut self.overlays[OverlayChannel::Selection.index()],
        );

        self.push_merged_overlay();
    }

    /// Apply a full-canvas R8 buffer (from magic wand, mask-to-selection).
    pub(crate) fn apply_selection_full(
        &mut self,
        shape_pixels: Vec<u8>,
        mode: SelectionMode,
        was_active: bool,
    ) {
        if !self.resolve_transform_conflict() {
            return;
        }
        match mode {
            SelectionMode::Replace => {
                self.upload_selection_replace_full(&shape_pixels);
            }
            _ => {
                self.apply_combine(&shape_pixels, mode);
            }
        }

        let rect = self.selection_full_canvas_rect();
        self.commit_selection_undo(was_active, rect);
        self.kick_selection_readback();
    }

    /// Run the GPU combine shader for boolean modes.
    fn apply_combine(&mut self, shape_pixels: &[u8], mode: SelectionMode) {
        let combine_mode = CombineMode::from_selection_mode(&mode);
        if let Some(state) = self.compositor.selection_state_mut() {
            self.gpu.encode("sel-combine", |encoder| {
                self.selection_pipelines.combine(
                    encoder,
                    &self.gpu.device,
                    &self.gpu.queue,
                    state,
                    shape_pixels,
                    combine_mode,
                );
            });
        }
        self.set_selection_pixel_bounds(None);
        self.invalidate_selection_cpu_cache();
        // Combine implies a selection now exists.
        self.set_selection_active(true);
    }

    /// Push a tight-bounds replace into the GPU + sync doc-side bounds and CPU
    /// cache. Used by `Replace` shape ops and `select_all`.
    fn upload_selection_replace(&mut self, mask: &RasterizedMask) {
        let old_bounds = self.selection_pixel_bounds();
        if let Some(state) = self.compositor.selection_state_mut() {
            state.upload_replace(&self.gpu.device, &self.gpu.queue, old_bounds, mask);
        }

        // Doc-side: tight bounds, CPU cache, active. The raster buffer is
        // inflated by an AA/feather margin (see `rasterize_sdf_r8`); for
        // pixel-aligned shapes those margin pixels have coverage = 0 exactly.
        // Reporting the inflated buffer dimensions as the selection bounds
        // leaks a zero-coverage border into downstream consumers (copy/cut
        // would read back an oversized region whose perimeter the GPU mask
        // multiply turns transparent).
        let tight_bounds = crate::mask::pixel_bounds_r8(&mask.data, mask.width, mask.height).map(
            |[bx, by, bw, bh]| {
                WindowRect::from_xywh((mask.x + bx) as i32, (mask.y + by) as i32, bw, bh)
            },
        );
        self.set_selection_pixel_bounds(tight_bounds);
        self.set_selection_active(true);

        let cw = self.doc.width;
        let mut cache = vec![0u8; (cw * self.doc.height) as usize];
        for y in 0..mask.height {
            let src = (y * mask.width) as usize;
            let dst = ((mask.y + y) * cw + mask.x) as usize;
            cache[dst..dst + mask.width as usize]
                .copy_from_slice(&mask.data[src..src + mask.width as usize]);
        }
        self.set_selection_cpu_cache(cache);
    }

    /// Full-canvas R8 replace — sets bounds from the buffer's non-zero region
    /// and seeds the CPU cache directly.
    pub(crate) fn upload_selection_replace_full(&mut self, data: &[u8]) {
        if let Some(state) = self.compositor.selection_state_mut() {
            state.upload_replace_full(&self.gpu.device, &self.gpu.queue, data);
        }

        let bounds = crate::mask::pixel_bounds_r8(data, self.doc.width, self.doc.height)
            .map(|[x, y, w, h]| WindowRect::from_xywh(x as i32, y as i32, w, h));
        self.set_selection_pixel_bounds(bounds);
        self.set_selection_active(true);
        self.set_selection_cpu_cache(data.to_vec());
    }

    // --- Undo helpers ---

    pub(crate) fn save_selection_for_undo(&mut self, rect: CanvasRect) {
        let frame = match self.compositor.selection_state() {
            Some(s) => s.canvas_frame(),
            None => return,
        };
        let snap = self.gpu.encode_ret("sel-undo-save", |encoder| {
            self.region_scratch.save_region(
                &self.gpu.device,
                encoder,
                &frame,
                wgpu::TextureFormat::R8Unorm,
                rect,
            )
        });
        self.pending_selection_snapshot = Some(snap);
    }

    pub(crate) fn commit_selection_undo(&mut self, was_active: bool, rect: CanvasRect) {
        if let Some(entry) = self.commit_selection_undo_entry(rect) {
            self.push_undo(Box::new(SelectionAction::new(was_active, entry)));
        }
    }

    /// Build the selection's GPU undo entry from the pending snapshot (paired
    /// with a prior [`save_selection_for_undo`](Self::save_selection_for_undo))
    /// without pushing an action. Returns `None` if the snapshot or selection
    /// filter is missing.
    pub(crate) fn commit_selection_undo_entry(
        &mut self,
        rect: CanvasRect,
    ) -> Option<crate::gpu::region_store::UndoRegionEntry> {
        let Some(snap) = self.pending_selection_snapshot.take() else {
            debug_assert!(false, "commit_selection_undo without a paired save");
            return None;
        };
        let filter_id = match self.selection_modifier_id() {
            Some(id) => id,
            None => {
                debug_assert!(false, "commit_selection_undo without a selection filter");
                return None;
            }
        };
        let frame = self.compositor.selection_state()?.canvas_frame();
        Some(commit_undo_region(
            &self.gpu,
            &self.region_scratch,
            &mut self.readbacks,
            "sel-undo-commit",
            filter_id,
            &frame,
            &snap,
            rect,
        ))
    }

    /// Full-canvas undo rect — used when post-op extent isn't known up-front.
    ///
    /// Window-local `(0, 0, w, h)`: the selection mask is a window-sized R8
    /// texture and its `canvas_frame` is window-local, so selection undo rects
    /// (saved/restored against that frame) stay window-local too. The plane
    /// anchoring of the selection is realized physically by re-copying the
    /// mask on crop/resize (`set_canvas_rect`), not by plane-space undo rects.
    pub(crate) fn selection_full_canvas_rect(&self) -> CanvasRect {
        CanvasRect::from_xywh(0, 0, self.doc.width, self.doc.height)
    }

    /// Undo rect that covers both the current (pre-op) selection and a new
    /// shape that's about to be applied. Save and commit must use the same
    /// rect — otherwise stale bytes outside the save rect leak into the
    /// commit and corrupt the selection on undo.
    pub(crate) fn selection_undo_rect_for_shape(&self, shape: [u32; 4]) -> CanvasRect {
        let cw = self.doc.width;
        let ch = self.doc.height;
        let [sx, sy, sw, sh] = shape;
        let [sx, sy, sw, sh] = [
            sx.min(cw),
            sy.min(ch),
            sw.min(cw - sx.min(cw)),
            sh.min(ch - sy.min(ch)),
        ];
        let shape_rect = CanvasRect::from_xywh(sx as i32, sy as i32, sw, sh);
        let old_frame = self.selection_pixel_bounds().map(selection_undo_frame_rect);
        match old_frame {
            Some(old) if sw > 0 && sh > 0 => old.union(shape_rect),
            Some(old) => old,
            None => CanvasRect::from_xywh(0, 0, cw, ch),
        }
    }

    /// Kick an async readback for contour extraction (marching ants) and CPU
    /// cache repopulation.
    pub(crate) fn kick_selection_readback(&mut self) {
        let w = self.doc.width;
        let h = self.doc.height;
        let texture = match self.compositor.selection_state() {
            Some(s) => s.texture(),
            None => return,
        };
        self.gpu.encode("sel-readback", |encoder| {
            // Selection texture is canvas-aligned: canvas coords == layer
            // coords here.
            let request = readback::request_readback(
                &self.gpu.device,
                encoder,
                texture,
                wgpu::TextureFormat::R8Unorm,
                crate::coord::LayerRect::from_xywh(0, 0, w, h),
            );
            self.readbacks
                .submit(request, ReadbackContext::SelectionReadback);
        });
    }

    // --- Selection overlay ---

    /// Regenerate marching ants from readback data and update CPU cache.
    pub(crate) fn update_selection_overlay_from_readback(&mut self, pixels: Vec<u8>) {
        self.overlays[OverlayChannel::Selection.index()].clear();

        if !self.has_selection() {
            self.push_merged_overlay();
            return;
        }

        if self.selection_pixel_bounds().is_none() {
            let bounds = crate::mask::pixel_bounds_r8(&pixels, self.doc.width, self.doc.height)
                .map(|[x, y, w, h]| WindowRect::from_xywh(x as i32, y as i32, w, h));
            self.set_selection_pixel_bounds(bounds);
        }

        self.set_selection_cpu_cache(pixels.clone());

        let polylines =
            crate::mask::contour_polylines_r8(&pixels, self.doc.width, self.doc.height, 127);
        let origin = self.doc.canvas_origin;
        emit_marching_ants(
            &polylines,
            0.0,
            0.0,
            origin,
            &mut self.overlays[OverlayChannel::Selection.index()],
        );

        self.push_merged_overlay();
    }

    /// Merge every overlay channel in z-order and push to the compositor's
    /// single overlay slot. Channels render bottom-to-top in
    /// [`OverlayChannel::ALL`] order (`Selection`, `CloneSource`, `Tool`),
    /// so the dab preview lands on top. A new channel is additive — extend
    /// the enum and it folds in here with no edit.
    pub(crate) fn push_merged_overlay(&mut self) {
        let merged = merge_overlay_channels(&self.overlays);
        if merged.is_empty() {
            self.compositor.tool_overlay_mut().clear_primitives();
        } else {
            self.compositor.tool_overlay_mut().set_primitives(merged);
        }
        self.compositor.mark_needs_present();
    }

    // --- Channel overlays ---

    /// Replace one channel's primitives wholesale and re-merge. Other
    /// channels are untouched, so the clone source marker survives the dab
    /// preview's every-hover-move `Tool` replacement.
    pub(crate) fn set_channel_overlay(
        &mut self,
        channel: OverlayChannel,
        prims: Vec<OverlayPrimitive>,
    ) {
        self.overlays[channel.index()] = prims;
        self.push_merged_overlay();
    }

    /// Clear one channel's primitives and re-merge.
    pub(crate) fn clear_channel_overlay(&mut self, channel: OverlayChannel) {
        self.overlays[channel.index()].clear();
        self.push_merged_overlay();
    }

    // --- Tool Overlay ---

    pub fn set_overlay_primitives(&mut self, prims: Vec<OverlayPrimitive>) {
        self.set_channel_overlay(OverlayChannel::Tool, prims);
    }

    #[handler]
    pub fn clear_overlay(&mut self) {
        self.clear_channel_overlay(OverlayChannel::Tool);
    }

    #[handler]
    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize> {
        self.compositor.tool_overlay().hit_test(screen_x, screen_y)
    }

    /// Upload the mask texture sampled by KIND_MASKED_STAMP overlay primitives.
    #[handler]
    pub fn set_overlay_mask(&mut self, width: u32, height: u32, rgba: &[u8]) {
        self.compositor.tool_overlay_mut().set_mask_texture(
            &self.gpu.device,
            &self.gpu.queue,
            width,
            height,
            rgba,
        );
        self.compositor.mark_needs_present();
    }

    #[handler]
    pub fn clear_overlay_mask(&mut self) {
        self.compositor.tool_overlay_mut().clear_mask_texture();
        self.compositor.mark_needs_present();
    }
}

#[cfg(test)]
mod overlay_channel_tests {
    use super::*;
    use crate::gpu::overlay::KIND_LINE;

    /// Tag a primitive by channel via its `p0.x` so the merged order is
    /// checkable without a GPU.
    fn tagged(tag: f32) -> OverlayPrimitive {
        OverlayPrimitive::new(KIND_LINE, 0, [tag, 0.0], [0.0, 0.0])
    }

    fn channels_with(
        sel: &[f32],
        clone: &[f32],
        tool: &[f32],
    ) -> [Vec<OverlayPrimitive>; OverlayChannel::COUNT] {
        let mut ch: [Vec<OverlayPrimitive>; OverlayChannel::COUNT] = Default::default();
        ch[OverlayChannel::Selection.index()] = sel.iter().map(|&t| tagged(t)).collect();
        ch[OverlayChannel::CloneSource.index()] = clone.iter().map(|&t| tagged(t)).collect();
        ch[OverlayChannel::Tool.index()] = tool.iter().map(|&t| tagged(t)).collect();
        ch
    }

    /// The merge concatenates Selection, then CloneSource, then Tool — the
    /// dab (`Tool`) lands on top.
    #[test]
    fn merges_in_z_order() {
        let ch = channels_with(&[1.0], &[2.0, 3.0], &[4.0]);
        let merged: Vec<f32> = merge_overlay_channels(&ch)
            .iter()
            .map(|p| p.p0[0])
            .collect();
        assert_eq!(merged, vec![1.0, 2.0, 3.0, 4.0]);
    }

    /// Clearing the `Tool` channel leaves the `CloneSource` marker intact —
    /// the property that lets the source crosshair survive the dab preview's
    /// every-hover-move replacement.
    #[test]
    fn clone_source_survives_tool_clear() {
        let mut ch = channels_with(&[1.0], &[2.0], &[4.0]);
        ch[OverlayChannel::Tool.index()].clear();
        let merged: Vec<f32> = merge_overlay_channels(&ch)
            .iter()
            .map(|p| p.p0[0])
            .collect();
        assert_eq!(
            merged,
            vec![1.0, 2.0],
            "CloneSource must persist after a Tool-channel clear"
        );
    }
}
