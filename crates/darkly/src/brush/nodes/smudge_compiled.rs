//! Smudge Compiled terminal — per-dab fragment-pass smear with a
//! per-brush compiled WGSL shader.
//!
//! Same outline as [`paint_compiled`](super::paint_compiled), but with
//! a per-dab flush loop instead of a single instanced draw. Each smudge
//! dab samples the scratch read mirror twice — once at `canvas_pos`
//! (current background) and once at `canvas_pos − motion` (the smear
//! sample, what was under the brush at the previous dab) — and mixes
//! the two by `rate × mask × selection × stroke_opacity`. Per-dab
//! serialization is *semantically required*: each dab must see the
//! prior dab's output, which a single instanced draw can't express.
//!
//! ## Per-dab flush
//!
//! 1. `evaluate_gpu` packs one record into [`BrushGpuContext::pending_dab_bytes`]
//!    *and* one [`SmudgeDabMeta`] into the parallel CPU-side
//!    [`BrushGpuContext::pending_dab_meta_bytes`] queue, plus the
//!    pre-computed canvas-space `copy_origin` into `slot_outputs_owned`
//!    so the framework's `pack_dab_record` picks it up via this node's
//!    `copy_origin` dab field. Stationary dabs (`|motion| < 0.5 px`)
//!    are skipped before queueing — `mix(bg, src, _)` collapses to
//!    identity in that regime.
//! 2. `flush_dabs` walks the queue in lockstep. For each dab it calls
//!    `prepare_dab_canvas_copy_split` (asymmetric read region around
//!    the dab's footprint by `±|motion|` per axis) to sync the
//!    scratch read mirror, then issues one render pass with instance
//!    index `i..i+1` so the vertex stage reads `dab_records[i]`.
//!    `wgpu` serializes encoder commands in submission order; the
//!    `copy_texture_to_texture` between passes carries an implicit
//!    barrier so each draw sees the prior draw's output once it lands
//!    in scratch — no `queue.submit()` between dabs needed.
//!
//! The read mirror's bind group is re-queried fresh each iteration
//! because [`crate::brush::scratch::Scratch::sync_read_mirror`] can
//! grow the mirror and rebuild the bind group mid-phase.
//!
//! ## Blend state
//!
//! REPLACE — the fragment shader fully composes its output
//! (`mix(bg, src, amount)`) and writes it straight to scratch. The
//! framework's `LoadOp::Load` keeps prior scratch pixels intact outside
//! the dab footprint; the fragment shader discards past `d.bbox_radius`.

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
use crate::brush::wgsl_compile::{
    pack_dab_record, pack_uniforms, CompileWgslCtx, CompiledBrush, DabField, NodeWgsl, WgslType,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

const SIZE_REFERENCE_PX: f32 = crate::brush::DAB_REFERENCE_SIZE as f32;

const MAX_UNIFORM_BYTES: usize = 1024;

/// Motion magnitude (canvas pixels) below which the dab is treated as
/// stationary and dropped before queueing — `mix(bg, src, _)` is an
/// identity write when `src == bg`.
const STATIONARY_THRESHOLD_PX: f32 = 0.5;

// ── Intrinsic uniforms ──────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct IntrinsicUniforms {
    layer_offset: [i32; 2],
    layer_size: [u32; 2],
    canvas_size: [u32; 2],
    _pad: [u32; 2],
}

const INTRINSIC_UNIFORMS_SIZE: usize = std::mem::size_of::<IntrinsicUniforms>();

// ── Per-dab CPU meta ────────────────────────────────────────────────────

/// CPU-side per-dab footprint info, packed by `evaluate_gpu` in
/// lockstep with the GPU dab record and drained by `flush_dabs`. Lets
/// the flush loop call `prepare_dab_canvas_copy_split` without
/// re-deriving the asymmetric footprint from the upload buffer.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SmudgeDabMeta {
    position: [f32; 2],
    write_half: [f32; 2],
    read_half: [f32; 2],
}

const SMUDGE_DAB_META_SIZE: usize = std::mem::size_of::<SmudgeDabMeta>();

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
                label: Some("smudge_compiled-brush"),
                source: wgpu::ShaderSource::Wgsl(compiled.wgsl.clone().into()),
            });

        let dabs_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("smudge_compiled-dabs-bgl"),
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

        // group(0..2) standard; group(3) is the scratch read mirror —
        // same layout as `watercolor_compiled`'s atlas binding, only
        // the binding semantics differ.
        let layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("smudge_compiled-layout"),
                bind_group_layouts: &[
                    ctx.uniform_bgl,
                    &dabs_bgl,
                    ctx.selection_bgl,
                    ctx.canvas_copy_bgl,
                ],
                immediate_size: 0,
            });

        // REPLACE blend — the fragment shader writes the final smeared
        // pixel; outside the disc it discards so LoadOp::Load preserves
        // the scratch.
        let pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("smudge_compiled"),
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
            "smudge_compiled-uniforms",
            uniform_size as u64,
            ctx.min_uniform_align,
        );
        let uniform_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("smudge_compiled-uniform-bg"),
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
            label: Some("smudge_compiled-dabs-buffer"),
            size: dabs_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dabs_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("smudge_compiled-dabs-bg"),
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

