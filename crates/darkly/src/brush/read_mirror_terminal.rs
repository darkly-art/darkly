//! Shared infrastructure for per-dab fragment-pass terminals that read
//! the scratch read mirror, transform it, and write it back —
//! [`smudge`](super::nodes::smudge), [`liquify`](super::nodes::liquify),
//! and [`blur`](super::nodes::blur).
//!
//! All three share one mechanical shape: each dab samples the scratch
//! read mirror (bound at `@group(3)`), produces a new pixel, and writes
//! it straight back under REPLACE blend. Dabs run one render pass each
//! (`i..i+1`) with a `copy_texture_to_texture` between them, so every dab
//! sees the prior dab's output through the mirror — the implicit barrier
//! that makes the per-dab serialization real. The only thing that varies
//! per terminal is *how* the fragment shader transforms the sample and
//! how wide a read region each dab needs; everything else — the per-brush
//! pipeline, the dab-meta queue, the flush loop, the `copy_origin`
//! plumbing, the cursor preview — is identical and lives here.
//!
//! A terminal opts in by implementing [`ReadMirrorTerminal`] (its read
//! half-extent math + variant WGSL) and delegating each
//! [`BrushNodeEvaluator`](crate::brush::eval::BrushNodeEvaluator) method
//! to the free functions below.
//!
//! ## Blend state
//!
//! REPLACE — the fragment shader fully composes its output and writes it
//! straight to scratch. `LoadOp::Load` keeps prior scratch pixels intact
//! outside the dab footprint; the fragment shader discards past
//! `d.bbox_target_px`.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::brush::eval::EvalContext;
use crate::brush::gpu_context::{BrushGpuContext, MAX_DABS_PER_PHASE};
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipeline::{
    BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};
use crate::brush::wgsl::{
    pack_intrinsic_uniforms, pack_uniforms, CompileWgslCtx, CompiledBrush, DabField, NodeWgsl,
    WgslType, INTRINSIC_UNIFORMS_SIZE,
};
use crate::brush::wire::ScalarValue;

// ── Constants ───────────────────────────────────────────────────────────

const SIZE_REFERENCE_PX: f32 = crate::brush::DAB_REFERENCE_SIZE as f32;

const MAX_UNIFORM_BYTES: usize = 1024;

/// The `@group(3)` scratch read-mirror bindings every read-mirror
/// terminal samples. Owned here so variants never declare the layout
/// themselves — the per-brush pipeline below must match it exactly.
const SCRATCH_MIRROR_BINDINGS: &str =
    "@group(3) @binding(0) var scratch_mirror_tex: texture_2d<f32>;\n\
     @group(3) @binding(1) var scratch_mirror_smp: sampler;\n";

// ── Per-variant surface ─────────────────────────────────────────────────

/// The only per-terminal surface. A read-mirror terminal supplies its
/// read half-extent (how wide a mirror snapshot each dab needs) and its
/// variant WGSL; the free functions below own everything else.
pub trait ReadMirrorTerminal {
    /// Registry id of this terminal's [`ReadMirrorPipeline`] —
    /// `"smudge"` | `"liquify"` | `"blur"`.
    const PIPELINE_ID: &'static str;
    /// Human-readable label prefix for GPU debug labels.
    const LABEL: &'static str;

    /// Desired read half-extent (canvas px, per axis), or `None` to drop
    /// this dab before it reaches the queue. `None` is the terminal's
    /// early-out: a stationary smudge, a sub-threshold liquify push, a
    /// zero-strength blur — all collapse to an identity write, so the
    /// per-dab pass and its mirror copy are pure waste.
    ///
    /// The framework clamps the returned half-extent up to at least the
    /// dab's write footprint (`bbox_radius`), so a terminal may return a
    /// value smaller than the footprint without breaking the
    /// read-encloses-write invariant.
    fn read_half(&self, ctx: &EvalContext, radius: f32, bbox_radius: f32) -> Option<[f32; 2]>;

