//! Full-stroke brush preview renderer.
//!
//! Runs the real `StrokeEngine` against a self-owned offscreen target to
//! produce a preview of a synthetic S-curve stroke — what the brush would
//! look like in actual use, not a single hover dab. Used by the brush
//! editor's live preview and by brush thumbnail baking.
//!
//! Distinct from the hover overlay path (`render_preview_pipeline` in
//! `eval.rs`), which forces `flow=1` and white color to produce a tip-mask
//! for the cursor-follow overlay. The editor preview runs the real
//! deposition pipeline — `begin_stroke` / `execute_gpu` / `commit` — so
//! flow, opacity, and other per-dab settings affect the output. The
//! stroke/background colors are theme-sourced (set via the engine's
//! `set_preview_theme`), not the active paint color, so all previews
//! share a consistent palette.

use std::collections::HashMap;

use super::dab_pool::DabTexturePool;
use super::gpu_context::BrushGpuContext;
use super::paint_info::PaintInformation;
use super::pipelines::BrushPipelines;
use super::spacing::SpacingConfig;
use super::stabilizer::PassThrough;
use super::stroke_buffer::StrokeBuffer;
use super::stroke_engine::StrokeEngine;
use super::wire::{BrushWireType, TextureHandle};
use crate::nodegraph::Graph;

/// Reusable GPU scratch + layer textures for preview rendering.
struct PreviewTarget {
    width: u32,
    height: u32,
    layer_texture: wgpu::Texture,
    layer_view: wgpu::TextureView,
    stroke_buffer: StrokeBuffer,
}

impl PreviewTarget {
    fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        dab_bgl: &wgpu::BindGroupLayout,
        pipelines: &BrushPipelines,
    ) -> Self {
        let layer_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brush-preview-layer"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let layer_view = layer_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let stroke_buffer = StrokeBuffer::new(device, width, height, dab_bgl, pipelines);
        Self {
            width,
            height,
            layer_texture,
            layer_view,
            stroke_buffer,
        }
    }
}

/// Renders a full-stroke preview into an offscreen RGBA texture using the
/// real stroke engine. One instance is reusable across renders; it holds
/// onto its scratch target between calls and reallocates only on size change.
pub struct BrushPreviewRenderer {
    target: Option<PreviewTarget>,
}

impl BrushPreviewRenderer {
    pub fn new() -> Self {
        Self { target: None }
    }