pub struct SmudgeCompiledPipeline {
    cache: RefCell<HashMap<u64, PerBrushPipeline>>,
}

impl SmudgeCompiledPipeline {
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

impl BrushPipelineEntry for SmudgeCompiledPipeline {
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

fn smudge_compiled_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "smudge_compiled",
        build: |ctx| Box::new(SmudgeCompiledPipeline::build(ctx)),
    }
}

// ── Node ────────────────────────────────────────────────────────────────

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![smudge_compiled_pipeline_reg()],
        node: NodeRegistration {
            type_id: "smudge_compiled",
            category: "output",
            display_name: "Smudge (Compiled)",
            ports: vec![
                PortDef::input("position", BrushWireType::Vec2)
                    .with_description("Canvas-pixel pen tip for this dab"),
                PortDef::input("motion", BrushWireType::Vec2)
                    .with_description("Per-dab motion vector — the offset to sample from"),
                PortDef::input("size_input", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Size Input")
                    .with_unit(UnitType::Percent)
                    .with_description(
                        "Per-touch size multiplier (wire pressure here for pressure-sensitive size).",
                    ),
                PortDef::input("size", BrushWireType::Scalar)
                    .with_range(0.0, 4.0, 0.1)
                    .with_label("Size")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-up-right-and-down-left-from-center")
                    .exposed()
                    .with_preview_value(0.1)
                    .with_description("Overall brush size"),
                PortDef::input("rate", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.6)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Smudge")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-paint-roller")
                    .exposed()
                    .with_description(
                        "How strongly each touch drags the canvas along the stroke. \
                         Higher values produce a longer smear trail; lower values \
                         barely move pixels.",
                    ),
                PortDef::input("opacity", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Opacity")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-droplet")
                    .exposed()
                    .with_description(
                        "Overall stroke strength. Lower values reduce how much the smudge affects the canvas.",
                    ),
                // Same `Texture` wire-type as watercolor_compiled.mask
                // — the upstream compiled `circle.texture` output is a
                // scalar coverage expression; the wire-type label is
                // shared with the per-dab dispatch model.
                PortDef::input("mask", BrushWireType::Texture).with_description(
                    "Per-fragment shape coverage (typically wired from circle.texture)",
                ),
                PortDef::output("dab_size", BrushWireType::Vec2)
                    .with_description("Brush mark size in canvas pixels"),
            ],
            params: &[],
            is_gpu: true,
        },
    }
}

pub struct SmudgeCompiledEvaluator;

impl SmudgeCompiledEvaluator {
    fn effective_radius(ctx: &EvalContext) -> f32 {
        let size_input = ctx.input_f32("size_input").max(0.0);
        let size = ctx.input_f32("size").max(0.0);
        let effective_size = size_input * size;
        (effective_size * SIZE_REFERENCE_PX * 0.5).max(0.5)
    }

    fn pack_intrinsic_header(bytes: &mut Vec<u8>, pos: [f32; 2], radius: f32, bbox_radius: f32) {
        bytes.extend_from_slice(bytemuck::bytes_of(&pos));
        bytes.extend_from_slice(bytemuck::bytes_of(&radius));
        bytes.extend_from_slice(bytemuck::bytes_of(&bbox_radius));
    }

    fn pack_intrinsic_uniforms(bytes: &mut Vec<u8>, intrinsic: IntrinsicUniforms) {
        bytes.extend_from_slice(bytemuck::bytes_of(&intrinsic));
    }

    /// Insert `copy_origin` into `slot_outputs_owned` under this
    /// node's dab-field key so the framework's `pack_dab_record`
    /// picks it up via the `copy_origin` `DabField` declared in
    /// `compile_wgsl`.
    fn insert_copy_origin(gpu: &mut BrushGpuContext, node_id: u32, value: [f32; 2]) {
        if let Some(outputs) = gpu.slot_outputs_owned.as_mut() {
            outputs.insert(
                format!("n{}_copy_origin", node_id),
                ScalarValue::Vec2(value),
            );
        }
    }
}

