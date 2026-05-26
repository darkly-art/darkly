//! Liquify warp GPU terminal node.
//!
//! ## Stays on the dispatch path
//!
//! Liquify is one of the two builtin brushes (with [`smudge`]) that did
//! not migrate to the compiled-WGSL single-pass model. Its output is a
//! *displaced sample of the input canvas* — not a deposit on top — and
//! successive dabs compound by reading the prior dab's scratch state.
//! A single instanced render pass can't express that feedback loop, and
//! the warp shape doesn't fit `paint_compiled`'s "deposit premultiplied
//! RGBA" terminal contract. See `handoff-port-everything-to-compiled.md`
//! §"What might not compile" for the design discussion.
//!
//! A warp brush that pushes pixels along the pen's drawing direction with a
//! radial falloff around the brush center. Unlike paint terminals it does
//! not deposit pigment — every pixel inside the brush disc is replaced with
//! a sample from elsewhere in the scratch, displaced opposite the direction
//! of travel.
//!
//! Displacement *magnitude* is `strength × |pen.motion|` — the
//! cursor's per-dab travel scaled by strength. The Liquify brush
//! pins `pen_input.spacing_min_px` to a fixed pixel value, so
//! `|motion|` is size-invariant per dab and brush size controls only
//! the warped disc extent, not the intensity. A slow deliberate
//! drag produces the same per-dab displacement as a fast flick.
//! Speed still governs stroke length (dabs fire as the pen moves),
//! but not per-dab warp intensity.
//!
//! ## Stroke lifecycle
//!
//! - `begin_stroke` — copies `pre_stroke_texture` → `stroke_scratch_texture`
//!   (full canvas). The scratch starts matching the layer so warp dabs have
//!   realistic pixels to read. On rewind boundaries this reseeds the scratch
//!   to the stable pre-stroke state, discarding all accumulated warps; the
//!   checkpoint system then overlays the bbox snapshot for partial rewinds.
//! - `evaluate_gpu` (per dab) — `ensure_canvas_copy` captures the scratch's
//!   current warp state into `scratch read mirror`, then the liquify shader reads
//!   the copy at a displaced UV (`canvas_pos − direction × magnitude × falloff`)
//!   and writes the warped sample back into the scratch. Successive dabs
//!   read each other's output, so the warp compounds along the stroke.
//! - `commit` — `copy_texture_to_texture(scratch → layer)`, full canvas.
//!   The scratch already holds the finished warped canvas; no source-over.
//!   `gpu.blend_mode` is ignored — warping isn't paint.
//! - `render_preview` — synthesizes a soft circle mask sized to the brush
//!   radius using the existing circle pipeline, then blits it into the
//!   overlay's preview mask and publishes placement info. Liquify has no
//!   stamp upstream, so preview generation is internal.
//!
//! Compared to Krita's CPU-bound [`KisLiquifyTransformWorker`](krita/libs/image/kis_liquify_transform_worker.cpp#L442)
//! (per-pixel polygon rasterisation over a grid-of-points deformation),
//! this runs as one fragment pass per dab over the affected rect. Cost
//! scales with the brush footprint, not the canvas.
//!
//! The `softness` input is a waveshape knob (not an edge-softness slider):
//! 0 → spike (sharp peak), 0.4 → saw (linear), 0.5 → sine (smooth),
//! 1 → square (hard edge). See `liquify.wgsl` for the interpolation formula.

use std::any::Any;