    /// Render a synthetic stroke into the preview texture.
    ///
    /// Returns the layer texture, GPU-resident — the caller issues any
    /// readback. Returns `None` if the graph fails to compile or `path` is
    /// empty.
    pub fn render_stroke(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dab_pool: &mut DabTexturePool,
        pipelines: &BrushPipelines,
        resource_handles: &HashMap<String, TextureHandle>,
        graph: &Graph<BrushWireType>,
        path: &[PaintInformation],
        fg_color: [f32; 4],
        bg_color: [f32; 4],
        width: u32,
        height: u32,
    ) -> Option<&wgpu::Texture> {
        if path.is_empty() || width == 0 || height == 0 {
            return None;
        }
        // Fresh compile so callers can edit the graph between renders.
        let runner = super::compile_graph(graph).ok()?;

        // Ensure scratch + layer textures match the requested size.
        let target_changed = match &self.target {
            Some(t) => t.width != width || t.height != height,
            None => true,
        };
        if target_changed {
            self.target = Some(PreviewTarget::new(
                device,
                width,
                height,
                dab_pool.bind_group_layout(),
                pipelines,
            ));
        }
        let target = self.target.as_mut().unwrap();

        // Pre-fill the layer with the background color, then snapshot it as
        // the pre-stroke. `color_output::commit` composites the stroke
        // scratch onto this snapshot and writes the result back to the
        // layer — so seeding `bg` here is how the background gets shown.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("brush-preview-pre-fill"),
        });
        {
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-preview-bg-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.layer_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg_color[0] as f64,
                            g: bg_color[1] as f64,
                            b: bg_color[2] as f64,
                            a: bg_color[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        let paint_target = crate::gpu::paint_target::GpuPaintTarget {
            texture: &target.layer_texture,
            view: &target.layer_view,
            format: wgpu::TextureFormat::Rgba8Unorm,
            width,
            height,
            offset_x: 0,
            offset_y: 0,
            canvas_width: width,
            canvas_height: height,
        };
        target
            .stroke_buffer
            .save_pre_stroke(device, &mut encoder, pipelines, &paint_target);
        queue.submit([encoder.finish()]);

        // Fresh uniform rings for the dab passes that follow.
        pipelines.reset_uniform_rings();

        // Fresh StrokeEngine every render — reusing the engine's own
        // `brush_stroke_engine` would contaminate save-points and dab-size
        // state with the user's in-flight real stroke.
        let spacing = SpacingConfig::default();
        let mut engine = StrokeEngine::new(runner, fg_color, spacing, Box::new(PassThrough::new()));

        // Pre-cooked points: pass them through a pass-through stabilizer so
        // `render_from_stabilized_range_to` walks them verbatim. No
        // smoothing, no lag — the S-curve is exactly what we handed in.
        for pt in path {
            let _ = engine.stabilize(*pt);
        }

        let sel_bg = pipelines.default_selection_bind_group();

        // `BrushGpuContext` borrows `dab_pool` mutably; each block below
        // creates a fresh context, runs one phase, and submits — the borrow
        // ends before the next block reborrows the pool.
        macro_rules! make_gpu_ctx {
            ($label:expr) => {{
                let (scratch, pre_stroke_texture, pre_stroke_bind_group) =
                    target.stroke_buffer.parts_for_brush_ctx();
                BrushGpuContext {
                    encoder: device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some($label),
                    }),
                    device,
                    queue,
                    dab_pool,
                    pipelines,
                    scratch: Some(scratch),
                    canvas_width: width,
                    canvas_height: height,
                    // Preview render target is canvas-aligned RGBA8.
                    paint_target: Some(paint_target),
                    selection_bind_group: sel_bg,
                    preview_target_view: None,
                    resource_handles,
                    blend_mode: 0,
                    preview_mask_view: None,
                    preview_mask_size: (0, 0),
                    brush_preview_info: None,
                    pre_stroke_texture: Some(pre_stroke_texture),
                    pre_stroke_bind_group: Some(pre_stroke_bind_group),
                    dab_write_canvas_bbox: None,
                }
            }};
        }

        // Terminal setup — color_output clears the scratch to transparent.
        {
            let mut ctx = make_gpu_ctx!("brush-preview-begin-stroke");
            engine.begin_stroke(&mut ctx);
            ctx.submit_final();
        }

        // Walk the full polyline placing dabs. `render_from_stabilized_range_to`
        // handles Catmull-Rom interpolation + sensor derivation internally.
        {
            let end = path.len() - 1;
            let mut ctx = make_gpu_ctx!("brush-preview-stroke");
            engine.render_from_stabilized_range_to(&mut ctx, 0, end);
            ctx.submit_final();
        }

        // Composite the scratch onto the pre-stroke snapshot and write
        // the result to the layer — same path as a real stroke's commit.
        {
            let mut ctx = make_gpu_ctx!("brush-preview-commit");
            engine.commit(&mut ctx);
            ctx.submit_final();
        }

        Some(&target.layer_texture)
    }

    /// Current target texture, if one is allocated.
    pub fn current_texture(&self) -> Option<&wgpu::Texture> {
        self.target.as_ref().map(|t| &t.layer_texture)
    }

    /// Current target dimensions, if one is allocated.
    pub fn current_size(&self) -> Option<(u32, u32)> {
        self.target.as_ref().map(|t| (t.width, t.height))
    }
}

