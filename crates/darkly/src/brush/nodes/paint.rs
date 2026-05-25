//! Paint terminal — single-pass instanced fragment for Basic brushes
//! (Round, Airbrush, Ink Pen).
//!
//! ## What this terminal does
//!
//! Each `evaluate_gpu` call (one per dab placement) only **queues** a
//! `PaintDabRecord` on the shared dab queue. No render passes are opened
//! per dab. At the end of the rendering phase, the runner's `flush_dabs`
//! hook fires; this terminal's `flush_dabs` opens **one** render pass on
//! the scratch texture and issues a single instanced draw — six vertices,
//! N instances. The hardware blend stage handles per-dab source-over
//! (paint) or destination-out (erase) ordering across instances.
//!
//! ## Alpha convention
//!
//! Output is **premultiplied** — the shader emits `dab.color * coverage`
//! and the pipeline blend state is `(One, OneMinusSrcAlpha, Add)` on
//! both color and alpha, which is the canonical hardware premultiplied
//! source-over. The scratch texture (`Scratch::write_texture`) is the
//! same one other terminals treat as straight-alpha; during a Basic-brush
//! stroke the convention is internal to this terminal, and the `commit`
//! hook below sets `fg_premultiplied: true` so the composite shader
//! interprets the scratch correctly when blitting it onto the layer.
//! See `compositing-lessons-learned.md` §4 for why hardware source-over
//! requires a premultiplied destination.
//!
//! ## Why this is the only terminal we ship for Basic brushes
//!
//! - vs per-dab fragment passes (#1 in `paint-compute-perf-tracking.md`):
//!   one pass with N instances eliminates per-dab `begin_render_pass`
//!   overhead, which dominated small-radius cells.
//! - vs compute (#2/#3): no buffer round-trip. Hardware ROP writes the
//!   scratch directly, so per-flush cost scales with `Σ(dab_area)` —
//!   actually-rasterized pixels — instead of `union_bbox_area`.

