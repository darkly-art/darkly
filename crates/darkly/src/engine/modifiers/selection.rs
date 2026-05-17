//! Engine-level selection ops (shape fills, booleans, active toggle, undo).
//!
//! The global selection is a typed [`crate::document::Modifier`] attached at
//! the document root. Selection state splits cleanly across:
//!
//! - **Document model**: `doc.selection: Option<Modifier>` carries name,
//!   visibility (active toggle), lock, [`crate::layer::PixelBuffer`] bounds,
//!   tight pixel bounds, and the CPU readback cache via [`SelectionModifier`].
//! - **Compositor**: `compositor.selection_state` carries the ping-pong R8
//!   textures, the brush+paint pipeline bind groups, and the modifier id used
//!   for region-store / undo keying.
//! - **Engine**: this file. The high-level ops the user invokes (select_rect,
//!   apply_selection_mask, invert, clear, magic wand, …) plus the bridge
//!   helpers consumers reach for (`selection_active`, `selection_cpu_cache`,
//!   `selection_pixel_bounds`, …).

use super::super::{DarklyEngine, ReadbackContext};
use crate::coord::CanvasRect;
use crate::document::SelectionMode;
use crate::gpu::flood_fill::{self, LayerFloodFillExtent};
use crate::gpu::overlay::{OverlayPrimitive, FLAG_CANVAS_SPACE, KIND_DASHED_LINE};
use crate::gpu::readback;
use crate::gpu::selection::CombineMode;
use crate::layer::LayerId;
use crate::mask::RasterizedMask;
use crate::undo::SelectionAction;

