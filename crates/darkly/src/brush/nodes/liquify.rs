//! Liquify terminal — per-dab fragment-pass warp with a per-brush
//! compiled WGSL shader.
//!
//! Same overall outline as [`smudge`](super::smudge).
//! Per dab the fragment shader samples the scratch read mirror at a
//! *displaced* UV inside a circular brush disc and writes the warped
//! sample back into the scratch. Successive dabs compound because each
//! reads the cumulatively-warped scratch — the per-dab serialization
//! is semantically required, not a perf bug.
//!
//! Displacement magnitude is `strength × |pen.motion|` — the cursor's
//! per-dab travel scaled by strength. With the Liquify brush's fixed
//! `pen_input.spacing_min_px = LIQUIFY_SPACING_PX`, `|motion|` is the
//! same constant at any brush size, so:
//!   * `strength = 1` locks pixels to the cursor (per-dab push =
//!     per-dab cursor motion);
//!   * `strength < 1` produces a strength-fraction drag;
//!   * brush size controls only the warped *extent* (the disc), never
//!     the *intensity*.
//!
//! Pen speed enters only via dab density along the path; the per-dab
//! push is identical for slow and fast drags.
//!
//! ## Stroke lifecycle
//!
//! - `begin_stroke` — copies `pre_stroke_texture` → scratch so warps
//!   start against a stable layer snapshot.
//! - `evaluate_gpu` (per dab) — queues a record + meta; skipped dabs
//!   (`radius < 1`, `strength < 1e-4`, `distance < 0.5`) never reach
//!   the queue, so the flush loop doesn't iterate them.
//! - `flush_dabs` — for each queued dab, `prepare_dab_canvas_copy`
//!   syncs the read mirror over a symmetric `radius + displacement`
//!   half-extent (bilinear sampler reaches into the padding); then
//!   one render pass with instance index `i..i+1`.
//! - `commit` — `commit_scratch_blit(scratch → layer)`; `blend_mode`
//!   ignored — warping isn't paint.
//!
//! ## Softness waveshape
//!
//! User-facing slider: `0 = hard` (uniform displacement across the
//! disc, square edge) ↔ `1 = soft` (sharp peak at the brush centre,
//! near-zero past the half-radius — only the cursor itself drags
//! pixels). Internally the falloff helper takes the *opposite*
//! convention (`0 = spike → 1 = square`), and the WGSL body inverts
//! the slider value before passing it in. The mapping the user sees:
//!   0    → uniform / square     (helper input `1.0`)
//!   0.5  → sine                  (helper input `0.5`)
//!   0.6  → linear saw            (helper input `0.4`)
//!   1    → spike (`pow(1-d, 8)`) (helper input `0.0`)

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::{BrushGpuContext, MAX_DABS_PER_PHASE};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipeline::{
    BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};
use crate::brush::wgsl::{
    pack_intrinsic_uniforms, pack_uniforms, CompileWgslCtx, CompiledBrush, DabField,
    IntrinsicUniforms, NodeWgsl, WgslType, INTRINSIC_UNIFORMS_SIZE,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

const SIZE_REFERENCE_PX: f32 = crate::brush::DAB_REFERENCE_SIZE as f32;
const MAX_UNIFORM_BYTES: usize = 1024;

/// Dab spacing for the Liquify brush, in canvas pixels. The brush
/// pins `pen_input.spacing_min_px` to this value (and sets ratio to
/// zero) so spacing stays fixed at any brush size. Per-dab
/// displacement is then `strength × |pen.motion| ≈ strength ×
/// LIQUIFY_SPACING_PX`, which makes:
///   * `strength = 1` lock pixels to the cursor (per-dab push equals
///     per-dab cursor motion);
///   * `strength = 0.5` lag the cursor by 50% (the "drag" feel);
///   * the absolute pixel push size-invariant — the size slider
///     controls the warped *extent* (the disc), not the *intensity*.
///
/// Tuned to 4 px: tight enough for smooth-looking warps without dab
/// banding, large enough not to blow up the dab count at huge
/// brushes (perf scales with `diameter / spacing`).
pub const LIQUIFY_SPACING_PX: f32 = 4.0;

/// Per-dab strength below which the dab is dropped — `mix(orig, warped, _·sel)`
/// collapses to identity and the per-dab pass would be a no-op.
const STRENGTH_EPSILON: f32 = 1.0e-4;

/// Brush radius below which the dab is dropped — sub-pixel discs warp
/// nothing visible.
const MIN_RADIUS_PX: f32 = 1.0;

/// Cumulative stroke distance below which liquify silently skips the
/// first dab. Without this, a stationary click would warp rightward
/// (default `drawing_angle = 0`).
const MIN_DISTANCE_PX: f32 = 0.5;

// ── Per-dab CPU meta ────────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LiquifyDabMeta {
    position: [f32; 2],
    /// Symmetric half-extent of the read region (`radius + displacement`).
    half: [f32; 2],
}

