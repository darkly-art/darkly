//! Ink Pen compute terminal — POC fold of `circle + stamp + color_output`
//! into a single compute dispatch per render-phase.
//!
//! ## What this terminal does
//!
//! Each call to `evaluate_gpu` (one per dab placement) only **queues** a
//! `DabComputeRecord` on the context. No render passes are opened. At the
//! end of the rendering phase, the runner's `flush_compute` hook fires;
//! this terminal's `flush_compute` opens **one** compute pass that
//! processes every queued dab serially inside one workgroup, writing into
//! a layer-sized storage buffer (the "compute scratch buffer" owned by
//! `Scratch`). A single `copy_buffer_to_texture` syncs the result back
//! into the regular scratch texture so the existing fragment-path
//! `commit` step (in this terminal's `commit` hook) can blend the stroke
//! onto the layer unchanged.
//!
//! ## Why
//!
//! The investigation in `darkly-stabilization-perf-investigation.md` traced
//! ~30ms/event in stabilization phase to per-dab render-pass overhead.
//! Compute eliminates that overhead by folding every dab's GPU work into
//! one pass with one barrier at each end, regardless of dab count.
//!
//! ## Scope
//!
//! Wired into the Ink Pen brush only (see `builtin_brushes::ink_pen`).
//! Other brushes continue to use the existing `stamp + color_output`
//! chain. If perf measurement shows the compute path wins, watercolor is
//! the next conversion candidate (it needs the same R/W scratch access
//! and is also slow).

use std::any::Any;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::{BrushGpuContext, DabComputeRecord};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipeline::{
    BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

/// Canvas-pixel reference for `size_input * size = 1.0`. Mirrors stamp's
/// `DAB_REFERENCE_SIZE`; kept local to avoid pulling `dab_pool` into a
/// terminal that doesn't otherwise use it.
const SIZE_REFERENCE_PX: f32 = crate::brush::dab_pool::DAB_REFERENCE_SIZE as f32;

/// Max dabs queued in one compute dispatch. A long high-stabilization
/// pen event places ~30 dabs in a stamp-based brush; round up generously.
/// If a phase exceeds this it falls back to splitting across multiple
/// dispatches (handled in `flush_compute`).
const MAX_DABS_PER_DISPATCH: u32 = 1024;

// ── Pipeline ────────────────────────────────────────────────────────────

/// Per-dispatch uniforms. Layout MUST match `Uniforms` in
/// `shaders/brush/ink_pen_compute.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct InkPenComputeUniforms {
    union_origin: [u32; 2],
    union_size: [u32; 2],
    layer_offset: [i32; 2],
    layer_size: [u32; 2],
    canvas_size: [u32; 2],
    aligned_width: u32,
    dab_count: u32,
    blend_mode: u32,
    _pad: u32,
}