use std::any::Any;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::{BrushGpuContext, PaintDabRecord, MAX_DABS_PER_PHASE};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipeline::{
    BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

/// Canvas-pixel reference for `size_input * size = 1.0`. Mirrors stamp's
/// `DAB_REFERENCE_SIZE` so the user's "Size" slider feels the same as
/// the stamp-based brushes.
const SIZE_REFERENCE_PX: f32 = crate::brush::dab_pool::DAB_REFERENCE_SIZE as f32;

// ── Uniforms ────────────────────────────────────────────────────────────

/// Per-flush uniforms. Layout MUST match `Uniforms` in
/// `shaders/brush/paint.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PaintUniforms {
    layer_offset: [i32; 2],
    layer_size: [u32; 2],
    canvas_size: [u32; 2],
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PreviewUniforms {
    softness: f32,
    _pad: [f32; 3],
}

// ── Pipeline ────────────────────────────────────────────────────────────

pub struct PaintPipeline {
    /// Source-over (paint) pipeline. Premultiplied blend.
    paint_pipeline: wgpu::RenderPipeline,
    /// Destination-out (erase) pipeline. Removes coverage from the
    /// scratch's existing alpha; rgb is untouched.
    erase_pipeline: wgpu::RenderPipeline,
    /// group(0) — per-flush uniform ring (dynamic offset).
    uniform_ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
    /// group(1) — dab storage buffer. Pre-allocated to
    /// `MAX_DABS_PER_PHASE * sizeof(PaintDabRecord)` bytes. Re-written
    /// per flush via `queue.write_buffer`.
    dabs_buffer: wgpu::Buffer,
    dabs_bind_group: wgpu::BindGroup,
    /// Procedural disc preview: tiny fragment shader that draws a soft
    /// disc so the hover cursor still shows the brush footprint. Same
    /// shape as `paint_compute`'s preview path so the cursor feel is
    /// preserved.
    ///
    /// Uses an in-place uniform buffer (NOT a `DynamicUniformRing`) —
    /// only one live uniform per preview submit, no ring required.
    preview_pipeline: wgpu::RenderPipeline,
    preview_uniform_buffer: wgpu::Buffer,
    preview_uniform_bind_group: wgpu::BindGroup,
}

impl PaintPipeline {
    fn build(ctx: &BuildContext) -> Self {
        // ── Paint/erase pipelines ────────────────────────────────────
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("paint"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../../../shaders/brush/paint.wgsl").into(),
                ),
            });

        // group(1): dab storage buffer (read-only) — visible from vs+fs
        // because the vertex shader needs `dab.pos`/`dab.radius` to build
        // the per-instance quad and the fragment shader needs `dab.color`/
        // `dab.softness` for coverage and color.
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

        // Premultiplied source-over: `out.rgb = src.rgb + dst.rgb * (1 -
        // src.a); out.a = src.a + dst.a * (1 - src.a)`. Both color and
        // alpha use the same `(One, OneMinusSrcAlpha, Add)` factors —
        // this is the canonical hardware premultiplied source-over per
        // `compositing-lessons-learned.md` §4.
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
        // Destination-out (erase): `out.rgb = dst.rgb * (1 - src.a)`,
        // `out.a = dst.a * (1 - src.a)`. The fragment shader emits the
        // dab's premultiplied color (rgb already scaled by alpha), so
        // multiplying `dst` by `(1 - src.a)` reduces both rgb and alpha
        // by the coverage. Source contribution is zero — only the
        // destination's existing pixels are scaled down.
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
        let paint_pipeline = make_pipeline("paint", paint_blend);
        let erase_pipeline = make_pipeline("paint-erase", erase_blend);

        let (uniform_ring, uniform_bind_group) =
            ctx.make_uniform_ring::<PaintUniforms>("paint-uniforms", "paint-uniform-bg");

        let dabs_buffer_size =
            (MAX_DABS_PER_PHASE as u64) * (std::mem::size_of::<PaintDabRecord>() as u64);
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

        // ── Preview pipeline (procedural disc) ───────────────────────
        // Lifted unchanged from paint_compute so the hover cursor matches
        // the existing Basic-brush feel.
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
    let d = distance(in.uv, vec2<f32>(0.5, 0.5)) * 2.0;
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
                label: Some("paint-preview"),
                source: wgpu::ShaderSource::Wgsl(preview_shader_src.into()),
            });
        let preview_uniform_bgl =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("paint-preview-uniform-bgl"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });
        let preview_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("paint-preview-layout"),
                bind_group_layouts: &[&preview_uniform_bgl],
                immediate_size: 0,
            });
        let preview_pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("paint-preview"),
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
        let preview_uniform_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("paint-preview-uniform"),
            size: std::mem::size_of::<PreviewUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let preview_uniform_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint-preview-uniform-bg"),
            layout: &preview_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: preview_uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            paint_pipeline,
            erase_pipeline,
            uniform_ring,
            uniform_bind_group,
            dabs_buffer,
            dabs_bind_group,
            preview_pipeline,
            preview_uniform_buffer,
            preview_uniform_bind_group,
        }
    }
}

impl BrushPipelineEntry for PaintPipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        Some(&self.uniform_ring)
    }
}

fn paint_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "paint",
        build: |ctx| Box::new(PaintPipeline::build(ctx)),
    }
}

// ── Node ────────────────────────────────────────────────────────────────

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![paint_pipeline_reg()],
        node: NodeRegistration {
            type_id: "paint",
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
                PortDef::input("softness", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.1)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Softness")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-feather")
                    .exposed()
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

pub struct PaintEvaluator;

impl PaintEvaluator {
    /// Effective canvas-pixel radius from the `size_input * size` product.
    /// Same formula as `paint_compute` so a brush built around `paint`
    /// feels identical to one built around `paint_compute` at the same
    /// port values.
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
        let Some(paint_target) = gpu.paint_target.as_ref() else {
            return vec![];
        };

        let position = ctx.input("position").as_vec2();
        let radius = Self::effective_radius(ctx);
        let softness = ctx.input_f32("softness").clamp(0.0, 1.0);
        let flow = ctx.input_f32("flow").clamp(0.0, 1.0);
        let mut color = ctx.input("color").as_color();
        // Premultiply alpha + fold per-dab flow into alpha so the shader
        // can blend without scaling. Same convention as `paint_compute`
        // — the dab record's `color` field is premultiplied.
        color[3] *= flow;
        color[0] *= color[3];
        color[1] *= color[3];
        color[2] *= color[3];

        let diameter = radius * 2.0;
        if diameter <= 0.0 || color[3] <= 0.0 {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        }