const LIQUIFY_DAB_META_SIZE: usize = std::mem::size_of::<LiquifyDabMeta>();

// ── Per-brush pipeline ──────────────────────────────────────────────────

struct PerBrushPipeline {
    pipeline: wgpu::RenderPipeline,
    uniform_ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
    dabs_buffer: wgpu::Buffer,
    dabs_bind_group: wgpu::BindGroup,
    uniform_size: usize,
}

impl PerBrushPipeline {
    fn build(ctx: &BuildContext, compiled: &CompiledBrush) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("liquify-brush"),
                source: wgpu::ShaderSource::Wgsl(compiled.stroke_wgsl.clone().into()),
            });

        let dabs_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("liquify-dabs-bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("liquify-layout"),
                bind_group_layouts: &[
                    ctx.uniform_bgl,
                    &dabs_bgl,
                    ctx.selection_bgl,
                    ctx.canvas_copy_bgl,
                ],
                immediate_size: 0,
            });

        let pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("liquify"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
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
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        let uniform_size =
            (INTRINSIC_UNIFORMS_SIZE + compiled.uniform_size).max(INTRINSIC_UNIFORMS_SIZE);
        let uniform_ring = DynamicUniformRing::new(
            ctx.device,
            "liquify-uniforms",
            uniform_size as u64,
            ctx.min_uniform_align,
        );
        let uniform_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("liquify-uniform-bg"),
            layout: ctx.uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_ring.buffer,
                    offset: 0,
                    size: Some(uniform_ring.binding_size()),
                }),
            }],
        });

        let dab_record_size = compiled.dab_record_size.max(16);
        let dabs_buffer_size = (MAX_DABS_PER_PHASE as u64) * (dab_record_size as u64);
        let dabs_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("liquify-dabs-buffer"),
            size: dabs_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dabs_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("liquify-dabs-bg"),
            layout: &dabs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dabs_buffer.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            uniform_ring,
            uniform_bind_group,
            dabs_buffer,
            dabs_bind_group,
            uniform_size,
        }
    }
}

// ── Pipeline registry entry ─────────────────────────────────────────────

pub struct LiquifyPipeline {
    cache: RefCell<HashMap<u64, PerBrushPipeline>>,
}

impl LiquifyPipeline {
    fn build(_ctx: &BuildContext) -> Self {
        Self {
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn ensure_pipeline(&self, ctx: &BuildContext, compiled: &CompiledBrush) {
        let mut cache = self.cache.borrow_mut();
        cache
            .entry(compiled.topology_hash)
            .or_insert_with(|| PerBrushPipeline::build(ctx, compiled));
    }

    fn with_pipeline<R>(&self, hash: u64, f: impl FnOnce(&PerBrushPipeline) -> R) -> R {
        let cache = self.cache.borrow();
        let p = cache
            .get(&hash)
            .expect("ensure_pipeline must run before with_pipeline");
        f(p)
    }
}

impl BrushPipelineEntry for LiquifyPipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        None
    }
    fn rings(&self) -> Vec<&DynamicUniformRing> {
        Vec::new()
    }
}

