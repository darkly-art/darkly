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
/// Created once per `move_to()` by the painting layer and passed to
/// the stroke engine.  Each dab records its render passes into the
/// encoder, then calls `submit_and_reset()` to flush — this ensures
/// `queue.write_buffer` uniform data is consumed before the next dab
/// overwrites it.
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
    /// Global brush scale multiplier applied at composite time.
    /// The node graph renders dabs at internal resolution; this scales the
    /// final canvas footprint.  1.0 = dab pixels map 1:1 to canvas pixels.
    pub global_scale: f32,
    /// Resource name → TextureHandle for images uploaded by the preset loader.
    /// Image nodes read from this to resolve their `resource_name` param.
    pub resource_handles: &'a HashMap<String, TextureHandle>,
}

impl<'a> BrushGpuContext<'a> {
    /// Submit the current encoder and create a fresh one.
    ///
    /// Must be called after each dab so that `queue.write_buffer` uniform
    /// writes are consumed before the next dab overwrites them.
    pub fn submit_and_reset(&mut self) {
        let finished = std::mem::replace(
            &mut self.encoder,
            self.device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor { label: Some("brush-dab") },
            ),
        );
        self.queue.submit([finished.finish()]);
    }
}