    /// Insert any extra per-dab `slot_outputs` this terminal's WGSL reads
    /// through a [`DabField`] (e.g. blur's `blur_px`). Called immediately
    /// before `queue_dab`, so the value packs into *this* dab's record —
    /// the same ordering `copy_origin` relies on. Default: no extra slots.
    fn pack_extra(
        &self,
        _ctx: &EvalContext,
        _gpu: &mut BrushGpuContext,
        _node_id: &str,
        _radius: f32,
    ) {
    }

    /// Variant WGSL: the fragment body, plus any module-scope `decls` and
    /// extra `dab_fields` the body references. The wrapper appends the
    /// shared `copy_origin` dab field and owns `terminal_bindings`; the
    /// variant must **not** set `terminal_bindings`. `copy_origin_field`
    /// is the dab-record field name to read the mirror snapshot origin
    /// from.
    fn compile_body(
        &self,
        cctx: &CompileWgslCtx,
        copy_origin_field: &str,
    ) -> Result<NodeWgsl, String>;

    /// Variant preview-mode body — the cursor footprint, sampling no
    /// `@group(3)` bindings (preview omits them).
    fn compile_cursor_preview_body(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String>;
}

// ── Per-dab CPU meta ────────────────────────────────────────────────────

/// CPU-side per-dab footprint info, packed by [`evaluate_gpu`] in
/// lockstep with the GPU dab record and drained by [`flush_dabs`]. Lets
/// the flush loop call `prepare_dab_canvas_copy` without re-deriving the
/// footprint from the upload buffer.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ReadMirrorDabMeta {
    position: [f32; 2],
    /// Half-extent of the write region (the dab footprint).
    write_half: [f32; 2],
    /// Half-extent of the read region (the mirror snapshot the shader
    /// samples). Always encloses `write_half`.
    read_half: [f32; 2],
}

const READ_MIRROR_DAB_META_SIZE: usize = std::mem::size_of::<ReadMirrorDabMeta>();

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
    fn build(ctx: &BuildContext, compiled: &CompiledBrush, label: &str) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&format!("{label}-brush")),
                source: wgpu::ShaderSource::Wgsl(compiled.stroke_wgsl.clone().into()),
            });

        let dabs_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(&format!("{label}-dabs-bgl")),
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
        // same layout as `watercolor`'s atlas binding, only the binding
        // semantics differ.
        let layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(&format!("{label}-layout")),
                bind_group_layouts: &[
                    Some(ctx.uniform_bgl),
                    Some(&dabs_bgl),
                    Some(ctx.selection_bgl),
                    Some(ctx.canvas_copy_bgl),
                ],
                immediate_size: 0,
            });

        // REPLACE blend — the fragment shader writes the final pixel;
        // outside the disc it discards so LoadOp::Load preserves the
        // scratch.
        let pipeline = ctx
            .device
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
            &format!("{label}-uniforms"),
            uniform_size as u64,
            ctx.min_uniform_align,
        );
        let uniform_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{label}-uniform-bg")),
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
            label: Some(&format!("{label}-dabs-buffer")),
            size: dabs_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dabs_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{label}-dabs-bg")),
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

/// Per-brush pipeline cache shared by every read-mirror terminal. One
/// instance is registered per terminal id (`"smudge"`, `"liquify"`,
/// `"blur"`); they are the same Rust type, distinguished only by their
/// registry key.
pub struct ReadMirrorPipeline {
    cache: RefCell<HashMap<u64, PerBrushPipeline>>,
}

