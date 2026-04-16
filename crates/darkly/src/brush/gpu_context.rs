//! GPU context bundle passed to brush node evaluators during `execute_gpu`.
//!
//! Provides everything a GPU node needs: command encoder, device, queue,
//! dab texture pool, pipelines, canvas target, and selection bind group.

use std::collections::HashMap;

use super::dab_pool::DabTexturePool;
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
}