pub struct InkPenComputePipeline {
    pipeline: wgpu::ComputePipeline,
    /// group(0) — uniform ring slot (dynamic offset), built from the
    /// shared `uniform_bgl`. One slot per dispatch.
    uniform_ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
    /// group(1) — dab storage buffer. Pre-allocated to
    /// `MAX_DABS_PER_DISPATCH * sizeof(DabComputeRecord)` bytes. Contents
    /// uploaded per-dispatch via `queue.write_buffer`.
    dabs_buffer: wgpu::Buffer,
    dabs_bind_group: wgpu::BindGroup,
    /// group(3) — scratch storage buffer BGL. We can't pre-build the
    /// bind group here because the buffer (on `Scratch`) is allocated
    /// lazily and may grow; rebuilt per-dispatch in `flush_compute`.
    scratch_bgl: wgpu::BindGroupLayout,
    /// Cached preview pipeline support: small fragment shader that draws
    /// a soft disc procedurally so the hover cursor still shows a brush
    /// shape when ink-pen-compute replaces the stamp + color_output chain.
    /// Reuses the existing `blit` infra to stretch the result into
    /// `preview_mask_view`.
    preview_pipeline: wgpu::RenderPipeline,
    preview_ring: DynamicUniformRing,
    preview_uniform_bind_group: wgpu::BindGroup,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PreviewUniforms {
    /// Edge softness as a fraction of radius (matches the compute path).
    softness: f32,
    _pad: [f32; 3],
}

impl InkPenComputePipeline {
    fn build(ctx: &BuildContext) -> Self {
        // ── Compute pipeline ─────────────────────────────────────────
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ink-pen-compute"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../../../shaders/brush/ink_pen_compute.wgsl").into(),
                ),
            });

        // group(1): dab storage buffer (read-only).
        let dabs_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ink-pen-compute-dabs-bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        // group(3): scratch storage buffer (read_write). Bind group is
        // built per-dispatch in `flush_compute` because the underlying
        // buffer is owned by `Scratch` and lazily allocated.
        let scratch_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ink-pen-compute-scratch-bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let pipeline_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ink-pen-compute-layout"),
                bind_group_layouts: &[ctx.uniform_bgl, &dabs_bgl, ctx.selection_bgl, &scratch_bgl],
                immediate_size: 0,
            });

        let pipeline = ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("ink-pen-compute"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            });

        let (uniform_ring, uniform_bind_group) = ctx.make_uniform_ring::<InkPenComputeUniforms>(
            "ink-pen-compute-uniforms",
            "ink-pen-compute-uniform-bg",
        );

        let dabs_buffer_size =
            (MAX_DABS_PER_DISPATCH as u64) * (std::mem::size_of::<DabComputeRecord>() as u64);
        let dabs_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ink-pen-compute-dabs-buffer"),
            size: dabs_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dabs_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ink-pen-compute-dabs-bg"),
            layout: &dabs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dabs_buffer.as_entire_binding(),
            }],
        });

        // ── Preview pipeline ─────────────────────────────────────────
        // Tiny fragment shader that draws a soft disc into a preview
        // mask. Keeps the hover cursor visible while ink-pen-compute
        // replaces stamp + color_output's preview path.
        let preview_shader_src = r#"
struct PreviewU { softness: f32, _pad0: f32, _pad1: f32, _pad2: f32 };
@group(0) @binding(0) var<uniform> u: PreviewU;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0),
    );
    var uv = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0), vec2<f32>(2.0, 1.0), vec2<f32>(0.0, -1.0),
    );
    var o: VsOut;
    o.pos = vec4<f32>(pos[i], 0.0, 1.0);
    o.uv = uv[i];
    return o;
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d = distance(in.uv, vec2<f32>(0.5, 0.5)) * 2.0; // 0 at centre, 1 at edge
    let r_solid = 1.0 - u.softness;
    var coverage: f32;
    if (d >= 1.0) { coverage = 0.0; }
    else if (d <= r_solid) { coverage = 1.0; }
    else { coverage = clamp((1.0 - d) / max(1.0 - r_solid, 1e-5), 0.0, 1.0); }
    return vec4<f32>(coverage, coverage, coverage, coverage);
}
"#;
        let preview_shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ink-pen-compute-preview"),
                source: wgpu::ShaderSource::Wgsl(preview_shader_src.into()),
            });
        let preview_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ink-pen-compute-preview-layout"),
                bind_group_layouts: &[ctx.uniform_bgl],
                immediate_size: 0,
            });
        let preview_pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("ink-pen-compute-preview"),
                layout: Some(&preview_layout),
                vertex: wgpu::VertexState {
                    module: &preview_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &preview_shader,
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
        let (preview_ring, preview_uniform_bind_group) = ctx.make_uniform_ring::<PreviewUniforms>(
            "ink-pen-compute-preview-uniforms",
            "ink-pen-compute-preview-uniform-bg",
        );

        Self {
            pipeline,
            uniform_ring,
            uniform_bind_group,
            dabs_buffer,
            dabs_bind_group,
            scratch_bgl,
            preview_pipeline,
            preview_ring,
            preview_uniform_bind_group,
        }
    }

    pub fn scratch_bgl(&self) -> &wgpu::BindGroupLayout {
        &self.scratch_bgl
    }
}