fn liquify_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "liquify",
        build: |ctx| Box::new(LiquifyPipeline::build(ctx)),
    }
}

// ── Node ────────────────────────────────────────────────────────────────

pub const TYPE_ID: &str = "liquify";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![liquify_pipeline_reg()],
        evaluator: || Box::new(LiquifyEvaluator),
        lifecycle: crate::brush::node::Lifecycle::SeedScratchFromPreStroke,
        node: NodeRegistration {
            type_id: TYPE_ID,
            category: "output",
            display_name: "Liquify",
            ports: vec![
                PortDef::input("position", BrushWireType::Vec2)
                    .with_description("Where to apply the warp"),
                // No `natural_range`: radians are a unit, not a normalized
                // signal. `pen.drawing_angle → direction` (canonical wire)
                // is a unit-preserving identity.
                PortDef::input("direction", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                    .with_description("Direction to push pixels"),
                PortDef::input("distance", BrushWireType::Scalar)
                    .with_description("How far the pen has traveled along the stroke"),
                // Per-dab cursor motion in canvas pixels. Wire from
                // `pen.motion`; the magnitude becomes the per-dab
                // displacement scale (`strength × |motion|`) so 100%
                // strength locks pixels to the cursor and 50% lets
                // them drag half-step behind, regardless of brush
                // size. When unwired, defaults to (0, 0) → no warp.
                PortDef::input("motion", BrushWireType::Vec2)
                    .with_description(
                        "Per-dab cursor motion vector. Magnitude sets \
                         the per-dab displacement scale.",
                    ),
                PortDef::input("size_input", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Size Input")
                    .with_unit(UnitType::Percent)
                    .with_description(
                        "Per-touch size multiplier (wire pressure here for pressure-sensitive size).",
                    ),
                PortDef::input("size", BrushWireType::Scalar)
                    .with_range(0.0, 4.0, 0.3)
                    .with_label("Size")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-up-right-and-down-left-from-center")
                    .exposed()
                    .with_preview_value(0.1)
                    .with_description(
                        "Brush size. Can go above 100% for large-area warps (capped at 400%).",
                    ),
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
                    .with_description(
                        "Edge shape. Low values concentrate the warp at the brush center; \
                         high values spread it evenly across the brush.",
                    ),
                // Optional brush-shape modulation. If wired, the warp
                // strength multiplies by the upstream coverage. If
                // unwired, defaults to 1.0 (uniform inside the disc).
                PortDef::input("mask", BrushWireType::Texture).with_description(
                    "Per-fragment shape coverage (typically wired from circle.texture); \
                     defaults to 1.0 (uniform inside the disc) when unwired.",
                ),
                PortDef::output("dab_size", BrushWireType::Vec2)
                    .with_description("Size of the affected area"),
            ],
            params: &[],
            is_gpu: true,
            is_terminal: true,
            supports_erase: false,
        },
    }
}

pub struct LiquifyEvaluator;

impl LiquifyEvaluator {
    fn effective_radius(ctx: &EvalContext) -> f32 {
        let size_input = ctx.input_f32("size_input").max(0.0);
        let size = ctx.input_f32("size").max(0.0);
        let effective_size = size_input * size;
        (effective_size * SIZE_REFERENCE_PX * 0.5).max(0.5)
    }

    fn insert_copy_origin(gpu: &mut BrushGpuContext, node_id: u32, value: [f32; 2]) {
        if let Some(outputs) = gpu.slot_outputs_owned.as_mut() {
            outputs.insert(
                format!("n{}_copy_origin", node_id),
                ScalarValue::Vec2(value),
            );
        }
    }
}

