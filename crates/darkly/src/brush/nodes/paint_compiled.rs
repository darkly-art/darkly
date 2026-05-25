//! Paint Compiled terminal — single-pass instanced fragment with a
//! per-brush compiled WGSL shader.
//!
//! ## What this terminal does
//!
//! Same structural shape as [`paint`](super::paint): per-dab records
//! queue up via [`BrushGpuContext::pending_dab_bytes`], one instanced
//! render pass drains them at phase end. The differences:
//!
//! - **The fragment shader is generated per-brush at brush load** by
//!   walking the upstream graph and asking each node to emit WGSL.
//!   See [`crate::brush::wgsl_compile`].
//! - **The per-dab record schema is dynamic**, sized by what fields
//!   the brush's nodes contribute. No fixed `PaintDabRecord` struct.
//! - **The uniform buffer carries stroke-constant values** from any
//!   upstream nodes that declared `uniform_fields` (e.g. `paint_color`).
//!
//! No upstream GPU dispatch happens — `circle`, `stamp`, etc. compile
//! inline into the fragment shader, evaluated per-fragment-per-dab.
//! No dab pool slots, no intermediate textures.
//!
//! ## Pipeline cache
//!
//! Per-brush pipelines are built lazily on the first `flush_dabs`
//! call and cached on [`PaintCompiledPipeline`] keyed by the brush
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
    pack_dab_record, pack_uniforms, CompileWgslCtx, CompiledBrush, InputBinding, NodeWgsl,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

/// Canvas-pixel reference for `size_input * size = 1.0`. Mirrors
/// `paint::SIZE_REFERENCE_PX` so a brush feels identical whether it
/// terminates in `paint` or `paint_compiled`.
const SIZE_REFERENCE_PX: f32 = crate::brush::dab_pool::DAB_REFERENCE_SIZE as f32;

/// Maximum uniform buffer size we'll allocate per brush pipeline.
const MAX_UNIFORM_BYTES: usize = 1024;

// ── Intrinsic uniforms ──────────────────────────────────────────────────

/// The `IntrinsicUniforms` struct from `_compiled_prelude.wgsl`. The
/// terminal packs this at the front of every uniform buffer; node-
/// contributed uniforms follow.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct IntrinsicUniforms {
    layer_offset: [i32; 2],
    layer_size: [u32; 2],
    canvas_size: [u32; 2],
    _pad: [u32; 2],
}

const INTRINSIC_UNIFORMS_SIZE: usize = std::mem::size_of::<IntrinsicUniforms>();

// ── Per-brush pipeline ──────────────────────────────────────────────────

/// Per-brush resources built on the first `flush_dabs` call for a
/// brush with a given `topology_hash`. Cached on [`PaintCompiledPipeline`].
struct PerBrushPipeline {
    paint_pipeline: wgpu::RenderPipeline,
    erase_pipeline: wgpu::RenderPipeline,
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
                label: Some("paint_compiled-brush"),
                source: wgpu::ShaderSource::Wgsl(compiled.wgsl.clone().into()),
            });

        // group(1): dabs storage buffer. Same VERTEX_FRAGMENT visibility
        // as `paint` — vertex stage reads `pos`/`radius` to build the
        // quad, fragment stage reads the rest.
        let dabs_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("paint_compiled-dabs-bgl"),
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
                label: Some("paint_compiled-layout"),
                bind_group_layouts: &[ctx.uniform_bgl, &dabs_bgl, ctx.selection_bgl],
                immediate_size: 0,
            });

        // Same paint/erase blend states as `paint` — premultiplied
        // source-over for paint, destination-out for erase.
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
        let erase_blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let make_pipeline = |label: &str, blend: wgpu::BlendState| {
            ctx.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
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
                            blend: Some(blend),
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
                })
        };
        let paint_pipeline = make_pipeline("paint_compiled", paint_blend);
        let erase_pipeline = make_pipeline("paint_compiled-erase", erase_blend);

        // Uniform ring sized for this brush's actual uniform layout.
        let uniform_size =
            (INTRINSIC_UNIFORMS_SIZE + compiled.uniform_size).max(INTRINSIC_UNIFORMS_SIZE);
        let uniform_ring = DynamicUniformRing::new(
            ctx.device,
            "paint_compiled-uniforms",
            uniform_size as u64,
            ctx.min_uniform_align,
        );
        let uniform_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint_compiled-uniform-bg"),
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
            label: Some("paint_compiled-dabs-buffer"),
            size: dabs_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dabs_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint_compiled-dabs-bg"),
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
            erase_pipeline,
            uniform_ring,
            uniform_bind_group,
            dabs_buffer,
            dabs_bind_group,
            uniform_size,
        }
    }
}

// ── Pipeline registry entry ─────────────────────────────────────────────

/// The single registry entry for the `paint_compiled` terminal. Holds
/// a cache of per-brush pipelines keyed by `topology_hash`. Pipelines
/// are built lazily on first use.
pub struct PaintCompiledPipeline {
    cache: RefCell<HashMap<u64, PerBrushPipeline>>,
}

impl PaintCompiledPipeline {
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

impl BrushPipelineEntry for PaintCompiledPipeline {
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

fn paint_compiled_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "paint_compiled",
        build: |ctx| Box::new(PaintCompiledPipeline::build(ctx)),
    }
}

// ── Node ────────────────────────────────────────────────────────────────

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![paint_compiled_pipeline_reg()],
        node: NodeRegistration {
            type_id: "paint_compiled",
            category: "output",
            display_name: "Paint (Compiled)",
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
        },
    }
}

pub struct PaintCompiledEvaluator;

