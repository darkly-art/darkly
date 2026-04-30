//! GPU context bundle passed to brush node evaluators during `execute_gpu`
//! and `render_preview_pipeline`.
//!
//! Provides everything a GPU node needs: command encoder, device, queue,
//! dab texture pool, pipelines, canvas target, and selection bind group.
//! Stroke and preview modes are differentiated by *which* method the runner
//! invokes (`evaluate_gpu` vs `render_preview`), not by a flag on this
//! struct — terminals stop branching on a mode enum.

use std::collections::HashMap;

use super::dab_pool::DabTexturePool;
use super::eval::BrushPreviewInfo;
use super::pipelines::BrushPipelines;
use super::wire::TextureHandle;

/// Everything a GPU brush node needs to record render passes.
///
/// Created once per rendering batch (per-segment in divergence, per-frame
/// in the no-divergence tail) and passed to the stroke engine.  Each dab
/// records its render passes into the encoder.  Dynamic uniform buffer
/// offsets allow all dabs to share one encoder without per-dab submission.
/// Call `submit_final()` when the batch is complete.
pub struct BrushGpuContext<'a> {
    pub encoder: wgpu::CommandEncoder,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub dab_pool: &'a mut DabTexturePool,
    pub pipelines: &'a BrushPipelines,
    /// The stroke scratch texture view — dabs composite into this during
    /// a stroke, and the terminal node commits it onto the layer on every
    /// pen event. Reused as a placeholder in preview mode (nothing writes to it).
    pub stroke_scratch_view: &'a wgpu::TextureView,
    /// The stroke scratch texture (needed for copy_texture_to_texture by
    /// `ensure_canvas_copy`).
    pub stroke_scratch_texture: &'a wgpu::Texture,
    pub canvas_width: u32,
    pub canvas_height: u32,
    /// Layer texture pixel dimensions (== stroke_scratch dims). For
    /// canvas-aligned layers this equals canvas_{width,height}; for paste-
    /// extent layers it is the layer texture's actual size.
    pub layer_width: u32,
    pub layer_height: u32,
    /// Canvas-space offset of the layer texture's (0,0) pixel. Zero for
    /// canvas-aligned layers.
    pub layer_offset_x: i32,
    pub layer_offset_y: i32,
    /// Selection mask bind group (or default 1x1 white when no selection).
    pub selection_bind_group: &'a wgpu::BindGroup,
    /// Resource name → TextureHandle for images uploaded by the brush loader.
    /// Image nodes read from this to resolve their `resource_name` param.
    pub resource_handles: &'a HashMap<String, TextureHandle>,
    /// Composite blend mode override: 0 = source-over (paint), 1 = destination-out (erase).
    /// Set per-stroke by the engine based on the active tool.
    pub blend_mode: u32,
    /// Origin (in canvas pixels) of the valid region in `canvas_copy_texture`
    /// for the current dab, if the copy has already been issued.  `None` means
    /// no copy has been made yet for this dab.
    ///
    /// Multiple GPU nodes per dab may need canvas_copy (e.g. a displacement
    /// node reads it to sample source pixels, color_output reads it for
    /// Porter-Duff).  Tracking origin lets the second caller reuse the
    /// first's copy when regions match.  Reset by `StrokeEngine::place_dab`
    /// before each dab.
    pub canvas_copy_origin: Option<[u32; 2]>,
    /// Preview mask target. Populated by the engine during preview regen;
    /// terminal `render_preview` hooks blit their preview texture into it.
    /// `None` during stroke evaluation (the preview path isn't running).
    pub preview_mask_view: Option<&'a wgpu::TextureView>,
    pub preview_mask_size: (u32, u32),
    /// Set by a terminal's `render_preview` hook to publish overlay
    /// placement info (extent + rotation) to the engine. The engine reads
    /// this after `render_preview_pipeline` returns. `None` outside the
    /// preview path; first-write-wins if multiple terminals try to publish
    /// (unusual — typically one terminal owns the preview).
    pub brush_preview_info: Option<BrushPreviewInfo>,
    /// The actual layer texture view — write target for the terminal's
    /// `commit` hook. `None` in preview mode (no layer to commit to).
    pub layer_view: Option<&'a wgpu::TextureView>,
    /// The actual layer texture (for copy_texture_to_texture at commit).
    pub layer_texture: Option<&'a wgpu::Texture>,
    /// Pre-stroke layer snapshot. Supplied by `StrokeBuffer::save_pre_stroke`
    /// at the start of a stroke. `Some` during a stroke, `None` in preview.
    pub pre_stroke_texture: Option<&'a wgpu::Texture>,
    /// Bind group (canvas-copy BGL) over `pre_stroke_texture`, pre-built
    /// by `StrokeBuffer` so `color_output::commit` can bind it as the
    /// composite background without recreating bind groups every event.
    pub pre_stroke_bind_group: Option<&'a wgpu::BindGroup>,
    /// Bind group (dab BGL) over the stroke scratch, pre-built by
    /// `StrokeBuffer` so `color_output::commit` can bind it as the
    /// composite foreground (the per-dab accumulation).
    pub scratch_bind_group: Option<&'a wgpu::BindGroup>,
    /// Union of canvas-pixel rects the current dab's passes write to. The
    /// node that issues the write is the only thing that knows the real
    /// footprint — stroke_engine can't derive it from `info.pos` because
    /// the graph may offset the dab (scatter, wobble, future
    /// position-modulating nodes). Each pass unions its rect into this via
    /// `push_dab_write_bbox`; stroke_engine reads it after `execute_gpu`
    /// for the save-point bbox and resets it before the next dab. `None`
    /// outside stroke evaluation.
    pub dab_write_canvas_bbox: Option<crate::coord::CanvasRect>,
}