use crate::brush::dab_pool::DAB_REFERENCE_SIZE;
use crate::brush::eval::{BrushNodeEvaluator, BrushPreviewInfo, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::nodes::circle::{CirclePipeline, CircleUniforms};
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipeline::{
    BlitUniforms, BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Pipeline ────────────────────────────────────────────────────────────

/// Uniform data for the liquify warp shader.
///
/// The shader samples `scratch read mirror` (a copy of the stroke scratch) at a
/// displaced UV inside a circular brush disc and writes the warped sample
/// back to the scratch. Everything is canvas-space; the shader converts to
/// UVs via `canvas_size` and `copy_origin`.
///
/// Per-dab displacement magnitude is decided on the CPU as
/// `strength × |pen.motion|` and passed as `displacement`; the shader
/// multiplies by a unit direction vector and the radial falloff.
/// Because the Liquify brush pins `pen_input.spacing_min_px`, the
/// per-dab `|motion|` is constant in canvas pixels regardless of
/// brush size. Pen speed never enters the equation — a slow drag
/// produces the same per-dab warp as a fast flick.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LiquifyUniforms {
    /// Top-left of the render-pass quad in canvas pixels (clamped to the
    /// **layer's** canvas extent so paste-extent / grown layers can warp
    /// off-canvas pixels).
    pub rect_origin: [f32; 2],
    /// Width and height of the render-pass quad in canvas pixels.
    pub rect_size: [f32; 2],
    /// Layer's canvas-space offset (= GpuPaintTarget.offset_x/y). Vertex
    /// stage subtracts this from canvas_pos before the NDC divide so the
    /// quad maps onto the layer-sized scratch render target correctly.
    pub target_offset: [f32; 2],
    /// Layer pixel dimensions (= GpuPaintTarget.width/height). Used by
    /// the vertex stage as the NDC denominator.
    pub target_size: [f32; 2],
    /// Document canvas dimensions (fragment-stage selection UV only —
    /// the selection texture is canvas-sized).
    pub canvas_size: [f32; 2],
    /// Layer-local origin of the scratch read mirror region (matches the
    /// `ensure_canvas_copy` source origin). The fragment shader floors
    /// this before dividing to recover the texel coordinate, same
    /// floor-then-ceil pattern as `composite.wgsl`.
    pub copy_origin: [f32; 2],
    /// Brush centre in canvas pixels.
    pub center: [f32; 2],
    /// Unit direction vector (cos θ, sin θ). Pixels sampled from
    /// `canvas_pos − direction × displacement × falloff`.
    pub direction: [f32; 2],
    /// Displacement magnitude in canvas pixels at the brush centre
    /// (where falloff = 1). Computed as `strength × |pen.motion|`.
    pub displacement: f32,
    /// Brush radius in canvas pixels.
    pub radius: f32,
    /// Waveshape knob (0–1). 0 = saw, 0.5 = sine, 1 = square.
    pub softness: f32,
    pub _pad: f32,
}

/// Liquify warp pipeline.  REPLACE blend — reads canvas_copy at a
/// displaced UV, writes the result straight into the scratch render
/// target.  No alpha blending; outside the disc the shader discards.
pub struct LiquifyPipeline {
    pipeline: wgpu::RenderPipeline,
    ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
}

impl LiquifyPipeline {
    fn build(ctx: &BuildContext) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("brush-liquify"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../../../shaders/brush/liquify.wgsl").into(),
                ),
            });
        // group(0) = uniforms, group(1) = selection mask,
        // group(2) = canvas copy (sampled at displaced UV — linear).
        let layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("brush-liquify-layout"),
                bind_group_layouts: &[ctx.uniform_bgl, ctx.selection_bgl, ctx.canvas_copy_bgl],
                immediate_size: 0,
            });
        let pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("brush-liquify"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            });
        let (ring, uniform_bind_group) = ctx.make_uniform_ring::<LiquifyUniforms>(
            "brush-liquify-uniforms",
            "brush-liquify-uniform-bg",
        );
        Self {
            pipeline,
            ring,
            uniform_bind_group,
        }
    }

    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    pub fn uniform_bind_group(&self) -> &wgpu::BindGroup {
        &self.uniform_bind_group
    }

    pub fn write_uniforms(&self, queue: &wgpu::Queue, uniforms: &LiquifyUniforms) -> u32 {
        self.ring.write(queue, bytemuck::bytes_of(uniforms))
    }
}

impl BrushPipelineEntry for LiquifyPipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        Some(&self.ring)
    }
}

fn liquify_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "liquify",
        build: |ctx| Box::new(LiquifyPipeline::build(ctx)),
    }
}