impl Default for BrushPreviewRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Synthesize a single full-pressure dab at the centre of a target rect.
///
/// Drives the brush graph through the regular stroke pipeline with one
/// stationary sample — useful for the brush picker's tile-shape thumbnail
/// (and the BrushBar trigger button), where the user wants to see the
/// tip silhouette without a full stroke arc.
pub fn synthesize_preview_dab(width: f32, height: f32) -> Vec<PaintInformation> {
    vec![PaintInformation {
        pos: [width * 0.5, height * 0.5],
        pressure: 1.0,
        ..Default::default()
    }]
}

/// Synthesize an S-curve preview stroke of the given dimensions.
///
/// Samples `n_points` evenly along a cubic Bezier from lower-left to upper-
/// right. Pressure ramps 0 → 1 → 0.2 along the curve so users can see
/// pressure-driven dynamics (size taper, flow attenuation, etc.).
///
/// `inset` is the canvas-pixel margin reserved on every edge so an
/// endpoint dab of that radius fits inside the canvas. Caller is
/// responsible for passing a value < `min(width, height) / 2` — this
/// function does not clamp.
///
/// Shape follows Krita's `KisPresetLivePreviewView::setupAndPaintStroke`
/// — start low-left at pressure 0, end high-right at pressure 0.2, peak
/// pressure at the midpoint.
pub fn synthesize_preview_stroke(
    width: f32,
    height: f32,
    n_points: usize,
    inset: f32,
) -> Vec<PaintInformation> {
    let n = n_points.max(2);
    let lx = inset;
    let rx = width - inset;
    let ty = inset;
    let by = height - inset;
    let span_x = rx - lx;
    let span_y = by - ty;
    let p0 = [lx, ty + span_y * 0.7];
    let p1 = [lx + span_x * 0.30, ty + span_y * 0.10];
    let p2 = [lx + span_x * 0.70, ty + span_y * 0.90];
    let p3 = [rx, ty + span_y * 0.30];

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / (n - 1) as f32;
        let pos = cubic_bezier(p0, p1, p2, p3, t);
        let pressure = if t < 0.5 {
            // 0 → 1 over first half
            t * 2.0
        } else {
            // 1 → 0.2 over second half
            1.0 - (t - 0.5) * 1.6
        };
        out.push(PaintInformation {
            pos,
            pressure,
            // Half-second synthetic stroke so speed-sensitive nodes see a
            // non-zero dt between samples.
            time: t * 0.5,
            ..Default::default()
        });
    }
    out
}

fn cubic_bezier(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let w0 = u * u * u;
    let w1 = 3.0 * u * u * t;
    let w2 = 3.0 * u * t * t;
    let w3 = t * t * t;
    [
        w0 * p0[0] + w1 * p1[0] + w2 * p2[0] + w3 * p3[0],
        w0 * p0[1] + w1 * p1[1] + w2 * p2[1] + w3 * p3[1],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesized_stroke_bounds() {
        let inset = 16.0;
        let path = synthesize_preview_stroke(320.0, 120.0, 30, inset);
        assert_eq!(path.len(), 30);

        // Endpoints sit at the inset edge so an `inset`-radius dab fits.
        assert!((path[0].pos[0] - inset).abs() < 1e-3);
        assert!((path[29].pos[0] - (320.0 - inset)).abs() < 1e-3);

        // Pressure profile: 0 at start, ~1.0 at midpoint, 0.2 at end.
        assert!((path[0].pressure - 0.0).abs() < 1e-6);
        assert!((path[29].pressure - 0.2).abs() < 1e-3);
        let mid = path.len() / 2;
        assert!(path[mid].pressure > 0.9);

        for p in &path {
            assert!(p.pos[0] >= 0.0 && p.pos[0] <= 320.0);
            assert!(p.pos[1] >= 0.0 && p.pos[1] <= 120.0);
        }
    }

    #[test]
    fn synthesized_stroke_respects_min_points() {
        let path = synthesize_preview_stroke(100.0, 100.0, 1, 0.0);
        assert_eq!(path.len(), 2);
    }
}