impl BrushNodeEvaluator for SmudgeCompiledEvaluator {
    fn is_compiled_terminal(&self) -> bool {
        true
    }

    /// Erase mode (destination-out) on a smear isn't meaningful —
    /// matches the dispatch-path smudge terminal.
    fn supports_erase(&self) -> bool {
        false
    }

    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let Some(compiled) = gpu.compiled_brush.clone() else {
            debug_assert!(
                false,
                "smudge_compiled requires compiled_brush on gpu_context"
            );
            return vec![];
        };
        let Some(paint_target) = gpu.paint_target.as_ref() else {
            return vec![];
        };
        let position = ctx.input("position").as_vec2();
        let motion = ctx.input("motion").as_vec2();
        let radius = Self::effective_radius(ctx);
        let diameter = radius * 2.0;
        if diameter <= 0.0 {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        }

        // Stationary-dab early-out — `mix(bg, src, _)` is identity in
        // this regime. Skipping the queue saves a render pass and a
        // mirror copy. Matches the dispatch-path early-out at
        // [`smudge.rs:278`] before the migration.
        if motion[0].abs() < STATIONARY_THRESHOLD_PX && motion[1].abs() < STATIONARY_THRESHOLD_PX {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        }

        // Per-brush extent: composed by the framework at compile time.
        // For smudge this is `1.0` when the upstream is a plain disc.
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

        // Asymmetric read region — expanded by `|motion|` per axis so
        // the smear sample at `canvas_pos − motion` always lies inside
        // the mirror snapshot. Symmetric expansion (~2× per axis vs. a
        // signed-motion tight fit) keeps the math simple; the cost is
        // negligible compared to the per-dab pass.
        let write_half_w = bbox_radius;
        let write_half_h = bbox_radius;
        let read_half_w = write_half_w + motion[0].abs().ceil();
        let read_half_h = write_half_h + motion[1].abs().ceil();

        // Pre-compute `copy_origin` (canvas-space top-left of the
        // mirror snapshot) using the same formula
        // `prepare_dab_canvas_copy_split` will use at flush time. The
        // CPU compute is deterministic from `position` + `read_half`;
        // the flush-time call re-derives the same value before issuing
        // the actual sync.
        let read_x0 = (position[0] - read_half_w).max(layer_x0);
        let read_y0 = (position[1] - read_half_h).max(layer_y0);
        let copy_canvas_x = read_x0.floor();
        let copy_canvas_y = read_y0.floor();
        Self::insert_copy_origin(gpu, ctx.node_id.0 as u32, [copy_canvas_x, copy_canvas_y]);

        // Pack the GPU dab record: intrinsic header + node fields.
        let record_start = gpu.pending_dab_bytes.len();
        Self::pack_intrinsic_header(&mut gpu.pending_dab_bytes, position, radius, bbox_radius);
        let outputs = gpu
            .slot_outputs_owned
            .as_ref()
            .expect("smudge_compiled requires slot_outputs_owned on gpu_context");
        pack_dab_record(&compiled, outputs, &mut gpu.pending_dab_bytes);
        let written = gpu.pending_dab_bytes.len() - record_start;
        if written < compiled.dab_record_size {
            gpu.pending_dab_bytes
                .resize(record_start + compiled.dab_record_size, 0);
        }
        gpu.pending_dab_count = gpu.pending_dab_count.saturating_add(1);
        debug_assert!(
            gpu.pending_dab_count <= MAX_DABS_PER_PHASE,
            "smudge_compiled dab queue overflowed MAX_DABS_PER_PHASE"
        );