// ── Node ────────────────────────────────────────────────────────────────

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![liquify_pipeline_reg()],
        node: NodeRegistration {
        type_id: "liquify",
        category: "output",
        display_name: "Liquify",
        ports: vec![
            PortDef::input("size", BrushWireType::Scalar)
                .with_range(0.0, 4.0, 0.3)
                .with_natural_range(0.0, 4.0)
                .with_label("Size")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-up-right-and-down-left-from-center")
                .exposed()
                .with_description("Brush size. Can go above 100% for large-area warps (capped at 400%)."),
            PortDef::input("strength", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_natural_range(0.0, 1.0)
                .with_label("Strength")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-gauge-high")
                .exposed()
                .with_description("How far pixels are pushed by each brush touch"),
            PortDef::input("softness", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_natural_range(0.0, 1.0)
                .with_label("Softness")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-wave-square")
                .exposed()
                .with_description("Edge shape. Low values concentrate the warp at the brush center; high values spread it evenly across the brush."),
            PortDef::input("position", BrushWireType::Vec2)
                .with_description("Where to apply the warp"),
            // No `natural_range`: radians are a unit, not a normalized
            // signal. `pen.drawing_angle → direction` (the canonical
            // wire) is a unit-preserving identity.
            PortDef::input("direction", BrushWireType::Scalar)
                .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                .with_description("Direction to push pixels"),
            PortDef::input("distance", BrushWireType::Scalar)
                .with_description("How far the pen has traveled along the stroke"),
            // Per-dab cursor motion vector. Magnitude sets the per-dab
            // displacement scale (`strength × |motion|`). See
            // `liquify_compiled` for the lock/drag rationale.
            PortDef::input("motion", BrushWireType::Vec2)
                .with_description(
                    "Per-dab cursor motion vector. Magnitude sets \
                     the per-dab displacement scale.",
                ),
            PortDef::output("dab_size", BrushWireType::Vec2)
                .with_description("Size of the affected area"),
        ],
        params: &[],
        is_gpu: true,
        },
    }
}

pub struct LiquifyEvaluator;

impl BrushNodeEvaluator for LiquifyEvaluator {
    /// Liquify warps pixels; `gpu.blend_mode` (paint/erase) has no
    /// meaning here. The brush-tool UI hides the erase toggle.
    fn supports_erase(&self) -> bool {
        false
    }

    /// Publish `dab_size` so the stroke engine can space dabs along the
    /// path. `size = 1.0` maps to diameter `DAB_REFERENCE_SIZE`; larger values
    /// allow brushes wider than that for full-canvas warp effects.
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let size = ctx.input_f32("size").clamp(0.0, 4.0);
        let diameter = (size * DAB_REFERENCE_SIZE as f32).max(1.0);
        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    /// Per-dab warp: read the scratch's current state (via canvas_copy) at
    /// a displaced UV, write the warped sample into the scratch's disc
    /// region. Outside the disc the shader discards, preserving LoadOp::Load
    /// content.
    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let size = ctx.input_f32("size").clamp(0.0, 4.0);
        let strength = ctx.input_f32("strength").clamp(0.0, 1.0);
        let softness = ctx.input_f32("softness").clamp(0.0, 1.0);
        let position = ctx.input("position").as_vec2();
        let direction = ctx.input_f32("direction");
        let distance = ctx.input_f32("distance");
        let motion = ctx.input("motion").as_vec2();
        let motion_mag = (motion[0] * motion[0] + motion[1] * motion[1]).sqrt();

        let radius = size * (DAB_REFERENCE_SIZE as f32) * 0.5;
        if radius < 1.0 {
            return vec![];
        }

        if strength < 1e-4 {
            return vec![];
        }

        // Gate: skip dabs that have no direction of travel yet. The stroke
        // engine's first dab fires before any motion is recorded (pen-down
        // moment) — drawing_angle defaults to 0 (east), so without this gate
        // a stationary click would immediately warp rightward. `distance`
        // stays at zero until the pen actually moves, so it's the cleanest
        // "stroke has a direction" signal.
        if distance < 0.5 {
            return vec![];
        }