impl ReadMirrorPipeline {
    fn build(_ctx: &BuildContext) -> Self {
        Self {
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn ensure_pipeline(&self, ctx: &BuildContext, compiled: &CompiledBrush, label: &str) {
        let mut cache = self.cache.borrow_mut();
        cache
            .entry(compiled.topology_hash)
            .or_insert_with(|| PerBrushPipeline::build(ctx, compiled, label));
    }

    fn with_pipeline<R>(&self, hash: u64, f: impl FnOnce(&PerBrushPipeline) -> R) -> R {
        let cache = self.cache.borrow();
        let p = cache
            .get(&hash)
            .expect("ensure_pipeline must run before with_pipeline");
        f(p)
    }
}

impl BrushPipelineEntry for ReadMirrorPipeline {
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

/// Build a pipeline registration for a read-mirror terminal under the
/// given registry id. Drop one of these in a terminal's `register()`
/// `pipelines` list.
pub fn read_mirror_pipeline_reg(id: &'static str) -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id,
        build: |ctx| Box::new(ReadMirrorPipeline::build(ctx)),
    }
}

// ── Shared helpers ───────────────────────────────────────────────────────

/// Effective dab radius in canvas pixels: the stroke's ambient base size
/// (`pen_input.size`, via [`EvalContext::base_size`]) times this terminal's
/// per-touch `size` modulation. Floored at 0.5 px so a dab always has
/// positive area. Shared by every terminal — `paint` and `watercolor`
/// delegate to it, and the read-mirror terminals (blur/smudge/liquify) call
/// it through this module.
pub fn effective_radius(ctx: &EvalContext) -> f32 {
    let modulation = ctx.input_f32("size").max(0.0);
    (ctx.base_size() * modulation * SIZE_REFERENCE_PX * 0.5).max(0.5)
}

/// Insert a per-dab value into `dab_batch.slot_outputs` under this node's
/// dab-field key (`n{node_id}_{base}`), so the framework's
/// `pack_dab_record` picks it up via the matching [`DabField`] declared
/// in [`compile_wgsl`]. Used for `copy_origin` (always) and any terminal
/// extras (e.g. blur's `blur_px`, via [`ReadMirrorTerminal::pack_extra`]).
pub fn insert_slot_output(
    gpu: &mut BrushGpuContext,
    node_id: &str,
    base: &str,
    value: ScalarValue,
) {
    if let Some(outputs) = gpu.dab_batch.slot_outputs.as_mut() {
        outputs.insert(format!("n{}_{}", node_id, base), value);
    }
}

/// Per-dab `evaluate_gpu`. Computes the dab geometry, calls
/// [`ReadMirrorTerminal::read_half`] (dropping the dab on `None`), clamps
/// the footprint to the layer extent, records the write bbox + bbox
/// union, pre-computes `copy_origin`, then packs the dab record + meta in
/// lockstep. Returns `dab_size` for downstream spacing.
pub fn evaluate_gpu<T: ReadMirrorTerminal>(
    term: &T,
    ctx: &EvalContext,
    gpu: &mut BrushGpuContext,
) -> Vec<(String, ScalarValue)> {
    let Some(compiled) = gpu.dab_batch.compiled_brush.clone() else {
        debug_assert!(false, "{} requires compiled_brush on gpu_context", T::LABEL);
        return vec![];
    };
    let Some(stroke) = gpu.stroke.as_ref() else {
        return vec![];
    };
    let paint_target = &stroke.paint_target;
    let position = ctx.input("position").as_vec2();
    let radius = effective_radius(ctx);
    let diameter = radius * 2.0;
    let dab_size = || vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
    if diameter <= 0.0 {
        return dab_size();
    }

    // Per-brush extent: composed by the framework at compile time. For a
    // plain disc upstream this is `radius`.
    let bbox_radius = radius * compiled.brush_extent_factor + compiled.brush_extent_extra_px;

    // Terminal's desired read half-extent — `None` is the early-out for
    // dabs whose transform is an identity write.
    let Some(read_half) = term.read_half(ctx, radius, bbox_radius) else {
        return dab_size();
    };

    let canvas_ext = paint_target.canvas_extent();
    // Near-edge of the layer extent — reused below for the read-region
    // copy origin (a one-sided clamp, distinct from the dab footprint
    // clamp).
    let layer_x0 = canvas_ext.x0() as f32;
    let layer_y0 = canvas_ext.y0() as f32;
    // Clamp the dab footprint to the layer extent; a dab entirely
    // off-extent has no pixels to draw and is skipped.
    let canvas_bbox = match canvas_ext.clamp_f32(
        position[0] - bbox_radius,
        position[1] - bbox_radius,
        position[0] + bbox_radius,
        position[1] + bbox_radius,
    ) {
        Some(r) => r,
        None => return dab_size(),
    };
    let local = paint_target
        .canvas_frame()
        .canvas_to_layer_rect(canvas_bbox)
        .expect("canvas_bbox came from canvas_ext.clamp_f32, so it overlaps the extent");
    gpu.dab_batch.push_write_bbox(canvas_bbox);
    gpu.dab_batch.bbox = Some(match gpu.dab_batch.bbox {
        Some([x0, y0, x1, y1]) => [
            x0.min(local.x0()),
            y0.min(local.y0()),
            x1.max(local.x1()),
            y1.max(local.y1()),
        ],
        None => [local.x0(), local.y0(), local.x1(), local.y1()],
    });

    // The write region is the dab footprint; the read region is the
    // mirror snapshot. Clamp the read half up to at least the write half
    // per axis so `prepare_dab_canvas_copy`'s read-encloses-write
    // debug-assert always holds, even when `bbox_radius` dominates a tiny
    // requested read extent.
    let write_half = [bbox_radius, bbox_radius];
    let read_half = [
        read_half[0].max(write_half[0]),
        read_half[1].max(write_half[1]),
    ];

    // Pre-compute `copy_origin` (canvas-space top-left of the mirror
    // snapshot) with the same formula `prepare_dab_canvas_copy` will use
    // at flush time; the per-dab record carries it so the shader can map
    // `target_pos` into mirror UVs.
    let read_x0 = (position[0] - read_half[0]).max(layer_x0);
    let read_y0 = (position[1] - read_half[1]).max(layer_y0);
    let copy_origin = [read_x0.floor(), read_y0.floor()];
    let node_id = ctx.node_id.as_str();
    insert_slot_output(gpu, node_id, "copy_origin", ScalarValue::Vec2(copy_origin));

    // Extra per-dab slots (blur's kernel size, …) must be inserted before
    // `queue_dab` packs the record.
    term.pack_extra(ctx, gpu, node_id, radius);

    gpu.dab_batch
        .queue_dab(&compiled, position, bbox_radius, radius);

    // Pack the CPU-side meta in lockstep with the GPU record.
    let meta = ReadMirrorDabMeta {
        position,
        write_half,
        read_half,
    };
    gpu.dab_batch
        .meta_bytes
        .extend_from_slice(bytemuck::bytes_of(&meta));

    dab_size()
}

/// Per-phase flush. Walks the dab-meta queue in lockstep: for each dab it
/// syncs the mirror snapshot (`prepare_dab_canvas_copy`, whose
/// `copy_texture_to_texture` carries the implicit barrier that lets each
/// draw see the prior draw's output) and issues one render pass with
/// instance index `i..i+1`.
pub fn flush_dabs<T: ReadMirrorTerminal>(gpu: &mut BrushGpuContext) {
    if gpu.dab_batch.count == 0 {
        return;
    }
    let Some(compiled) = gpu.dab_batch.compiled_brush.clone() else {
        debug_assert!(false, "{}::flush_dabs requires compiled_brush", T::LABEL);
        return;
    };

    let bbox = gpu.dab_batch.bbox.unwrap_or([0, 0, 0, 0]);
    let union_w = bbox[2].saturating_sub(bbox[0]);
    let union_h = bbox[3].saturating_sub(bbox[1]);
    let (dab_bytes, total_dabs) = gpu.dab_batch.take();
    let meta_bytes = gpu.dab_batch.take_meta();
    if total_dabs == 0 {
        return;
    }
    debug_assert_eq!(
        meta_bytes.len(),
        (total_dabs as usize) * READ_MIRROR_DAB_META_SIZE,
        "{} meta queue out of sync with dab queue",
        T::LABEL
    );
    let metas: Vec<ReadMirrorDabMeta> = bytemuck::cast_slice(&meta_bytes).to_vec();
    gpu.perf
        .record_dab_flush_workload(total_dabs, union_w, union_h);

    let pipeline_ref = gpu.pipelines.get::<ReadMirrorPipeline>(T::PIPELINE_ID);
    ensure_per_brush_pipeline(gpu, pipeline_ref, &compiled, T::LABEL);

    let stroke = gpu
        .stroke
        .as_ref()
        .expect("read-mirror flush_dabs requires stroke resources");
    let paint_target = &stroke.paint_target;
    let canvas_ext = paint_target.canvas_extent();
    let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];
    let layer_size = [canvas_ext.width, canvas_ext.height];

    let mut uniform_bytes: Vec<u8> = Vec::with_capacity(MAX_UNIFORM_BYTES);
    pack_intrinsic_uniforms(
        &mut uniform_bytes,
        gpu.intrinsic_header(layer_offset, layer_size),
    );
    let outputs = gpu
        .dab_batch
        .slot_outputs
        .as_ref()
        .expect("read-mirror flush_dabs requires dab_batch.slot_outputs");
    pack_uniforms(&compiled, outputs, &mut uniform_bytes);

    let pass_label = format!("{}-flush", T::LABEL);
    pipeline_ref.with_pipeline(compiled.topology_hash, |per_brush| {
        if uniform_bytes.len() < per_brush.uniform_size {
            uniform_bytes.resize(per_brush.uniform_size, 0);
        }
        per_brush.uniform_ring.reset();
        let uniform_offset = per_brush.uniform_ring.write(gpu.queue, &uniform_bytes);
        gpu.queue
            .write_buffer(&per_brush.dabs_buffer, 0, &dab_bytes);

        for (i, meta) in metas.iter().enumerate() {
            // Invalidate the per-dab read-mirror origin cache so this dab
            // re-copies the scratch even when it shares an origin with the
            // previous dab. Without this, two dabs at the same spot would
            // reuse the prior snapshot and the dab would read stale pixels
            // instead of the previous dab's writeback — which would break
            // dwell-compounding (scrub-in-place re-blur / re-smear) and the
            // per-dab barrier for any same-origin pair.
            if let Some(stroke) = gpu.stroke.as_mut() {
                stroke.reset_per_dab_read_cache();
            }

            // Sync the mirror snapshot for this dab. The implicit barrier
            // from this `copy_texture_to_texture` makes the subsequent
            // render pass see prior dab writes.
            let _ = gpu.prepare_dab_canvas_copy(
                meta.position,
                meta.write_half[0],
                meta.write_half[1],
                meta.read_half[0],
                meta.read_half[1],
            );

            // Fresh read-mirror bind group each iteration — a mid-loop
            // grow can rebuild it.
            let scratch_ref = &*gpu
                .stroke
                .as_ref()
                .expect("read-mirror flush_dabs requires stroke resources")
                .scratch;
            let read_bg = scratch_ref.read_mirror_bind_group();
            let write_view = scratch_ref.write_view();

            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(&pass_label),
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

/// Direct blit scratch → layer. The scratch already holds the finished
/// image; commit just copies it across. `gpu.blend_mode` is ignored —
/// erase semantics aren't meaningful for these read-back transforms.
pub fn commit(gpu: &mut BrushGpuContext) {
    let Some(stroke) = gpu.stroke.as_ref() else {
        return;
    };
    stroke.paint_target.commit_scratch_blit(
        gpu.device,
        &mut gpu.encoder,
        gpu.pipelines,
        stroke.scratch.write_view(),
        stroke.scratch.write_texture(),
    );
}

/// Hover-cursor preview. Routes through the shared preview helper at the
/// dab's effective radius.
pub fn render_cursor_preview(
    ctx: &EvalContext,
    gpu: &mut BrushGpuContext,
) -> Vec<(String, ScalarValue)> {
    let radius = effective_radius(ctx);
    let _ = crate::brush::wgsl::render_compiled_cursor_preview(gpu, radius);
    vec![]
}

/// Assemble the terminal's `compile_wgsl` output: the variant body +
/// decls, plus the shared `copy_origin` dab field and the `@group(3)`
/// mirror bindings. The variant must not set `terminal_bindings`.
pub fn compile_wgsl<T: ReadMirrorTerminal>(
    term: &T,
    cctx: &CompileWgslCtx,
) -> Result<NodeWgsl, String> {
    let copy_origin_field = cctx.dab_field_name("copy_origin");
    let mut wgsl = term.compile_body(cctx, &copy_origin_field)?;

    debug_assert!(
        wgsl.terminal_bindings.is_empty(),
        "read-mirror variant must not set terminal_bindings — the wrapper owns @group(3)",
    );

    // Shared per-dab `copy_origin` field. The terminal's `evaluate_gpu`
    // inserts the value into `dab_batch.slot_outputs`; the packer reads
    // it through the standard `pack_dab_record` path.
    let key = copy_origin_field.clone();
    wgsl.dab_fields.push(DabField {
        name: copy_origin_field,
        ty: WgslType::Vec2,
        pack: Arc::new(move |outputs, bytes| {
            let v = outputs.get(&key).map(|s| s.as_vec2()).unwrap_or([0.0; 2]);
            bytes.extend_from_slice(bytemuck::bytes_of(&v));
        }),
    });

    wgsl.terminal_bindings = SCRATCH_MIRROR_BINDINGS.to_string();

    Ok(wgsl)
}

// ── Per-brush pipeline build helper ─────────────────────────────────────

fn ensure_per_brush_pipeline(
    gpu: &BrushGpuContext,
    pipe: &ReadMirrorPipeline,
    compiled: &CompiledBrush,
    label: &str,
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
        texture_registry: gpu.pipelines.texture_registry(),
        baked_sources: gpu.pipelines.baked_sources(),
    };
    pipe.ensure_pipeline(&ctx, compiled, label);
}

#[cfg(test)]
mod tests {
    use super::effective_radius;
    use crate::brush::eval::EvalContext;
    use crate::brush::wire::BrushWireType;
    use crate::nodegraph::{NodeId, PortDef};

    fn ctx_with<'a>(port_defs: &'a [PortDef<BrushWireType>], base_size: f32) -> EvalContext<'a> {
        static TEST_NODE_ID: std::sync::OnceLock<NodeId> = std::sync::OnceLock::new();
        EvalContext {
            input_slots: &[],
            input_values: &[],
            port_defs,
            lut: None,
            stroke_seed: 0,
            dab_index: 0,
            base_size,
            dabs_per_pass: 1.0,
            node_id: TEST_NODE_ID.get_or_init(|| NodeId("test".into())),
        }
    }

    /// Behavior-preservation regression: `effective_radius` is exactly
    /// `base_size × modulation × DAB_REFERENCE_SIZE × 0.5` (floored 0.5) — the
    /// same product the old `size_input × size` model produced, with the base
    /// now supplied by the ambient `pen_input.size` and the terminal port
    /// carrying only the per-touch modulation.
    #[test]
    fn effective_radius_is_base_times_modulation() {
        let ref_px = crate::brush::DAB_REFERENCE_SIZE as f32;

        // Modulation defaults to 1.0 (unwired terminal `size` port).
        let mod_default = [PortDef::input("size", BrushWireType::Scalar).with_range(0.0, 1.0, 1.0)];
        for base in [0.1_f32, 0.3, 2.0] {
            let got = effective_radius(&ctx_with(&mod_default, base));
            assert!(
                (got - (base * ref_px * 0.5).max(0.5)).abs() < 1e-3,
                "base {base}: got {got}",
            );
        }

        // A modulation ≠ 1.0 multiplies onto the base: base 0.4 × mod 0.5 must
        // equal the old `size=0.4, size_input=0.5`.
        let mod_half = [PortDef::input("size", BrushWireType::Scalar).with_range(0.0, 1.0, 0.5)];
        let got = effective_radius(&ctx_with(&mod_half, 0.4));
        assert!((got - (0.4 * 0.5 * ref_px * 0.5)).abs() < 1e-3, "got {got}");

        // Floor at 0.5 px so a zero-size dab still has positive area.
        let mod_zero = [PortDef::input("size", BrushWireType::Scalar).with_range(0.0, 1.0, 0.0)];
        assert_eq!(effective_radius(&ctx_with(&mod_zero, 0.0)), 0.5);
    }
}