        // Layer-clip the dab's canvas bbox. Drives the save-point bbox
        // and the per-flush union bbox tracked on `pending_dabs_bbox`
        // (so the bench harness can report workload shape per event).
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

        gpu.queue_dab(&PaintDabRecord {
            pos: position,
            radius,
            softness,
            color,
        });

        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    /// At stroke start (and on rewind boundaries): clear the scratch
    /// texture to transparent. Drops any leftover dab queue from a prior
    /// context. No compute buffer to allocate — this terminal writes
    /// the scratch texture directly via fragment.
    fn begin_stroke(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("paint::begin_stroke requires Scratch");
        let _ = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("paint-begin_stroke"),
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

    /// Phase-end batched draw. Called by `BrushGraphRunner::flush_dabs`
    /// at the end of every dab-rendering phase, just before
    /// `submit_final`. Drains the per-phase dab queue into **one**
    /// render pass with one instanced draw.
    fn flush_dabs(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.pending_dab_count == 0 {
            return;
        }

        let bbox = gpu.pending_dabs_bbox.unwrap_or([0, 0, 0, 0]);
        let union_w = bbox[2].saturating_sub(bbox[0]);
        let union_h = bbox[3].saturating_sub(bbox[1]);

        let (dab_bytes, total_dabs) = gpu.take_pending_dabs();
        if total_dabs == 0 {
            return;
        }

        // Record per-flush workload shape for the bench harness — same
        // counters watercolor_compute uses, so the bench cell layout
        // doesn't change.
        gpu.perf
            .record_dab_flush_workload(total_dabs, union_w, union_h);

        let pipeline_ref = gpu.pipelines.get::<PaintPipeline>("paint");
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("paint::flush_dabs requires Scratch");
        let paint_target = gpu
            .paint_target
            .as_ref()
            .expect("paint::flush_dabs requires paint_target");
        let canvas_ext = paint_target.canvas_extent();
        let layer_w = canvas_ext.width;
        let layer_h = canvas_ext.height;
        let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];

        let dabs: &[PaintDabRecord] = bytemuck::cast_slice(&dab_bytes);

        // Upload the dab records to the storage buffer. One write per
        // flush (vs per-dab in #1), N instances drain it in one draw.
        gpu.queue
            .write_buffer(&pipeline_ref.dabs_buffer, 0, bytemuck::cast_slice(dabs));

        let uniforms = PaintUniforms {
            layer_offset,
            layer_size: [layer_w, layer_h],
            canvas_size: [gpu.canvas_width, gpu.canvas_height],
            _pad: [0, 0],
        };
        let uniform_offset = pipeline_ref
            .uniform_ring
            .write(gpu.queue, bytemuck::bytes_of(&uniforms));

        let pipeline = if gpu.blend_mode == 1 {
            &pipeline_ref.erase_pipeline
        } else {
            &pipeline_ref.paint_pipeline
        };

        {
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
            pass.set_viewport(0.0, 0.0, layer_w as f32, layer_h as f32, 0.0, 1.0);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &pipeline_ref.uniform_bind_group, &[uniform_offset]);
            pass.set_bind_group(1, &pipeline_ref.dabs_bind_group, &[]);
            pass.set_bind_group(2, gpu.selection_bind_group, &[]);
            pass.draw(0..6, 0..dabs.len() as u32);
        }

        gpu.perf.record_dab_flush(total_dabs);
    }

    /// Commit accumulated scratch (premultiplied, written by this
    /// terminal during the just-finished phase) onto the layer with the
    /// stroke-level opacity cap and the engine's blend_mode.
    /// `fg_premultiplied: true` tells the composite shader that the
    /// scratch input is premultiplied, even though the same texture is
    /// straight-alpha when other terminals own the brush.
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

    /// Hover preview — procedural soft disc, same as paint_compute's
    /// preview. Basic-brush graphs have no stamp/circle node, so the
    /// terminal owns the cursor shape itself.
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

        let pipeline_ref = gpu.pipelines.get::<PaintPipeline>("paint");
        let uniforms = PreviewUniforms {
            softness,
            _pad: [0.0; 3],
        };
        gpu.queue.write_buffer(
            &pipeline_ref.preview_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("paint-preview"),
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
        pass.set_bind_group(0, &pipeline_ref.preview_uniform_bind_group, &[]);
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