        // Pack the CPU-side meta in lockstep.
        let meta = SmudgeDabMeta {
            position,
            write_half: [write_half_w, write_half_h],
            read_half: [read_half_w, read_half_h],
        };
        gpu.pending_dab_meta_bytes
            .extend_from_slice(bytemuck::bytes_of(&meta));

        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    /// Seed scratch from pre-stroke so commit's scratch→layer blit
    /// reproduces unchanged pixels outside the dab footprint. Same
    /// shape as the dispatch-path smudge / watercolor_compiled.
    fn begin_stroke(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        gpu.clear_pending_dabs();

        let Some(pre_stroke) = gpu.pre_stroke_texture else {
            return;
        };
        let Some(scratch) = gpu.scratch.as_deref() else {
            return;
        };
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

    fn flush_dabs(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.pending_dab_count == 0 {
            return;
        }
        let Some(compiled) = gpu.compiled_brush.clone() else {
            debug_assert!(false, "smudge_compiled::flush_dabs requires compiled_brush");
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
            (total_dabs as usize) * SMUDGE_DAB_META_SIZE,
            "smudge_compiled meta queue out of sync with dab queue"
        );
        let metas: Vec<SmudgeDabMeta> = bytemuck::cast_slice(&meta_bytes).to_vec();
        gpu.perf
            .record_dab_flush_workload(total_dabs, union_w, union_h);

        let pipeline_ref = gpu
            .pipelines
            .get::<SmudgeCompiledPipeline>("smudge_compiled");
        ensure_per_brush_pipeline(gpu, pipeline_ref, &compiled);

        let paint_target = gpu
            .paint_target
            .as_ref()
            .expect("smudge_compiled::flush_dabs requires paint_target");
        let canvas_ext = paint_target.canvas_extent();
        let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];
        let layer_size = [canvas_ext.width, canvas_ext.height];

        let mut uniform_bytes: Vec<u8> = Vec::with_capacity(MAX_UNIFORM_BYTES);
        Self::pack_intrinsic_uniforms(
            &mut uniform_bytes,
            IntrinsicUniforms {
                layer_offset,
                layer_size,
                canvas_size: [gpu.canvas_width, gpu.canvas_height],
                _pad: [0, 0],
            },
        );
        let outputs = gpu
            .slot_outputs_owned
            .as_ref()
            .expect("smudge_compiled::flush_dabs requires slot_outputs_owned");
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
                // Sync the mirror snapshot for this dab. The implicit
                // barrier from this `copy_texture_to_texture` makes
                // the subsequent render pass see prior dab writes.
                let _ = gpu.prepare_dab_canvas_copy_split(
                    meta.position,
                    meta.write_half[0],
                    meta.write_half[1],
                    meta.read_half[0],
                    meta.read_half[1],
                );

                // Fresh read-mirror bind group each iteration — a
                // mid-loop grow can rebuild it.
                let scratch_ref = gpu
                    .scratch
                    .as_deref()
                    .expect("smudge_compiled::flush_dabs requires Scratch");
                let read_bg = scratch_ref.read_mirror_bind_group();
                let write_view = scratch_ref.write_view();

                let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("smudge_compiled-flush"),
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

    /// Direct blit scratch → layer. The scratch already holds the
    /// finished image; commit just copies it across. `gpu.blend_mode`
    /// is ignored — erase semantics aren't meaningful for a smear.
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

    fn render_preview(
        &self,
        _ctx: &EvalContext,
        _gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        // Stubbed during phase 4 of the compiled-port migration;
        // landed-with-phase-5 per-terminal preview interface owns this.
        vec![]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();

        let mask_expr = cctx.input("mask").as_f32();
        let motion_expr = cctx.input("motion").as_vec2();
        let rate_expr = cctx.input("rate").as_f32();
        let opacity_expr = cctx.input("opacity").as_f32();

        // Per-dab `copy_origin` field. The terminal's `evaluate_gpu`
        // inserts this into `slot_outputs_owned` so the packer reads
        // it through the standard `pack_dab_record` path.
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

        wgsl.body = format!(
            "    let mask = clamp({mask_expr}, 0.0, 1.0);\n\
             \x20   let motion_v = {motion_expr};\n\
             \x20   let rate = clamp({rate_expr}, 0.0, 1.0);\n\
             \x20   let stroke_opacity = clamp({opacity_expr}, 0.0, 1.0);\n\
             \x20   let mirror_dims = vec2<f32>(textureDimensions(scratch_mirror_tex));\n\
             \x20   let bg_uv = (canvas_pos - d.{copy_origin_field}) / mirror_dims;\n\
             \x20   let src_uv = (canvas_pos - motion_v - d.{copy_origin_field}) / mirror_dims;\n\
             \x20   let bg = textureSampleLevel(scratch_mirror_tex, scratch_mirror_smp, bg_uv, 0.0);\n\
             \x20   let src = textureSampleLevel(scratch_mirror_tex, scratch_mirror_smp, src_uv, 0.0);\n\
             \x20   let amount = clamp(rate * mask * sel * stroke_opacity, 0.0, 1.0);\n\
             \x20   return mix(bg, src, amount);\n",
            copy_origin_field = copy_origin_field,
        );

        Ok(wgsl)
    }
}

// ── Per-brush pipeline build helper ─────────────────────────────────────

fn ensure_per_brush_pipeline(
    gpu: &BrushGpuContext,
    pipe: &SmudgeCompiledPipeline,
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
        dab_bgl: gpu.dab_pool.bind_group_layout(),
        canvas_copy_sampler: gpu.pipelines.canvas_copy_sampler(),
        min_uniform_align: gpu.device.limits().min_uniform_buffer_offset_alignment,
    };
    pipe.ensure_pipeline(&ctx, compiled);
}