impl BrushPipelineEntry for InkPenComputePipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        // The compute uniform ring is the per-dispatch slot; the preview
        // ring is also tracked here so frame reset/overflow check covers
        // it. `ring()` returns only one, so we return the compute one;
        // the preview ring's capacity is large enough that overflow during
        // a single preview render is unreachable in practice.
        Some(&self.uniform_ring)
    }
}

fn ink_pen_compute_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "ink_pen_compute",
        build: |ctx| Box::new(InkPenComputePipeline::build(ctx)),
    }
}

// ── Node ────────────────────────────────────────────────────────────────

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![ink_pen_compute_pipeline_reg()],
        node: NodeRegistration {
            type_id: "ink_pen_compute",
            category: "output",
            display_name: "Ink Pen (Compute)",
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
                PortDef::input("softness", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.1)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Softness")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-feather")
                    .with_description("Edge softness (0% = hard, 100% = feathered)"),
                PortDef::input("flow", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Flow")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-droplet")
                    .exposed()
                    .with_description("Paint deposited per dab"),
                PortDef::input("color", BrushWireType::Color)
                    .with_description("Brush color"),
                PortDef::input("opacity", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Opacity")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-fill-drip")
                    .exposed()
                    .with_description("Stroke-level opacity cap"),
                PortDef::output("dab_size", BrushWireType::Vec2)
                    .with_description("Brush mark size in canvas pixels (for spacing/save-points)"),
            ],
            params: &[],
            is_gpu: true,
        },
    }
}

pub struct InkPenComputeEvaluator;

impl InkPenComputeEvaluator {
    /// Compute the effective canvas-pixel radius for this dab from the
    /// `size_input * size` product. Mirrors `stamp::compute_dab_dims` for
    /// the round-tip case (no aspect ratio, no tip texture).
    fn effective_radius(ctx: &EvalContext) -> f32 {
        let size_input = ctx.input_f32("size_input").max(0.0);
        let size = ctx.input_f32("size").max(0.0);
        let effective_size = size_input * size;
        // Diameter in canvas pixels → radius is half of that.
        (effective_size * SIZE_REFERENCE_PX * 0.5).max(0.5)
    }
}

