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

use super::gpu_context::{BrushGpuContext, BrushPerfCounters, DabBatch, StrokeResources};
use super::nodes::brush_settings;
use super::paint_info::PaintInformation;
use super::pipeline::BrushPipelines;
use super::stabilizer::PassThrough;
use super::stroke_buffer::StrokeBuffer;
use super::stroke_engine::StrokeEngine;
use super::wire::BrushWireType;
use crate::gpu::preview::PreviewBackdrop;
use crate::nodegraph::Graph;

/// Stroke seed every preview render uses.
///
/// A preview is a picture of a brush, not of one stroke of it: five shipped
/// brushes contain `random`/`noise` nodes, and seeding those from the clock
/// would make a cached thumbnail differ from its own re-bake and a
/// documentation asset differ from its own rebuild. The value is arbitrary; that
/// it never changes is the point.
const PREVIEW_STROKE_SEED: u32 = 0x5EED_B00C;

/// Reusable GPU scratch + layer textures for preview rendering.
struct PreviewTarget {
    width: u32,
    height: u32,
    /// Scratch format the cached `stroke_buffer` was built for. Part of
    /// the cache key: this renderer is reused across brushes, and a warp
    /// terminal's scratch holds a float field rather than colour, so a
    /// buffer cached for one is unbindable by the other.
    scratch_format: wgpu::TextureFormat,
    layer_texture: wgpu::Texture,
    layer_view: wgpu::TextureView,
    stroke_buffer: StrokeBuffer,
}

impl PreviewTarget {
    fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        pipelines: &BrushPipelines,
        scratch_format: wgpu::TextureFormat,
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
        let stroke_buffer = StrokeBuffer::new(device, width, height, pipelines, scratch_format);
        Self {
            width,
            height,
            scratch_format,
            layer_texture,
            layer_view,
            stroke_buffer,
        }
    }
}

/// Renders a full-stroke preview into an offscreen RGBA texture using the
/// real stroke engine. One instance is reusable across renders; it holds
/// onto its scratch target between calls and reallocates only on size change.
pub struct BrushStrokePreviewRenderer {
    target: Option<PreviewTarget>,
}

