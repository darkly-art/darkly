//! Paint terminal — single-pass instanced fragment with a per-brush
//! compiled WGSL shader.
//!
//! ## What this terminal does
//!
//! Per-dab records queue up via [`BrushGpuContext::pending_dab_bytes`];
//! one instanced render pass drains them at phase end.
//!
//! - **The fragment shader is generated per-brush at brush load** by
//!   walking the upstream graph and asking each node to emit WGSL.
//!   See [`crate::brush::wgsl_compile`].
//! - **The per-dab record schema is dynamic**, sized by what fields
//!   the brush's nodes contribute. No fixed `PaintDabRecord` struct.
//! - **The uniform buffer carries stroke-constant values** from any
//!   upstream nodes that declared `uniform_fields` (e.g. `paint_color`).
//!
//! Upstream nodes (`circle`, `stamp`, etc.) compile inline into the
//! fragment shader and evaluate per-fragment-per-dab — no intermediate
//! textures.
//!
//! ## Pipeline cache
//!
//! Per-brush pipelines are built lazily on the first `flush_dabs`
//! call and cached on [`PaintPipeline`] keyed by the brush
//! graph's `topology_hash`. Two brushes with identical graph
//! topologies share a pipeline.
//!
//! ## Brush load failure
//!
//! Compilation happens in [`crate::brush::compile_graph`]. If any
//! upstream node returns `Err` from `compile_wgsl`, brush load fails
//! — there is no runtime fallback. See
//! [`crate::brush::wgsl_compile::CompileError`].

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::{BrushGpuContext, MAX_DABS_PER_PHASE};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipeline::{
    BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};
use crate::brush::wgsl_compile::{
    pack_dab_record, pack_intrinsic_dab_header, pack_intrinsic_uniforms, pack_uniforms,
    CompileWgslCtx, CompiledBrush, InputBinding, IntrinsicUniforms, NodeWgsl,
    INTRINSIC_UNIFORMS_SIZE,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

/// Canvas-pixel reference for `size_input * size = 1.0`. Same
/// `DAB_REFERENCE_SIZE` used by every other brush node — see
/// [`crate::brush::DAB_REFERENCE_SIZE`].
const SIZE_REFERENCE_PX: f32 = crate::brush::DAB_REFERENCE_SIZE as f32;

/// Maximum uniform buffer size we'll allocate per brush pipeline.
const MAX_UNIFORM_BYTES: usize = 1024;

// ── Per-brush pipeline ──────────────────────────────────────────────────

/// Per-brush resources built on the first `flush_dabs` call for a
/// brush with a given `topology_hash`. Cached on [`PaintPipeline`].
struct PerBrushPipeline {
    /// Per-dab pipeline. Always premultiplied source-over — the scratch
    /// is a coverage accumulator and only paints alpha *up*. Engine-level
    /// paint-vs-erase is a stroke decision applied at commit by
    /// `commit_brush_dab`, not here. (Branching the per-dab pass on
    /// `blend_mode` to a destination-out blend was a regression: the
    /// scratch starts at (0,0,0,0), so `dst*(1-src.a)` stays zero and
    /// the commit's `destination_out` then sees zero alpha and no-ops.)
    paint_pipeline: wgpu::RenderPipeline,
    uniform_ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
    dabs_buffer: wgpu::Buffer,
    dabs_bind_group: wgpu::BindGroup,
    /// Total size of the uniform block (intrinsic + node fields), in bytes.
    uniform_size: usize,
}

impl PerBrushPipeline {
    fn build(ctx: &BuildContext, compiled: &CompiledBrush) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("paint-brush"),
                source: wgpu::ShaderSource::Wgsl(compiled.stroke_wgsl.clone().into()),
            });

        // group(1): dabs storage buffer. Same VERTEX_FRAGMENT visibility
        // as `paint` — vertex stage reads `pos`/`bbox_target_px` to build the
        // quad, fragment stage reads the rest.
        let dabs_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("paint-dabs-bgl"),
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
                label: Some("paint-layout"),
                bind_group_layouts: &[ctx.uniform_bgl, &dabs_bgl, ctx.selection_bgl],
                immediate_size: 0,
            });

        // Premultiplied source-over: scratch accumulates coverage. See
        // the `paint_pipeline` field doc above for why there's no erase
        // variant at this stage.
        let paint_blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let paint_pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("paint"),
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
                        blend: Some(paint_blend),
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

        // Uniform ring sized for this brush's actual uniform layout.
        let uniform_size =
            (INTRINSIC_UNIFORMS_SIZE + compiled.uniform_size).max(INTRINSIC_UNIFORMS_SIZE);
        let uniform_ring = DynamicUniformRing::new(
            ctx.device,
            "paint-uniforms",
            uniform_size as u64,
            ctx.min_uniform_align,
        );
        let uniform_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint-uniform-bg"),
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

        // Dab record buffer sized for this brush's record stride.
        let dab_record_size = compiled.dab_record_size.max(16);
        let dabs_buffer_size = (MAX_DABS_PER_PHASE as u64) * (dab_record_size as u64);
        let dabs_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("paint-dabs-buffer"),
            size: dabs_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dabs_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint-dabs-bg"),
            layout: &dabs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dabs_buffer.as_entire_binding(),
            }],
        });

        // Avoid the unused-let warning while keeping the variable
        // for documentation — `dab_record_size` is what determines
        // `dabs_buffer_size` above.
        let _ = dab_record_size;

        Self {
            paint_pipeline,
            uniform_ring,
            uniform_bind_group,
            dabs_buffer,
            dabs_bind_group,
            uniform_size,
        }
    }
}