        // Per-dab displacement magnitude = `strength × |motion|`. The
        // brush pins `pen_input.spacing_min_px` to a fixed pixel
        // amount, so |motion| is size-invariant per dab. At
        // `strength = 1` per-dab push equals per-dab cursor motion,
        // locking pixels to the cursor; at fractional strength the
        // pixel lags by that fraction. Size controls the disc extent,
        // not the intensity.
        let displacement = motion_mag * strength;

        // Layer-clip the dab footprint, push the canvas-space write bbox,
        // and snapshot the scratch under the disc into canvas_copy.
        // Subsequent dabs in the same place see the prior dab's warp via
        // the cached snapshot. `half` includes displacement padding so
        // the bilinear-sampled canvas_copy footprint always lies inside
        // the copied region. None means the disc doesn't overlap the
        // layer (early-out).
        let half = radius + displacement;
        let footprint = match gpu.prepare_dab_canvas_copy(position, half, half) {
            Some(f) => f,
            None => return vec![],
        };
        let [pt_offset_x, pt_offset_y] = footprint.layer_offset;
        let [pt_width, pt_height] = footprint.layer_size;
        let [x0, y0] = footprint.origin;
        let [rect_w, rect_h] = footprint.size;
        let [copy_local_x, copy_local_y] = footprint.copy_local_origin;

        // Direction → unit vector. First dab of a stroke has no prior
        // position and arrives here with `direction = 0` (east). Acceptable
        // at stroke onset; subsequent dabs use the actual direction.
        let dir_vec = [direction.cos(), direction.sin()];