impl BrushNodeEvaluator for InkPenComputeEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        // No paint target → nothing to do (e.g. preview mode without
        // scratch). The default `render_preview` would have been called
        // instead — but defensive early-out anyway.
        let Some(paint_target) = gpu.paint_target.as_ref() else {
            return vec![];
        };

        let position = ctx.input("position").as_vec2();
        let radius = Self::effective_radius(ctx);
        let softness = ctx.input_f32("softness").clamp(0.0, 1.0);
        let flow = ctx.input_f32("flow").clamp(0.0, 1.0);
        let mut color = ctx.input("color").as_color();
        // Premultiply alpha + fold in per-dab flow so the shader doesn't
        // have to do either. Matches the convention `composite.wgsl`
        // expects from the stamp output.
        color[3] *= flow;
        color[0] *= color[3];
        color[1] *= color[3];
        color[2] *= color[3];

        let diameter = radius * 2.0;
        if diameter <= 0.0 || color[3] <= 0.0 {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        }

        // Layer-clip the dab's canvas bbox so out-of-layer dabs are
        // handled cleanly (zero pixels written) and the save-point bbox
        // tracks the actual damage. Mirrors `prepare_dab_canvas_copy`
        // without the read-mirror sync (compute path doesn't need it).
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

        // Save-point bbox in canvas coords (Storage Frame Rule).
        let bbox_x = cx0.floor() as i32;
        let bbox_y = cy0.floor() as i32;
        let bbox_w = (cx1.ceil() as i32 - bbox_x) as u32;
        let bbox_h = (cy1.ceil() as i32 - bbox_y) as u32;
        gpu.push_dab_write_bbox(crate::coord::CanvasRect::from_xywh(
            bbox_x, bbox_y, bbox_w, bbox_h,
        ));

        // Layer-local row range — feeds the per-phase buffer→texture
        // sync at flush time.
        let local_y0 = (bbox_y - canvas_ext.y0()).max(0) as u32;
        let local_y1 = local_y0 + bbox_h;
        gpu.pending_dabs_row_range = Some(match gpu.pending_dabs_row_range {
            Some([y0, y1]) => [y0.min(local_y0), y1.max(local_y1)],
            None => [local_y0, local_y1],
        });

        gpu.pending_dabs.push(DabComputeRecord {
            pos: position,
            radius,
            softness,
            color,
        });

        // `dab_size` output drives stroke-engine spacing + save-points.
        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    /// At stroke start (and on rewind boundaries): clear the scratch
    /// texture AND the compute scratch buffer so both views agree on
    /// "empty". Drops any leftover compute queue from a prior context.
    fn begin_stroke(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let scratch = gpu
            .scratch
            .as_deref_mut()
            .expect("ink_pen_compute::begin_stroke requires Scratch");
        // 1) Allocate the compute buffer if needed (lazy on first use,
        //    re-allocated by `Scratch::grow_write` after a layer grow).
        scratch.ensure_compute_buffer(gpu.device);
        // 2) Clear both sides to transparent.
        scratch.clear_compute_buffer(&mut gpu.encoder);
        let _ = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ink_pen_compute-begin_stroke"),
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
        // 3) Reset any stale dab queue from the previous context (this
        //    is a brand-new context, so this is just defensive).
        gpu.pending_dabs.clear();
        gpu.pending_dabs_row_range = None;
    }

    /// Phase-end batch dispatch. Called by `BrushGraphRunner::flush_compute`
    /// at the end of every `render_from_stabilized_*` phase, just before
    /// `submit_final`. Drains `gpu.pending_dabs` into one (or more, if
    /// the queue exceeds the buffer capacity) compute dispatch(es) and
    /// syncs the result back to the scratch texture.
    fn flush_compute(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.pending_dabs.is_empty() {
            return;
        }
        let t_dispatch = web_time::Instant::now();

        let total_dabs = gpu.pending_dabs.len() as u32;
        let row_range = gpu.pending_dabs_row_range.unwrap_or([0, 0]);
        let union_y0 = row_range[0];
        let union_y1 = row_range[1];
        let union_h = union_y1.saturating_sub(union_y0);

        let pipeline_ref = gpu
            .pipelines
            .get::<InkPenComputePipeline>("ink_pen_compute");
        // Scratch buffer bind group: rebuilt per dispatch because the
        // underlying buffer can be reallocated after a layer grow.
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("ink_pen_compute::flush_compute requires Scratch");
        let Some(scratch_buf) = scratch.compute_buffer() else {
            // No compute buffer yet (no begin_stroke ran?) — drop the
            // queue rather than dispatching against an absent target.
            gpu.pending_dabs.clear();
            gpu.pending_dabs_row_range = None;
            return;
        };
        let aligned_width = scratch.compute_aligned_width();
        let (write_w, write_h) = scratch.write_dimensions();
        let scratch_bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ink-pen-compute-scratch-bg"),
            layout: pipeline_ref.scratch_bgl(),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: scratch_buf.as_entire_binding(),
            }],
        });

        // Paint-target-derived layer offsets for the canvas → layer-local
        // translation in the shader.
        let paint_target = gpu
            .paint_target
            .as_ref()
            .expect("ink_pen_compute::flush_compute requires paint_target");
        let canvas_ext = paint_target.canvas_extent();
        let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];

        // Dispatch in batches of MAX_DABS_PER_DISPATCH if the phase
        // queued more dabs than the dabs_buffer holds. In practice one
        // phase queues ~30 dabs, so the loop runs once.
        let dabs = std::mem::take(&mut gpu.pending_dabs);
        let union_origin = [0u32, union_y0];
        let union_size = [write_w, union_h];
        let blend_mode = gpu.blend_mode;

        for chunk in dabs.chunks(MAX_DABS_PER_DISPATCH as usize) {
            // Upload this batch's dab data.
            gpu.queue
                .write_buffer(&pipeline_ref.dabs_buffer, 0, bytemuck::cast_slice(chunk));

            // Write uniforms to the next ring slot.
            let uniforms = InkPenComputeUniforms {
                union_origin,
                union_size,
                layer_offset,
                layer_size: [canvas_ext.width, canvas_ext.height],
                canvas_size: [gpu.canvas_width, gpu.canvas_height],
                aligned_width,
                dab_count: chunk.len() as u32,
                blend_mode,
                _pad: 0,
            };
            let uniform_offset = pipeline_ref
                .uniform_ring
                .write(gpu.queue, bytemuck::bytes_of(&uniforms));

            // One compute pass per chunk. Single workgroup, serial
            // dab loop inside the shader.
            {
                let mut pass = gpu
                    .encoder
                    .begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("ink-pen-compute-dispatch"),
                        timestamp_writes: None,
                    });
                pass.set_pipeline(&pipeline_ref.pipeline);
                pass.set_bind_group(0, &pipeline_ref.uniform_bind_group, &[uniform_offset]);
                pass.set_bind_group(1, &pipeline_ref.dabs_bind_group, &[]);
                pass.set_bind_group(2, gpu.selection_bind_group, &[]);
                pass.set_bind_group(3, &scratch_bind_group, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
        }

        // Sync the compute buffer's union rows back to the scratch
        // texture so the upcoming `commit` (or any other fragment-path
        // consumer this phase) sees current state.
        let t_sync = web_time::Instant::now();
        if union_h > 0 && write_h > 0 {
            scratch.sync_compute_buffer_to_texture(&mut gpu.encoder, union_y0, union_h);
        }
        gpu.perf
            .record_compute_buffer_sync(t_sync.elapsed().as_micros() as u64);

        gpu.perf.record_compute_dispatch_batch(total_dabs);
        gpu.perf
            .record_compute_dispatch(t_dispatch.elapsed().as_micros() as u64);

        gpu.pending_dabs_row_range = None;
        // `dabs` is dropped here (we already `mem::take`'d it into a local).
    }

    /// Composite the accumulated scratch onto the pre-stroke layer
    /// snapshot — same path color_output uses. The compute work has
    /// already synced its results into the scratch texture (via
    /// `flush_compute` at every preceding phase end), so this just runs
    /// the existing fragment-path commit unchanged.
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
        );
    }

    /// Hover preview. The user has no stamp/circle node in the ink-pen-
    /// compute graph, so we render a procedural soft disc directly into
    /// the preview mask. Sized to the brush's effective canvas-pixel
    /// extent so the overlay primitive scales it correctly.
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

        let radius = Self::effective_radius(ctx);
        let softness = ctx.input_f32("softness").clamp(0.0, 1.0);

        let pipeline_ref = gpu
            .pipelines
            .get::<InkPenComputePipeline>("ink_pen_compute");
        let uniforms = PreviewUniforms {
            softness,
            _pad: [0.0; 3],
        };
        let offset = pipeline_ref
            .preview_ring
            .write(gpu.queue, bytemuck::bytes_of(&uniforms));

        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ink-pen-compute-preview"),
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
        pass.set_pipeline(&pipeline_ref.preview_pipeline);
        pass.set_bind_group(0, &pipeline_ref.preview_uniform_bind_group, &[offset]);
        pass.draw(0..3, 0..1);
        drop(pass);

        if gpu.brush_preview_info.is_none() {
            gpu.brush_preview_info = Some(crate::brush::eval::BrushPreviewInfo {
                half_extent_canvas_px: [radius, radius],
                rotation_rad: 0.0,
            });
        }
        vec![]
    }
}
