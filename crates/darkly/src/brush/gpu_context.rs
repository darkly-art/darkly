//! GPU context bundle passed to brush node evaluators during `execute_gpu`.
//!
//! Provides everything a GPU node needs: command encoder, device, queue,
//! dab texture pool, pipelines, canvas target, and selection bind group.

use super::dab_pool::DabTexturePool;
use super::pipelines::BrushPipelines;

/// Everything a GPU brush node needs to record render passes.
///
/// Created per-dab by the stroke engine (Phase 4) and passed to
/// `BrushGraphRunner::execute_gpu()`.  The encoder accumulates all
/// render passes for one dab — the caller submits the encoder after
/// all dabs for the current `move_to()` are processed.
pub struct BrushGpuContext<'a> {
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub dab_pool: &'a mut DabTexturePool,
    pub pipelines: &'a BrushPipelines,
    pub canvas_view: &'a wgpu::TextureView,
    pub canvas_width: u32,
    pub canvas_height: u32,
    /// Selection mask bind group (or default 1x1 white when no selection).
    pub selection_bind_group: &'a wgpu::BindGroup,
}