        let uniforms = LiquifyUniforms {
            rect_origin: [x0, y0],
            rect_size: [rect_w, rect_h],
            target_offset: [pt_offset_x as f32, pt_offset_y as f32],
            target_size: [pt_width as f32, pt_height as f32],
            canvas_size: [gpu.canvas_width as f32, gpu.canvas_height as f32],
            copy_origin: [copy_local_x as f32, copy_local_y as f32],
            center: position,
            direction: dir_vec,
            displacement,
            radius,
            softness,
            _pad: 0.0,
        };
        let liq = gpu.pipelines.get::<LiquifyPipeline>("liquify");
        let offset = liq.write_uniforms(gpu.queue, &uniforms);
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("liquify::evaluate_gpu requires Scratch");

        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-liquify"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scratch.write_view(),
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            // Viewport must be the full layer (not the canvas) so the
            // shader can write into off-canvas pixels of paste-extent /
            // grown layers.
            pass.set_viewport(0.0, 0.0, pt_width as f32, pt_height as f32, 0.0, 1.0);
            pass.set_pipeline(liq.pipeline());
            pass.set_bind_group(0, liq.uniform_bind_group(), &[offset]);
            pass.set_bind_group(1, gpu.selection_bind_group, &[]);
            pass.set_bind_group(2, scratch.read_mirror_bind_group(), &[]);
            pass.draw(0..6, 0..1);
        }

        // dab_size is CPU-computed in evaluate_cpu; the slot is already set.
        // Returning empty avoids a redundant slot write during GPU eval.
        vec![]
    }

    /// Seed the stroke scratch with the immutable pre-stroke layer snapshot.
    /// Every rewind (full or partial-preamble) runs through this hook, so
    /// the scratch always restarts from the same pre-stroke state regardless
    /// of prior commits. Reading `layer_texture` instead would compound
    /// warps exponentially on each rewind.
    fn begin_stroke(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let Some(pre_stroke) = gpu.pre_stroke_texture else {
            return;
        };
        let Some(scratch) = gpu.scratch.as_deref() else {
            return;
        };
        // Copy the full scratch — paste-extent or off-canvas-grown layers
        // size pre_stroke and scratch beyond canvas dims; copying only
        // canvas-sized would leave the off-canvas strip uninitialised,
        // and `commit_scratch_blit` would blit transparent-black back
        // over the layer.
        let scratch_tex = scratch.write_texture();
        let w = scratch_tex.width();
        let h = scratch_tex.height();
        gpu.encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: pre_stroke,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: scratch_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Replace the layer with the warped scratch. Straight copy, no blend —
    /// the scratch already holds the finished image because warp dabs
    /// produced the full canvas state (pre-stroke + displacement) in place.
    /// `commit_scratch_blit` on the paint target does the format-aware path:
    /// hardware copy for RGBA8 layer destinations, render pass for R8 mask
    /// destinations.
    fn commit(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let Some(paint_target) = gpu.paint_target.as_ref() else {
            return;
        };
        let Some(scratch) = gpu.scratch.as_deref() else {
            return;
        };
        paint_target.commit_scratch_blit(
            gpu.device,
            &mut gpu.encoder,
            gpu.pipelines,
            scratch.write_view(),
            scratch.write_texture(),
        );
    }

    /// Generate a soft circle preview at the brush's canvas-pixel size and
    /// blit it into the overlay preview mask. Reuses the existing circle
    /// pipeline — no liquify-specific preview shader needed.
    fn render_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let Some(target_view) = gpu.preview_mask_view else {
            return vec![];
        };
        let (target_w, target_h) = gpu.preview_mask_size;
        if target_w == 0 || target_h == 0 {
            return vec![];
        }

        let size = ctx.input_f32("size").clamp(0.0, 4.0);
        // Soft edge for the preview ring — independent of the `softness`
        // waveshape knob (which controls warp falloff, not visual hardness).
        let preview_softness = 0.3_f32;
        let diameter_px = ((size * DAB_REFERENCE_SIZE as f32).max(4.0)) as u32;

        // Render the circle mask into a dab pool texture sized to the brush
        // extent. `acquire_sized` makes the texture self-describe its size,
        // so we don't need a separate size-reporting channel.
        let handle = gpu
            .dab_pool
            .acquire_sized(gpu.device, diameter_px, diameter_px);
        let circle_view = gpu.dab_pool.view(handle);
        // Liquify's preview ring is a plain hard-edged disc — algorithm = 0
        // (sine harmonic) with amplitude = 0 produces the unmodulated unit
        // circle, and the centroid lands at the texture centre by construction.
        let circle_uniforms = CircleUniforms {
            softness: preview_softness,
            algorithm: 0,
            amplitude: 0.0,
            frequency: 1.0,
            phase: 0.0,
            persistence: 0.5,
            seed: 0.0,
            octaves: 1,
            n1: 1.0,
            n2: 1.0,
            n3: 1.0,
            base_radius: 0.498,
            centroid_x: 0.0,
            centroid_y: 0.0,
            _pad: [0.0; 2],
        };
        let circle = gpu.pipelines.get::<CirclePipeline>("circle");
        let circle_offset = circle.write_uniforms(gpu.queue, &circle_uniforms);
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquify-preview-circle"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: circle_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(0.0, 0.0, diameter_px as f32, diameter_px as f32, 0.0, 1.0);
            pass.set_pipeline(circle.pipeline());
            pass.set_bind_group(0, circle.uniform_bind_group(), &[circle_offset]);
            pass.draw(0..3, 0..1);
        }

        // Blit the circle into the overlay preview mask (fixed-size texture
        // the overlay samples for display).
        let blit_uniforms = BlitUniforms {
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
        };
        let blit_offset = gpu.pipelines.write_blit_uniforms(gpu.queue, &blit_uniforms);
        let circle_bg = gpu.dab_pool.bind_group(handle).clone();
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquify-preview-blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(0.0, 0.0, target_w as f32, target_h as f32, 0.0, 1.0);
            pass.set_pipeline(gpu.pipelines.blit_pipeline());
            pass.set_bind_group(0, &gpu.pipelines.blit_uniform_bind_group, &[blit_offset]);
            pass.set_bind_group(1, &circle_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        // Publish placement info — canvas-pixel half-extent for the overlay
        // primitive. First terminal to publish wins (there's only one here).
        if gpu.brush_preview_info.is_none() {
            let half = diameter_px as f32 * 0.5;
            gpu.brush_preview_info = Some(BrushPreviewInfo {
                half_extent_canvas_px: [half, half],
                rotation_rad: 0.0,
            });
        }

        vec![]
    }
}