// ── Pipeline registry entry ─────────────────────────────────────────────

/// The single registry entry for the `paint` terminal. Holds
/// a cache of per-brush pipelines keyed by `topology_hash`. Pipelines
/// are built lazily on first use.
pub struct PaintPipeline {
    cache: RefCell<HashMap<u64, PerBrushPipeline>>,
}

impl PaintPipeline {
    fn build(_ctx: &BuildContext) -> Self {
        Self {
            cache: RefCell::new(HashMap::new()),
        }
    }

    /// Build (or look up) the per-brush pipeline for `compiled`. Called
    /// on every `flush_dabs` — the first call for a hash builds; later
    /// calls reuse. With ~tens of brushes max, the HashMap lookup is
    /// noise compared to the render pass cost.
    fn ensure_pipeline(&self, ctx: &BuildContext, compiled: &CompiledBrush) {
        let mut cache = self.cache.borrow_mut();
        cache
            .entry(compiled.topology_hash)
            .or_insert_with(|| PerBrushPipeline::build(ctx, compiled));
    }

    /// Run a closure with the per-brush pipeline. Panics if the
    /// pipeline hasn't been built yet (caller must `ensure_pipeline`
    /// first within the same `flush_dabs` invocation).
    fn with_pipeline<R>(&self, hash: u64, f: impl FnOnce(&PerBrushPipeline) -> R) -> R {
        let cache = self.cache.borrow();
        let p = cache
            .get(&hash)
            .expect("ensure_pipeline must run before with_pipeline");
        f(p)
    }
}

impl BrushPipelineEntry for PaintPipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        None
    }
    fn rings(&self) -> Vec<&DynamicUniformRing> {
        // The ring is owned by each per-brush pipeline. We can't
        // safely return references through the RefCell — the frame
        // reset loop expects &DynamicUniformRing with a lifetime tied
        // to self, but the rings live behind a RefCell borrow that
        // doesn't outlive this call. Workaround: keep the rings out
        // of the central reset loop and reset them ourselves on each
        // `flush_dabs` (the ring only holds per-flush state).
        Vec::new()
    }
}

