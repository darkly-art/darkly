//! GPU context bundle passed to brush node evaluators during `execute_gpu`.
//!
//! Provides everything a GPU node needs: command encoder, device, queue,
//! dab texture pool, pipelines, canvas target, and selection bind group.

use std::collections::HashMap;

use super::dab_pool::DabTexturePool;
use super::pipelines::BrushPipelines;
use super::wire::TextureHandle;

/// Which terminal node should actually do GPU work during this pass.
///
/// Brush graphs have two terminals — `color_output` writes to the canvas,
/// `preview_output` writes to the overlay preview mask. Only one runs per
/// pass; the other's `evaluate_gpu` is a no-op when the mode doesn't match.
/// All non-terminal nodes (stamp, circle, etc.) are mode-agnostic and run
/// identically either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    /// Normal stroke path. `color_output` composites; `preview_output` skips.
    Stroke,
    /// Preview regen. `preview_output` blits into the preview mask;
    /// `color_output` skips.
    Preview,
}

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
    pub canvas_view: &'a wgpu::TextureView,
    /// The canvas layer texture (needed for copy_texture_to_texture).
    pub canvas_texture: &'a wgpu::Texture,
    pub canvas_width: u32,
    pub canvas_height: u32,
    /// Selection mask bind group (or default 1x1 white when no selection).
    pub selection_bind_group: &'a wgpu::BindGroup,
    /// Resource name → TextureHandle for images uploaded by the preset loader.
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
    /// Which terminal should run in this pass. Only inspected by terminal
    /// nodes (`color_output`, `preview_output`). Non-terminals ignore.
    pub render_mode: RenderMode,
    /// Preview mask target. Populated by the engine when `render_mode ==
    /// Preview`; the `preview_output` node renders into it. `None` in stroke
    /// mode.
    pub preview_target_view: Option<&'a wgpu::TextureView>,
    pub preview_target_size: (u32, u32),
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
                self.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor { label: Some("brush-ring-flush") },
                ),
            );
            self.queue.submit([finished.finish()]);
            self.pipelines.reset_uniform_rings();
        }
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
                texture: self.canvas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: origin_x, y: origin_y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: self.pipelines.canvas_copy_texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.canvas_copy_origin = Some([origin_x, origin_y]);
    }
}