impl BrushNodeEvaluator for LiquifyEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let Some(compiled) = gpu.compiled_brush.clone() else {
            debug_assert!(false, "liquify requires compiled_brush on gpu_context");
            return vec![];
        };
        let Some(paint_target) = gpu.paint_target.as_ref() else {
            return vec![];
        };
        let position = ctx.input("position").as_vec2();
        let strength = ctx.input_f32("strength").clamp(0.0, 1.0);
        let distance = ctx.input_f32("distance");
        let motion = ctx.input("motion").as_vec2();
        let motion_mag = (motion[0] * motion[0] + motion[1] * motion[1]).sqrt();
        let radius = Self::effective_radius(ctx);
        let diameter = radius * 2.0;

        // Three early-outs — skip stationary or sub-pixel dabs whose
        // warp would be a no-op.
        if radius < MIN_RADIUS_PX {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        }
        if strength < STRENGTH_EPSILON {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        }
        if distance < MIN_DISTANCE_PX {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        }

        // Per-dab displacement magnitude — the cursor's per-dab
        // motion scaled by strength. `strength = 1` pushes pixels by
        // the full cursor step (lock), `strength = 0.5` pushes them
        // by half (drag), regardless of brush size. CPU and shader
        // compute the same value (the shader reads motion + strength
        // from the dab record).
        let displacement = motion_mag * strength;

        let bbox_radius = radius * compiled.brush_extent_factor + compiled.brush_extent_extra_px;
        let canvas_ext = paint_target.canvas_extent();
        let layer_x0 = canvas_ext.x0() as f32;
        let layer_y0 = canvas_ext.y0() as f32;
        let layer_x1 = layer_x0 + canvas_ext.width as f32;
        let layer_y1 = layer_y0 + canvas_ext.height as f32;
        let cx0 = (position[0] - bbox_radius).max(layer_x0);
        let cy0 = (position[1] - bbox_radius).max(layer_y0);
        let cx1 = (position[0] + bbox_radius).min(layer_x1);
        let cy1 = (position[1] + bbox_radius).min(layer_y1);
        if cx1 <= cx0 || cy1 <= cy0 {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        }
        let bbox_x = cx0.floor() as i32;
        let bbox_y = cy0.floor() as i32;
        let bbox_w = (cx1.ceil() as i32 - bbox_x) as u32;
        let bbox_h = (cy1.ceil() as i32 - bbox_y) as u32;
        gpu.push_dab_write_bbox(crate::coord::CanvasRect::from_xywh(
            bbox_x, bbox_y, bbox_w, bbox_h,
        ));
        let layer_w = canvas_ext.width;
        let layer_h = canvas_ext.height;
        let local_x0 = (bbox_x - canvas_ext.x0()).max(0) as u32;
        let local_y0 = (bbox_y - canvas_ext.y0()).max(0) as u32;
        let local_x1 = (local_x0 + bbox_w).min(layer_w);
        let local_y1 = (local_y0 + bbox_h).min(layer_h);
        gpu.pending_dabs_bbox = Some(match gpu.pending_dabs_bbox {
            Some([x0, y0, x1, y1]) => [
                x0.min(local_x0),
                y0.min(local_y0),
                x1.max(local_x1),
                y1.max(local_y1),
            ],
            None => [local_x0, local_y0, local_x1, local_y1],
        });

        // Symmetric read region — disc inflated by `displacement` per
        // axis so the warped sample at
        // `target_pos - direction × displacement × falloff(d)` always
        // lies inside the mirror snapshot (bilinear sampler reaches
        // into the inflation margin too).
        let read_half = radius + displacement;

        let read_x0 = (position[0] - read_half).max(layer_x0);
        let read_y0 = (position[1] - read_half).max(layer_y0);
        let copy_canvas_x = read_x0.floor();
        let copy_canvas_y = read_y0.floor();
        Self::insert_copy_origin(gpu, ctx.node_id.0 as u32, [copy_canvas_x, copy_canvas_y]);

        gpu.queue_dab(&compiled, position, bbox_radius, radius);

        let meta = LiquifyDabMeta {
            position,
            half: [read_half, read_half],
        };
        gpu.pending_dab_meta_bytes
            .extend_from_slice(bytemuck::bytes_of(&meta));

        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    fn flush_dabs(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.pending_dab_count == 0 {
            return;
        }
        let Some(compiled) = gpu.compiled_brush.clone() else {
            debug_assert!(false, "liquify::flush_dabs requires compiled_brush");
            return;
        };

        let bbox = gpu.pending_dabs_bbox.unwrap_or([0, 0, 0, 0]);
        let union_w = bbox[2].saturating_sub(bbox[0]);
        let union_h = bbox[3].saturating_sub(bbox[1]);
        let (dab_bytes, total_dabs) = gpu.take_pending_dabs();
        let meta_bytes = gpu.take_pending_dab_meta();
        if total_dabs == 0 {
            return;
        }
        debug_assert_eq!(
            meta_bytes.len(),
            (total_dabs as usize) * LIQUIFY_DAB_META_SIZE,
            "liquify meta queue out of sync with dab queue"
        );
        let metas: Vec<LiquifyDabMeta> = bytemuck::cast_slice(&meta_bytes).to_vec();
        gpu.perf
            .record_dab_flush_workload(total_dabs, union_w, union_h);

        let pipeline_ref = gpu.pipelines.get::<LiquifyPipeline>("liquify");
        ensure_per_brush_pipeline(gpu, pipeline_ref, &compiled);

        let paint_target = gpu
            .paint_target
            .as_ref()
            .expect("liquify::flush_dabs requires paint_target");
        let canvas_ext = paint_target.canvas_extent();
        let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];
        let layer_size = [canvas_ext.width, canvas_ext.height];

        let mut uniform_bytes: Vec<u8> = Vec::with_capacity(MAX_UNIFORM_BYTES);
        pack_intrinsic_uniforms(
            &mut uniform_bytes,
            IntrinsicUniforms {
                layer_offset,
                layer_size,
                canvas_size: [gpu.canvas_width, gpu.canvas_height],
                preview_centre: [0.0, 0.0],
                preview_size: [0, 0],
                _pad: [0, 0],
            },
        );
        let outputs = gpu
            .slot_outputs_owned
            .as_ref()
            .expect("liquify::flush_dabs requires slot_outputs_owned");
        pack_uniforms(&compiled, outputs, &mut uniform_bytes);

        pipeline_ref.with_pipeline(compiled.topology_hash, |per_brush| {
            if uniform_bytes.len() < per_brush.uniform_size {
                uniform_bytes.resize(per_brush.uniform_size, 0);
            }
            per_brush.uniform_ring.reset();
            let uniform_offset = per_brush.uniform_ring.write(gpu.queue, &uniform_bytes);
            gpu.queue
                .write_buffer(&per_brush.dabs_buffer, 0, &dab_bytes);

            for (i, meta) in metas.iter().enumerate() {
                let _ = gpu.prepare_dab_canvas_copy(
                    meta.position,
                    meta.half[0],
                    meta.half[1],
                    meta.half[0],
                    meta.half[1],
                );

                let scratch_ref = gpu
                    .scratch
                    .as_deref()
                    .expect("liquify::flush_dabs requires Scratch");
                let read_bg = scratch_ref.read_mirror_bind_group();
                let write_view = scratch_ref.write_view();

                let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("liquify-flush"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: write_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_viewport(
                    0.0,
                    0.0,
                    layer_size[0] as f32,
                    layer_size[1] as f32,
                    0.0,
                    1.0,
                );
                pass.set_pipeline(&per_brush.pipeline);
                pass.set_bind_group(0, &per_brush.uniform_bind_group, &[uniform_offset]);
                pass.set_bind_group(1, &per_brush.dabs_bind_group, &[]);
                pass.set_bind_group(2, gpu.selection_bind_group, &[]);
                pass.set_bind_group(3, read_bg, &[]);
                let ii = i as u32;
                pass.draw(0..6, ii..ii + 1);
            }
        });

        gpu.perf.record_dab_flush(total_dabs);
    }

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

    /// Hover-cursor preview. Routes through the shared preview helper.
    /// Liquify's preview is a soft disc with the same softness
    /// falloff the stroke applies — scrubbing the softness slider
    /// visibly reshapes the cursor. Rotation is 0 (the preview is
    /// radially symmetric).
    fn render_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let radius = Self::effective_radius(ctx);
        let _ = crate::brush::wgsl::render_compiled_preview(gpu, radius, 0.0);
        vec![]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();

        // `mask` defaults to 1.0 when unwired — uniform warp inside
        // the disc.
        let mask_expr = if cctx.input_is_wired("mask") {
            cctx.input("mask").as_f32()
        } else {
            "1.0".to_string()
        };
        let strength_expr = cctx.input("strength").as_f32();
        let softness_expr = cctx.input("softness").as_f32();
        let direction_expr = cctx.input("direction").as_f32();
        let motion_expr = cctx.input("motion").as_vec2();

        let copy_origin_field = cctx.dab_field_name("copy_origin");
        let key = copy_origin_field.clone();
        wgsl.dab_fields.push(DabField {
            name: copy_origin_field.clone(),
            ty: WgslType::Vec2,
            pack: Arc::new(move |outputs, bytes| {
                let v = outputs.get(&key).map(|s| s.as_vec2()).unwrap_or([0.0; 2]);
                bytes.extend_from_slice(bytemuck::bytes_of(&v));
            }),
        });

        wgsl.terminal_bindings = "@group(3) @binding(0) var scratch_mirror_tex: texture_2d<f32>;\n\
             @group(3) @binding(1) var scratch_mirror_smp: sampler;\n"
            .to_string();

        // Per-node falloff fn — suffixed by node id so two liquify
        // terminals (hypothetical) in the same brush don't collide.
        let falloff_fn = cctx.ident("liquify_falloff");
        wgsl.decls = format!(
            "fn {falloff_fn}(d_norm: f32, softness: f32) -> f32 {{\n\
             \x20   let saw  = 1.0 - d_norm;\n\
             \x20   let sine = 0.5 + 0.5 * cos(3.14159265 * d_norm);\n\
             \x20   let saw_break  = 0.4;\n\
             \x20   let sine_break = 0.5;\n\
             \x20   let k_max      = 8.0;\n\
             \x20   if softness <= saw_break {{\n\
             \x20       let t = softness / saw_break;\n\
             \x20       let k = mix(k_max, 1.0, t);\n\
             \x20       return pow(max(saw, 0.0), k);\n\
             \x20   }} else if softness <= sine_break {{\n\
             \x20       let t = (softness - saw_break) / (sine_break - saw_break);\n\
             \x20       return mix(saw, sine, t);\n\
             \x20   }} else {{\n\
             \x20       let t = (softness - sine_break) / (1.0 - sine_break);\n\
             \x20       return mix(sine, 1.0, t);\n\
             \x20   }}\n\
             }}\n"
        );

        // Fragment body: `local_dist` and `target_pos` come from the
        // framework wrapper; the framework already discards past
        // `d.bbox_target_px`. We additionally discard past
        // `local_dist >= 1.0` so the warp stays inside the disc (the
        // framework's discard kicks in for `bbox_target_px < radius`
        // cases too, but when extent contribution lands at 1.0 the
        // two conditions coincide).
        // The falloff helper takes `0 = spike` / `1 = square`. The
        // user-facing slider is labelled "Softness" with the opposite
        // intuition — `1 = soft / feathery` (only the brush centre
        // pushes, edges fade to nothing), `0 = hard / sharp` (uniform
        // displacement, square step at the disc edge). Invert before
        // passing to the helper so the slider matches the label.
        wgsl.body = format!(
            "    if (local_dist >= 1.0) {{ discard; }}\n\
             \x20   let warp_mask = clamp({mask_expr}, 0.0, 1.0);\n\
             \x20   let strength = clamp({strength_expr}, 0.0, 1.0);\n\
             \x20   let softness = clamp({softness_expr}, 0.0, 1.0);\n\
             \x20   let falloff_param = 1.0 - softness;\n\
             \x20   let direction_angle = {direction_expr};\n\
             \x20   let motion_vec = {motion_expr};\n\
             \x20   let f = {falloff_fn}(local_dist, falloff_param);\n\
             \x20   let dir = vec2<f32>(cos(direction_angle), sin(direction_angle));\n\
             \x20   let displacement = length(motion_vec) * strength;\n\
             \x20   let source_pos = target_pos - dir * displacement * f;\n\
             \x20   let mirror_dims = vec2<f32>(textureDimensions(scratch_mirror_tex));\n\
             \x20   let copy_uv    = (source_pos - d.{copy_origin_field}) / mirror_dims;\n\
             \x20   let warped     = textureSampleLevel(scratch_mirror_tex, scratch_mirror_smp, copy_uv,    0.0);\n\
             \x20   let original_uv = (target_pos  - d.{copy_origin_field}) / mirror_dims;\n\
             \x20   let original   = textureSampleLevel(scratch_mirror_tex, scratch_mirror_smp, original_uv, 0.0);\n\
             \x20   return mix(original, warped, sel * warp_mask);\n",
            copy_origin_field = copy_origin_field,
        );

        Ok(wgsl)
    }

    /// Preview-mode body. The stroke body samples `scratch_mirror`
    /// (bound at `@group(3)` in stroke mode, omitted in preview);
    /// preview emits the falloff disc so scrubbing the softness slider
    /// visibly reshapes the cursor — helpful side-effect of reusing
    /// the same `falloff_fn` the stroke decls emit.
    ///
    /// The overlay's `KIND_MASKED_STAMP` reads only the `.r` channel
    /// of this mask as coverage; the displayed colour comes from
    /// `fs_snapshot`'s background-shift math, not from anything we
    /// write here. So `.r = f` puts liquify's peak coverage on par
    /// with paint's at the centre. An earlier version multiplied by
    /// `0.6` thinking that emitted "neutral gray", which actually
    /// just capped peak coverage at 60% — visibly fainter than other
    /// brushes.
    fn compile_preview_body(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        let softness_expr = cctx.input("softness").as_f32();
        let falloff_fn = cctx.ident("liquify_falloff");
        wgsl.body = format!(
            "    if (local_dist >= 1.0) {{ discard; }}\n\
             \x20   let softness = clamp({softness_expr}, 0.0, 1.0);\n\
             \x20   let f = {falloff_fn}(local_dist, 1.0 - softness);\n\
             \x20   return vec4<f32>(f, f, f, f);\n"
        );
        Ok(wgsl)
    }
}

// ── Per-brush pipeline build helper ─────────────────────────────────────

fn ensure_per_brush_pipeline(
    gpu: &BrushGpuContext,
    pipe: &LiquifyPipeline,
    compiled: &CompiledBrush,
) {
    if pipe.cache.borrow().contains_key(&compiled.topology_hash) {
        return;
    }
    let ctx = BuildContext {
        device: gpu.device,
        queue: gpu.queue,
        uniform_bgl: gpu.pipelines.uniform_bind_group_layout(),
        selection_bgl: gpu.pipelines.selection_bind_group_layout(),
        canvas_copy_bgl: gpu.pipelines.canvas_copy_bind_group_layout(),
        canvas_copy_sampler: gpu.pipelines.canvas_copy_sampler(),
        min_uniform_align: gpu.device.limits().min_uniform_buffer_offset_alignment,
    };
    pipe.ensure_pipeline(&ctx, compiled);
}