fn paint_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "paint",
        build: |ctx| Box::new(PaintPipeline::build(ctx)),
    }
}

// ── Node ────────────────────────────────────────────────────────────────

pub const TYPE_ID: &str = "paint";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![paint_pipeline_reg()],
        evaluator: || Box::new(PaintEvaluator),
        lifecycle: crate::brush::node::Lifecycle::ClearScratchToTransparent,
        node: NodeRegistration {
            type_id: TYPE_ID,
            category: "output",
            display_name: "Paint",
            ports: vec![
                PortDef::input("position", BrushWireType::Vec2)
                    .with_description("Canvas-pixel pen tip for this dab"),
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
                PortDef::input("flow", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Flow")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-droplet")
                    .exposed()
                    .with_description("Stroke-level flow cap (folded into rgba alpha)"),
                PortDef::input("opacity", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Opacity")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-fill-drip")
                    .exposed()
                    .with_description("Stroke-level opacity cap (applied at commit)"),
                // Cursor-preview rotation in radians. Wire from
                // `pen.tilt_direction` or `pen.drawing_angle` to make
                // the hover-cursor mask rotate with the pen. The
                // current paint shader doesn't apply this to the
                // stroke deposit — it's read only by
                // `render_preview` for the overlay. Defaults to 0
                // (no rotation).
                PortDef::input("rotation", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                    .with_description("Cursor-preview rotation (radians)"),
                // Typed as `Texture` to match the upstream `stamp.dab`
                // output's wire type — the wire-type label is shared
                // with the per-dab dispatch model where it'd be a
                // texture handle. In the compiled path it's a
                // `vec4<f32>` expression. Without this match, the
                // graph compiler rejects the connection at brush load.
                PortDef::input("rgba", BrushWireType::Texture).with_description(
                    "Premultiplied RGBA from the upstream compiled graph (typically `stamp.dab`)",
                ),
                PortDef::output("dab_size", BrushWireType::Vec2)
                    .with_description("Brush mark size in canvas pixels"),
            ],
            params: &[],
            is_gpu: true,
            is_terminal: true,
            supports_erase: true,
        },
    }
}

pub struct PaintEvaluator;

impl PaintEvaluator {
    fn effective_radius(ctx: &EvalContext) -> f32 {
        let size_input = ctx.input_f32("size_input").max(0.0);
        let size = ctx.input_f32("size").max(0.0);
        let effective_size = size_input * size;
        (effective_size * SIZE_REFERENCE_PX * 0.5).max(0.5)
    }
}