impl PaintCompiledEvaluator {
    fn effective_radius(ctx: &EvalContext) -> f32 {
        let size_input = ctx.input_f32("size_input").max(0.0);
        let size = ctx.input_f32("size").max(0.0);
        let effective_size = size_input * size;
        (effective_size * SIZE_REFERENCE_PX * 0.5).max(0.5)
    }

    /// Pack the intrinsic dab header (`pos` + `radius` + `_pad`) at
    /// the end of the byte buffer. Followed by node-contributed
    /// fields via [`pack_dab_record`].
    fn pack_intrinsic_header(bytes: &mut Vec<u8>, pos: [f32; 2], radius: f32) {
        bytes.extend_from_slice(bytemuck::bytes_of(&pos));
        bytes.extend_from_slice(bytemuck::bytes_of(&radius));
        bytes.extend_from_slice(bytemuck::bytes_of(&0.0f32)); // _pad
    }

    /// Pack the intrinsic uniforms (layer offset/size, canvas size)
    /// at the front of the uniform buffer. Followed by node-
    /// contributed uniforms via [`pack_uniforms`].
    fn pack_intrinsic_uniforms(bytes: &mut Vec<u8>, intrinsic: IntrinsicUniforms) {
        bytes.extend_from_slice(bytemuck::bytes_of(&intrinsic));
    }
}

impl BrushNodeEvaluator for PaintCompiledEvaluator {
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
            debug_assert!(
                false,
                "paint_compiled requires compiled_brush on gpu_context"
            );
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

        // Layer-clip the dab's footprint. Matches `paint`'s tracking
        // so the bench harness's per-flush union-bbox reads the same.
        let canvas_ext = paint_target.canvas_extent();
        let layer_x0 = canvas_ext.x0() as f32;
        let layer_y0 = canvas_ext.y0() as f32;
        let layer_x1 = layer_x0 + canvas_ext.width as f32;
        let layer_y1 = layer_y0 + canvas_ext.height as f32;
        let cx0 = (position[0] - radius).max(layer_x0);
        let cy0 = (position[1] - radius).max(layer_y0);
        let cx1 = (position[0] + radius).min(layer_x1);
        let cy1 = (position[1] + radius).min(layer_y1);
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
        Self::pack_intrinsic_header(&mut gpu.pending_dab_bytes, position, radius);
        let outputs = gpu
            .slot_outputs_owned
            .as_ref()
            .expect("paint_compiled requires slot_outputs_owned on gpu_context");
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
            "paint_compiled dab queue overflowed MAX_DABS_PER_PHASE"
        );

        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    fn begin_stroke(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("paint_compiled::begin_stroke requires Scratch");
        let _ = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("paint_compiled-begin_stroke"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: scratch.write_view(),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        gpu.clear_pending_dabs();
    }

    fn flush_dabs(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.pending_dab_count == 0 {
            return;
        }
        let Some(compiled) = gpu.compiled_brush.clone() else {
            debug_assert!(false, "paint_compiled::flush_dabs requires compiled_brush");
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

        let pipeline_ref = gpu.pipelines.get::<PaintCompiledPipeline>("paint_compiled");

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
            .expect("paint_compiled::flush_dabs requires Scratch");
        let paint_target = gpu
            .paint_target
            .as_ref()
            .expect("paint_compiled::flush_dabs requires paint_target");
        let canvas_ext = paint_target.canvas_extent();
        let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];
        let layer_size = [canvas_ext.width, canvas_ext.height];

        // Build the uniform buffer: intrinsic header + node fields.
        // Per-stroke not per-dab, but still no need to clone.
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
            .expect("paint_compiled::flush_dabs requires slot_outputs_owned");
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

            let pipeline = if gpu.blend_mode == 1 {
                &per_brush.erase_pipeline
            } else {
                &per_brush.paint_pipeline
            };
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("paint_compiled-flush"),
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

    /// Preview is unimplemented for now — falls through to a no-op so
    /// the hover cursor disappears for compiled brushes. The shape-
    /// aware procedural preview lives on the to-do list (see
    /// `handoff-brush-preview.md`).
    fn render_preview(
        &self,
        _ctx: &EvalContext,
        _gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    /// Emit the fragment-shader body's terminal — multiplies the
    /// upstream graph's premultiplied RGBA expression by the
    /// selection mask and returns. The framework's
    /// [`crate::brush::wgsl_compile::assemble_shader`] places the
    /// node bodies inside `fs_main` already bound with `d`, `u`,
    /// `local_uv`, `local_dist`, `theta`, `canvas_pos`, and `sel`.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        let rgba_expr = match cctx.inputs.get("rgba") {
            Some(InputBinding::Wired(expr)) => expr.clone(),
            _ => {
                // Unwired rgba — fall back to opaque white modulated
                // by the soft-disc that the wrapper's `local_dist`
                // gives us. This makes a graph with just
                // pen → paint_compiled still produce something
                // visible, mirroring `paint`'s procedural-disc
                // fallback.
                "vec4<f32>(1.0, 1.0, 1.0, 1.0) * max(1.0 - local_dist, 0.0)".into()
            }
        };
        wgsl.body = format!(
            "    let rgba = {rgba_expr};\n\
             \x20   return rgba * sel;\n"
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
    pipe: &PaintCompiledPipeline,
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
        watercolor_sources_bgl: gpu.pipelines.watercolor_sources_bind_group_layout(),
        dab_bgl: gpu.dab_pool.bind_group_layout(),
        canvas_copy_sampler: gpu.pipelines.canvas_copy_sampler(),
        min_uniform_align: gpu.device.limits().min_uniform_buffer_offset_alignment,
    };
    pipe.ensure_pipeline(&ctx, compiled);
}