/// Emit marching-ants overlay primitives from contour polylines. Each edge
/// produces a pair of dashed-line primitives — black backing + white dashes —
/// carrying cumulative arc length in `dash_offset` so dash phase flows
/// continuously around the contour. Without that, each edge resets phase at
/// its start: invisible at high zoom, glitchy flicker when zoomed out.
///
/// Two passes (all backings, then all dashes) preserve the original draw order
/// so white dashes always render on top of any backing at corner overlaps.
fn emit_marching_ants(
    polylines: &[Vec<[f32; 2]>],
    ox: f32,
    oy: f32,
    out: &mut Vec<OverlayPrimitive>,
) {
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

impl DarklyEngine {
    // ========================================================================
    // Bridge helpers — read/write the selection's split state through one
    // facade so consumers don't have to know whether a fact lives on the
    // document modifier or the compositor's GPU state.
    // ========================================================================

    /// True when the selection modifier is allocated AND its visibility flag
    /// is set. Equivalent to the old `gpu_selection.active`.
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
            .find_modifier(id)
            .and_then(|m| m.as_selection())
            .and_then(|s| s.cpu_cache.data.as_deref())
    }

    /// Cached tight bounds of the non-zero selection region, in canvas coords.
    pub(crate) fn selection_pixel_bounds(&self) -> Option<CanvasRect> {
        let id = self.doc.selection?;
        self.doc
            .find_modifier(id)
            .and_then(|m| m.as_selection())
            .and_then(|s| s.pixel_bounds)
    }

    /// Selection modifier id, if allocated.
    pub(crate) fn selection_modifier_id(&self) -> Option<LayerId> {
        self.doc.selection_id()
    }

    /// Set / clear the selection's tight pixel bounds (called after Replace
    /// or after an async readback recomputes them).
    pub(crate) fn set_selection_pixel_bounds(&mut self, bounds: Option<CanvasRect>) {
        let id = match self.doc.selection {
            Some(id) => id,
            None => return,
        };
        if let Some(s) = self
            .doc
            .find_modifier_mut(id)
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
            .find_modifier_mut(id)
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
            .find_modifier_mut(id)
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
        if let Some(modifier) = self.doc.find_modifier_mut(id) {
            modifier.common.visible = active;
        }
    }

    // ========================================================================
    // Selection ops — the user-facing shape fills, booleans, invert, clear.
    // ========================================================================

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

        let mask =
            crate::mask::rasterize_polygon_r8(self.doc.width, self.doc.height, vertices, antialias);
        self.apply_selection_mask(mask, mode);
    }

    pub fn select_magic_wand(
        &mut self,
        layer_id: LayerId,
        seed_canvas: crate::coord::CanvasPoint,
        tolerance: u8,
        mode: SelectionMode,
    ) {
        if self.paint_target(layer_id).is_none() {
            return;
        }

        let was_active = self.has_selection();
        // Magic wand operates on full-canvas data — reserve full-canvas undo rect.
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);

        let pt = self.paint_target(layer_id).unwrap();
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
                node_id: layer_id,
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

    pub fn clear_selection(&mut self) {
        if !self.has_selection() {
            return;
        }
        let rect = self
            .selection_pixel_bounds()
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

        self.commit_selection_undo(was_active, rect);
        self.selection_overlay.clear();
        self.push_merged_overlay();
    }

    pub fn select_all(&mut self) {
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

    pub fn invert_selection(&mut self) {
        if !self.has_selection() {
            return;
        }
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);
        let was_active = self.has_selection();

        let brush_bgl = self.brush_pipelines.selection_bind_group_layout();
        let paint_bgl = &self.paint_pipelines.selection_bind_group_layout;
        if let Some(state) = self.compositor.selection_state_mut() {
            self.gpu.encode("invert-sel", |encoder| {
                self.selection_pipelines.invert(
                    encoder,
                    &self.gpu.device,
                    &self.gpu.queue,
                    state,
                    brush_bgl,
                    paint_bgl,
                );
            });
        }
        self.set_selection_pixel_bounds(None);
        self.invalidate_selection_cpu_cache();
        self.commit_selection_undo(was_active, rect);
        self.kick_selection_readback();
    }

    pub fn clear_selection_contents(&mut self, layer_id: LayerId) {
        self.auto_commit_floating();
        if !self.has_selection() {
            return;
        }
        self.gpu_clear_selection(layer_id);
    }

    // --- Core selection application ---

    /// Apply a tight-bounds rasterized mask (from SDF tools).
    pub(crate) fn apply_selection_mask(&mut self, mask: RasterizedMask, mode: SelectionMode) {
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
        self.selection_overlay.clear();

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
        emit_marching_ants(&polylines, ox, oy, &mut self.selection_overlay);

        self.push_merged_overlay();
    }

    /// Apply a full-canvas R8 buffer (from magic wand, mask-to-selection).
    pub(crate) fn apply_selection_full(
        &mut self,
        shape_pixels: Vec<u8>,
        mode: SelectionMode,
        was_active: bool,
    ) {
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
        let brush_bgl = self.brush_pipelines.selection_bind_group_layout();
        let paint_bgl = &self.paint_pipelines.selection_bind_group_layout;
        if let Some(state) = self.compositor.selection_state_mut() {
            self.gpu.encode("sel-combine", |encoder| {
                self.selection_pipelines.combine(
                    encoder,
                    &self.gpu.device,
                    &self.gpu.queue,
                    state,
                    shape_pixels,
                    combine_mode,
                    brush_bgl,
                    paint_bgl,
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
        let brush_bgl = self.brush_pipelines.selection_bind_group_layout();
        let paint_bgl = &self.paint_pipelines.selection_bind_group_layout;
        if let Some(state) = self.compositor.selection_state_mut() {
            state.upload_replace(
                &self.gpu.device,
                &self.gpu.queue,
                old_bounds,
                mask,
                brush_bgl,
                paint_bgl,
            );
        }

        // Doc-side: tight bounds, CPU cache, active.
        self.set_selection_pixel_bounds(Some(CanvasRect::from_xywh(
            mask.x as i32,
            mask.y as i32,
            mask.width,
            mask.height,
        )));
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
        let brush_bgl = self.brush_pipelines.selection_bind_group_layout();
        let paint_bgl = &self.paint_pipelines.selection_bind_group_layout;
        if let Some(state) = self.compositor.selection_state_mut() {
            state.upload_replace_full(
                &self.gpu.device,
                &self.gpu.queue,
                data,
                brush_bgl,
                paint_bgl,
            );
        }

        let bounds = crate::mask::pixel_bounds_r8(data, self.doc.width, self.doc.height)
            .map(|[x, y, w, h]| CanvasRect::from_xywh(x as i32, y as i32, w, h));
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
            self.region_store.save_region(
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
        let Some(snap) = self.pending_selection_snapshot.take() else {
            debug_assert!(false, "commit_selection_undo without a paired save");
            return;
        };
        let modifier_id = match self.selection_modifier_id() {
            Some(id) => id,
            None => {
                debug_assert!(false, "commit_selection_undo without a selection modifier");
                return;
            }
        };
        let frame = match self.compositor.selection_state() {
            Some(s) => s.canvas_frame(),
            None => return,
        };
        self.gpu.encode("sel-undo-commit", |encoder| {
            let entry = self
                .region_store
                .commit_region(encoder, modifier_id, &frame, &snap, rect);
            self.undo_stack.push(
                &mut self.doc,
                Box::new(SelectionAction::new(was_active, entry)),
            );
        });
    }

    /// Full-canvas undo rect — used when post-op extent isn't known up-front.
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
        match self.selection_pixel_bounds() {
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
        self.selection_overlay.clear();

        if !self.has_selection() {
            self.push_merged_overlay();
            return;
        }

        if self.selection_pixel_bounds().is_none() {
            let bounds = crate::mask::pixel_bounds_r8(&pixels, self.doc.width, self.doc.height)
                .map(|[x, y, w, h]| CanvasRect::from_xywh(x as i32, y as i32, w, h));
            self.set_selection_pixel_bounds(bounds);
        }

        self.set_selection_cpu_cache(pixels.clone());

        let polylines =
            crate::mask::contour_polylines_r8(&pixels, self.doc.width, self.doc.height, 127);
        emit_marching_ants(&polylines, 0.0, 0.0, &mut self.selection_overlay);

        self.push_merged_overlay();
    }

    /// Merge selection_overlay + tool_overlay and push to compositor.
    pub(crate) fn push_merged_overlay(&mut self) {
        let mut merged = Vec::with_capacity(self.selection_overlay.len() + self.tool_overlay.len());
        merged.extend_from_slice(&self.selection_overlay);
        merged.extend_from_slice(&self.tool_overlay);
        if merged.is_empty() {
            self.compositor.clear_overlay();
        } else {
            self.compositor.set_overlay_primitives(merged);
        }
    }

    // --- Tool Overlay ---

    pub fn set_overlay_primitives(&mut self, prims: Vec<OverlayPrimitive>) {
        self.tool_overlay = prims;
        self.push_merged_overlay();
    }

    pub fn clear_overlay(&mut self) {
        self.tool_overlay.clear();
        self.push_merged_overlay();
    }

    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize> {
        self.compositor.overlay_hit_test(screen_x, screen_y)
    }

    /// Upload the mask texture sampled by KIND_MASKED_STAMP overlay primitives.
    pub fn set_overlay_mask(&mut self, width: u32, height: u32, rgba: &[u8]) {
        self.compositor
            .set_overlay_mask(&self.gpu.device, &self.gpu.queue, width, height, rgba);
    }

    pub fn clear_overlay_mask(&mut self) {
        self.compositor.clear_overlay_mask();
    }
}