impl BrushStrokePreviewRenderer {
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
        pipelines: &BrushPipelines,
        graph: &Graph<BrushWireType>,
        path: &[PaintInformation],
        fg_color: [f32; 4],
        bg_color: [f32; 4],
        backdrop: PreviewBackdrop,
        width: u32,
        height: u32,
        base_size_override: Option<f32>,
    ) -> Option<&wgpu::Texture> {
        if path.is_empty() || width == 0 || height == 0 {
            return None;
        }
        // Fresh compile so callers can edit the graph between renders.
        let runner = super::compile_graph(graph).ok()?;

        // Ensure scratch + layer textures match the requested size *and*
        // the brush's scratch format — the cached target is shared across
        // brushes, so previewing a warp terminal after a colour one must
        // reallocate rather than bind a colour scratch to a field pipeline.
        let scratch_format = runner.scratch_format();
        let target_changed = match &self.target {
            Some(t) => t.width != width || t.height != height || t.scratch_format != scratch_format,
            None => true,
        };
        if target_changed {
            self.target = Some(PreviewTarget::new(
                device,
                width,
                height,
                pipelines,
                scratch_format,
            ));
        }
        let target = self.target.as_mut().unwrap();

        // Pre-fill the layer with the backdrop, then snapshot it as the
        // pre-stroke. `color_output::commit` composites the stroke scratch onto
        // this snapshot and writes the result back to the layer — so painting
        // the backdrop here is how it gets shown. It is also the only way one
        // reaches a terminal that *transports* the destination: those sample
        // `source_override.unwrap_or(pre_stroke_texture)` (`gpu_context.rs`),
        // and the preview captures no source snapshot, so the pre-stroke is
        // what they smear, warp, blur or clone.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("brush-preview-pre-fill"),
        });
        backdrop.fill(
            queue,
            &mut encoder,
            &target.layer_view,
            &target.layer_texture,
            (width, height),
            fg_color,
            bg_color,
        );
        let paint_target = crate::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
            &target.layer_texture,
            &target.layer_view,
            wgpu::TextureFormat::Rgba8Unorm,
            crate::coord::CanvasRect::from_xywh(0, 0, width, height),
        );
        target
            .stroke_buffer
            .save_pre_stroke(device, &mut encoder, pipelines, &paint_target);
        queue.submit([encoder.finish()]);

        // Fresh uniform rings for the dab passes that follow.
        pipelines.reset_uniform_rings();

        // Fresh StrokeEngine every render — reusing the engine's own
        // `brush_stroke_engine` would contaminate save-points and dab-size
        // state with the user's in-flight real stroke.
        //
        // Spacing comes from the graph's brush_settings node — same source the
        // real stroke uses — so scrubbing the spacing slider actually moves
        // the dabs in the preview.
        let spacing = brush_settings::spacing_config(graph);
        // Dab previews render at a fixed, larger canonical size than the brush's
        // own `brush_settings.size` so the rasterized tip carries enough detail
        // to survive the crop-and-downscale to the thumbnail. The stroke
        // preview passes `None` and keeps the graph-driven size.
        let base_size = base_size_override.unwrap_or_else(|| brush_settings::base_size(graph));

        // A brush that transports pixels from elsewhere has nowhere to
        // transport them from unless the preview says where. The offset comes
        // from the backdrop — the only thing that knows what displacement
        // escapes its own field — and the compiled graph says whether anything
        // will use it, so no node authors a coordinate and any future
        // source-sampling node gets a working preview for free.
        let clone_source_anchor = runner.samples_source().then(|| {
            let [du, dv] = backdrop.source_offset();
            [
                path[0].pos[0] + du * width as f32,
                path[0].pos[1] + dv * height as f32,
            ]
        });
        let mut engine = StrokeEngine::new(
            runner,
            fg_color,
            spacing,
            base_size,
            Box::new(PassThrough::new()),
            clone_source_anchor,
            PREVIEW_STROKE_SEED,
        );
        if clone_source_anchor.is_some() {
            // The snapshot being sampled is the pre-stroke, which covers the
            // whole preview target.
            engine.set_clone_source_frame(crate::coord::CanvasRect::from_xywh(0, 0, width, height));
        }

        // Pre-cooked points: pass them through a pass-through stabilizer so
        // `render_from_stabilized_range_to` walks them verbatim. No
        // smoothing, no lag — the S-curve is exactly what we handed in.
        for pt in path {
            let _ = engine.stabilize(*pt);
        }

        let sel_bg = pipelines.default_selection_bind_group();

        // Each block creates a fresh `BrushGpuContext`, runs one phase,
        // and submits.
        macro_rules! make_gpu_ctx {
            ($label:expr) => {{
                // The preview stroke buffer never captures a source
                // snapshot, so a source-sampling brush previews off the
                // pre-stroke snapshot — which is the backdrop, and is what
                // gives it something to transport.
                let (scratch, pre_stroke_texture, pre_stroke_bind_group, source_override) =
                    target.stroke_buffer.parts_for_brush_ctx();
                BrushGpuContext {
                    encoder: device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some($label),
                    }),
                    device,
                    queue,
                    pipelines,
                    selection_bind_group: sel_bg,
                    canvas_width: width,
                    canvas_height: height,
                    canvas_origin: [0, 0],
                    blend_mode: 0,
                    // Editor preview always renders at identity view; the
                    // S-curve preview shouldn't shift orientation when the
                    // user happens to rotate the canvas while editing.
                    view_rotation: 0.0,
                    perf: BrushPerfCounters::default(),
                    // Preview render target is canvas-aligned RGBA8.
                    stroke: Some(StrokeResources {
                        scratch,
                        paint_target,
                        pre_stroke_texture,
                        pre_stroke_bind_group,
                        source_override,
                    }),
                    preview: None,
                    dab_batch: DabBatch::default(),
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

impl Default for BrushStrokePreviewRenderer {
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
pub fn synthesize_dab_path(width: f32, height: f32) -> Vec<PaintInformation> {
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
pub fn synthesize_stroke_path(
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
        let path = synthesize_stroke_path(320.0, 120.0, 30, inset);
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
        let path = synthesize_stroke_path(100.0, 100.0, 1, 0.0);
        assert_eq!(path.len(), 2);
    }
}