impl<'a> BrushGpuContext<'a> {
    /// Submit the batched encoder and consume the context.
    ///
    /// All dab render passes in this batch are submitted in a single
    /// `queue.submit()` call — no per-dab submission needed thanks to
    /// dynamic uniform buffer offsets.
    pub fn submit_final(self) {
        self.queue.submit([self.encoder.finish()]);
    }

    /// If any uniform ring is nearly full, submit the current encoder,
    /// reset all rings, and create a fresh encoder.  Called between dabs
    /// to prevent ring overflow — adds at most 1 extra submit per ~250
    /// dabs, which is negligible compared to the old per-dab submit.
    pub fn flush_if_needed(&mut self) {
        if self.pipelines.rings_nearly_full() {
            let finished = std::mem::replace(
                &mut self.encoder,
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("brush-ring-flush"),
                    }),
            );
            self.queue.submit([finished.finish()]);
            self.pipelines.reset_uniform_rings();
        }
    }

    /// Union a write-pass footprint into `dab_write_canvas_bbox`. Called by
    /// any GPU node whose pass writes to the stroke scratch, so
    /// stroke_engine can record a save-point bbox that matches what was
    /// actually drawn.
    pub fn push_dab_write_bbox(&mut self, bbox: crate::coord::CanvasRect) {
        if bbox.is_empty() {
            return;
        }
        self.dab_write_canvas_bbox = Some(match self.dab_write_canvas_bbox {
            Some(prev) => prev.union(bbox),
            None => bbox,
        });
    }

    /// Ensure `canvas_copy_texture` holds the canvas region starting at the
    /// given pixel origin, sized to cover `(width, height)`.  Idempotent per
    /// dab: the first caller issues `copy_texture_to_texture`; subsequent
    /// callers with matching origin are no-ops.  Mismatched origins force a
    /// fresh copy.
    ///
    /// Both `smudge_stamp` (canvas sampling) and `color_output` (Porter-Duff
    /// bg) need this, and both compute the same footprint from the same
    /// position — the cache prevents a redundant copy per dab.
    pub fn ensure_canvas_copy(&mut self, origin_x: u32, origin_y: u32, width: u32, height: u32) {
        if self.canvas_copy_origin == Some([origin_x, origin_y]) {
            return;
        }
        if width == 0 || height == 0 {
            return;
        }
        self.encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self.stroke_scratch_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin_x,
                    y: origin_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: self.pipelines.canvas_copy_texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.canvas_copy_origin = Some([origin_x, origin_y]);
    }
}