impl BrushNodeEvaluator for PaintEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let Some(compiled) = gpu.compiled_brush.clone() else {
            // Compiled brush wasn't attached — programming error in
            // the engine wiring. Panic in debug, drop dab silently in
            // release so we don't blow up an in-flight stroke.
            debug_assert!(false, "paint requires compiled_brush on gpu_context");
            return vec![];
        };
        let Some(paint_target) = gpu.paint_target.as_ref() else {
            return vec![];
        };
        let position = ctx.input("position").as_vec2();
        let radius = Self::effective_radius(ctx);
        let diameter = radius * 2.0;
        if diameter <= 0.0 {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        }

        // Per-brush extent: composed by the framework at compile time
        // from every upstream node's `ExtentContribution`. This is
        // exactly what the WGSL fragment shader discards past
        // (`d.bbox_target_px`); using the same value here means the
        // layer-clip bbox tracks exactly what the shader writes, and
        // mid-stroke rewinds can't truncate previous dabs.
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

        // Pack one dab record: intrinsic header + per-node fields.
        // The outputs map for the packer was built by the runner's
        // dispatch_gpu before this call (keyed by `n{id}_{port}`).
        // Split-borrow `slot_outputs_owned` and `pending_dab_bytes`
        // — they're disjoint fields, so no clone is needed. Cloning
        // the HashMap per dab was a multi-millisecond-per-event
        // disaster at high dab counts.
        let record_start = gpu.pending_dab_bytes.len();
        pack_intrinsic_dab_header(&mut gpu.pending_dab_bytes, position, bbox_radius, radius);
        let outputs = gpu
            .slot_outputs_owned
            .as_ref()
            .expect("paint requires slot_outputs_owned on gpu_context");
        pack_dab_record(&compiled, outputs, &mut gpu.pending_dab_bytes);
        // Pad to the full record size so the next dab starts aligned.
        let written = gpu.pending_dab_bytes.len() - record_start;
        if written < compiled.dab_record_size {
            gpu.pending_dab_bytes
                .resize(record_start + compiled.dab_record_size, 0);
        }
        gpu.pending_dab_count = gpu.pending_dab_count.saturating_add(1);
        debug_assert!(
            gpu.pending_dab_count <= MAX_DABS_PER_PHASE,
            "paint dab queue overflowed MAX_DABS_PER_PHASE"
        );

        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    fn flush_dabs(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.pending_dab_count == 0 {
            return;
        }
        let Some(compiled) = gpu.compiled_brush.clone() else {
            debug_assert!(false, "paint::flush_dabs requires compiled_brush");
            return;
        };

        let bbox = gpu.pending_dabs_bbox.unwrap_or([0, 0, 0, 0]);
        let union_w = bbox[2].saturating_sub(bbox[0]);
        let union_h = bbox[3].saturating_sub(bbox[1]);
        let (dab_bytes, total_dabs) = gpu.take_pending_dabs();
        if total_dabs == 0 {
            return;
        }
        gpu.perf
            .record_dab_flush_workload(total_dabs, union_w, union_h);

        let pipeline_ref = gpu.pipelines.get::<PaintPipeline>("paint");

        // Build the per-brush pipeline if this is the first dab for
        // this hash. The BuildContext borrows pieces from
        // BrushPipelines via private accessors — we use a minimal
        // local BuildContext built from the gpu_context's wgpu refs.
        // Note: this is a one-shot build per brush, so the cost is
        // amortised across thousands of dabs.
        ensure_per_brush_pipeline(gpu, pipeline_ref, &compiled);

        let scratch = gpu
            .scratch
            .as_deref()
            .expect("paint::flush_dabs requires Scratch");
        let paint_target = gpu
            .paint_target
            .as_ref()
            .expect("paint::flush_dabs requires paint_target");
        let canvas_ext = paint_target.canvas_extent();
        let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];
        let layer_size = [canvas_ext.width, canvas_ext.height];

        // Build the uniform buffer: intrinsic header + node fields.
        // Per-stroke not per-dab, but still no need to clone.
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
            .expect("paint::flush_dabs requires slot_outputs_owned");
        pack_uniforms(&compiled, outputs, &mut uniform_bytes);

        pipeline_ref.with_pipeline(compiled.topology_hash, |per_brush| {
            // Pad uniform bytes up to the per-brush uniform size so the
            // ring entry's binding_size matches.
            if uniform_bytes.len() < per_brush.uniform_size {
                uniform_bytes.resize(per_brush.uniform_size, 0);
            }
            // Reset the ring before each flush — the ring is per-
            // brush and isn't shared with other terminals, so this is
            // safe (we own all live writes in this `flush_dabs`).
            per_brush.uniform_ring.reset();
            let uniform_offset = per_brush.uniform_ring.write(gpu.queue, &uniform_bytes);

            // Upload the dab records.
            gpu.queue
                .write_buffer(&per_brush.dabs_buffer, 0, &dab_bytes);

            // Always source-over at per-dab. Paint-vs-erase routes through
            // `gpu.blend_mode` in `commit_brush_dab`; see `paint_pipeline`'s
            // doc on `PerBrushPipeline`.
            let pipeline = &per_brush.paint_pipeline;
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("paint-flush"),
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
            pass.set_viewport(
                0.0,
                0.0,
                layer_size[0] as f32,
                layer_size[1] as f32,
                0.0,
                1.0,
            );
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &per_brush.uniform_bind_group, &[uniform_offset]);
            pass.set_bind_group(1, &per_brush.dabs_bind_group, &[]);
            pass.set_bind_group(2, gpu.selection_bind_group, &[]);
            pass.draw(0..6, 0..total_dabs);
        });

        gpu.perf.record_dab_flush(total_dabs);
    }

    fn commit(&self, ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let Some(pre_stroke_bg) = gpu.pre_stroke_bind_group else {
            return;
        };
        let Some(scratch) = gpu.scratch.as_deref() else {
            return;
        };
        let Some(paint_target) = gpu.paint_target.as_ref() else {
            return;
        };
        let opacity = ctx.input_f32("opacity").clamp(0.0, 1.0);
        paint_target.commit_brush_dab(
            &mut gpu.encoder,
            gpu.pipelines,
            gpu.queue,
            scratch.write_bind_group(),
            gpu.selection_bind_group,
            pre_stroke_bg,
            opacity,
            gpu.blend_mode,
            /* fg_premultiplied */ true,
        );
    }

    /// Hover-cursor preview — reuses the shared
    /// [`crate::brush::wgsl_compile::render_compiled_preview`] helper.
    /// `paint`'s stroke body and preview body are the same
    /// source (no `compile_preview_body` override), so the cursor
    /// shows the brush color × shape × flow as the stroke would
    /// deposit.
    fn render_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let radius = Self::effective_radius(ctx);
        let rotation_rad = ctx.input_f32("rotation");
        let _ = crate::brush::wgsl_compile::render_compiled_preview(gpu, radius, rotation_rad);
        vec![]
    }

    /// Emit the fragment-shader body's terminal — multiplies the
    /// upstream graph's premultiplied RGBA expression by the
    /// selection mask and returns. The framework's
    /// [`crate::brush::wgsl_compile::assemble_shader`] places the
    /// node bodies inside `fs_main` already bound with `d`, `u`,
    /// `local_uv`, `local_dist`, `theta`, `target_pos`, and `sel`.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        let rgba_expr = match cctx.inputs.get("rgba") {
            Some(InputBinding::Wired(expr)) => expr.clone(),
            _ => {
                // Unwired rgba — fall back to opaque white modulated
                // by the soft-disc that the wrapper's `local_dist`
                // gives us. This makes a graph with just
                // pen → paint still produce something
                // visible, mirroring `paint`'s procedural-disc
                // fallback.
                "vec4<f32>(1.0, 1.0, 1.0, 1.0) * max(1.0 - local_dist, 0.0)".into()
            }
        };
        // Stroke-/dab-level flow cap. Matches the `paint` terminal's
        // `color[3] *= flow` step — folded directly into the
        // premultiplied rgba (multiply all four components). Wired
        // values flow through their dab-record field; unwired uses
        // the port default literal (1.0 by default).
        let flow_expr = cctx.input("flow").as_f32();
        wgsl.body = format!(
            "    let rgba = {rgba_expr};\n\
             \x20   let flow = clamp({flow_expr}, 0.0, 1.0);\n\
             \x20   return rgba * flow * sel;\n"
        );
        Ok(wgsl)
    }
}

// ── Per-brush pipeline build helper ─────────────────────────────────────

/// Build the per-brush pipeline for `compiled` if it isn't already
/// cached. Reconstructs a [`BuildContext`] from the `BrushGpuContext`'s
/// shared state — same BGLs and shared limits used at the original
/// `BrushPipelines::new` time, so the layouts match.
fn ensure_per_brush_pipeline(
    gpu: &BrushGpuContext,
    pipe: &PaintPipeline,
    compiled: &CompiledBrush,
) {
    // Skip the work entirely if the pipeline is already cached.
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
