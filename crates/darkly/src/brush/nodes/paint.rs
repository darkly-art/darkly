//! Paint terminal: one compute dispatch per dab with a per-brush
//! compiled WGSL shader.
//!
//! ## What this terminal does
//!
//! Per-dab records queue up on [`BrushGpuContext::dab_batch`]; one
//! compute pass drains them at phase end, one `dispatch_workgroups` per
//! dab over the dab's layer-clamped footprint, one thread per pixel.
//!
//! - **The compute shader is generated per-brush at brush load** by
//!   walking the upstream graph and asking each node to emit WGSL.
//!   See [`crate::brush::wgsl`].
//! - **The per-dab record schema is dynamic**, sized by what fields
//!   the brush's nodes contribute. No fixed `PaintDabRecord` struct.
//! - **The uniform buffer carries stroke-constant values** from any
//!   upstream nodes that declared `uniform_fields` (e.g. `paint_color`).
//! - **The ground is the stroke scratch**, a read-write `r32uint`
//!   storage texture holding packed premultiplied RGBA8
//!   ([`PACKED_GROUND_FORMAT`]). A thread whose dab has no coverage at
//!   its pixel returns before touching any ground; every other thread
//!   loads its texel, applies the brush's accumulation law
//!   (`shaders/brush/paint_accumulate.wgsl`) and stores the packed
//!   result only where it differs from what was loaded, so a dab the
//!   wash's ceiling refuses costs a load and no store. Both are
//!   identities at the texel: a zero source leaves either law's output
//!   equal to its input, and storing an equal word changes nothing.
//!   Dispatches in a pass are ordered and a dispatch's stores are visible
//!   to the next, so every dab sees the previous dab's output. The
//!   framework allocates, clears, checkpoints, restores and grows the
//!   ground as it does any scratch.
//! - **The dab index** reaches a dispatch through a static index buffer
//!   (slot `i` holds `i`, written once at build) bound with a dynamic
//!   offset: immediates are unavailable on the `webgpu` backend, and this
//!   is the cheapest legal mechanism there.
//! - **A graph that samples the live stroke** (the stroke's appearance at
//!   other pixels, [`CompiledBrush::reads_stroke_appearance`]) gets one
//!   more dispatch before each dab's: the
//!   [`AppearanceSnapshotPipeline`] renders the grounds through the commit
//!   law into the scratch's appearance mirror under the dab's read region
//!   (its footprint grown by the graph's per-dab read reach), and the dab
//!   samples the mirror. Inside one dispatch the read would race the
//!   threads writing the texels it reads; across dispatches it is ordered.
//! - **Two laws.** Under `Deposit` the `buildup` dial splits each dab
//!   between the wash ceiling and build-up source-over. Under `Move` the
//!   dab is what a finger brought from elsewhere (a live sampler's output)
//!   and `coverage` is how much of the pixel the finger displaced: the
//!   ground lerps toward the dab by that coverage and a `coverage` channel
//!   accumulates it, so the commit lays the moved pigment over `1 -
//!   coverage` of the layer. A smear can then thin an edge as well as
//!   thicken it, which source-over, whose coverage is its own alpha,
//!   never can. The `Move` ground is stored straight, not premultiplied:
//!   a dark colour under a light touch rounds to black in a premultiplied
//!   8-bit texel (`accumulate_move`).
//!
//! Upstream nodes (`circle`, `stamp`, etc.) compile inline into the
//! compute shader and evaluate per-pixel-per-dab, with no intermediate
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
//! upstream node returns `Err` from `compile_wgsl`, brush load fails;
//! there is no runtime fallback. See
//! [`crate::brush::wgsl::CompileError`].

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroU64;

use crate::brush::appearance_snapshot::{
    AppearanceSnapshotPipeline, SnapshotRecord, SnapshotSources, APPEARANCE_SNAPSHOT_ID,
};
use crate::brush::composite_pipeline::CommitLaw;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::{BrushGpuContext, MAX_DABS_PER_PHASE};
use crate::brush::input_value::InputValue;
use crate::brush::node::{BrushNodeRegistration, DabPass, PACKED_GROUND_FORMAT};
use crate::brush::paint_target_ext::{BrushPaintTargetExt, CommitForegrounds};
use crate::brush::pipeline::{
    BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};
use crate::brush::scratch::{ChannelUse, StrokeChannel};
use crate::brush::texture_source::{LiveSource, ResolvedSource};
use crate::brush::wgsl::{
    pack_intrinsic_uniforms, pack_uniforms, CompileWgslCtx, CompiledBrush, InputBinding, NodeWgsl,
    DAB_SLOT_STRIDE, DAB_WORKGROUP, GROUND_NAME, INTRINSIC_UNIFORMS_SIZE,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

/// Maximum uniform buffer size we'll allocate per brush pipeline.
const MAX_UNIFORM_BYTES: usize = 1024;

/// The accumulation the stacking half of a dab goes into when the brush sits
/// strictly inside the dial. Its law is the one the original terminal always
/// had: every dab composites over the last, so the dabs of a pass compound
/// and a stroke builds on itself. A second packed ground beside the
/// scratch, written by the same dispatch.
///
/// Declared only between the ends, where both halves exist. At either end the
/// single scratch carries everything and no channel is allocated.
const BUILD_CHANNEL: StrokeChannel = StrokeChannel {
    name: "build",
    format: PACKED_GROUND_FORMAT,
    kind: ChannelUse::Storage,
};

/// Under the move law, how much of each pixel the finger has displaced
/// this stroke, in the packed texel's alpha (`1 - prod(1 - cov_i)`). The
/// commit lays the ground over `1 - coverage` of the layer.
const COVERAGE_CHANNEL: StrokeChannel = StrokeChannel {
    name: "coverage",
    format: PACKED_GROUND_FORMAT,
    kind: ChannelUse::Storage,
};

const MODE_DEPOSIT: i32 = 0;
const MODE_MOVE: i32 = 1;

/// The law a brush accumulates and commits under, from its `mode` port and,
/// under `Deposit`, its `buildup` dial.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PaintLaw {
    Deposit { buildup: f32 },
    Move,
}

impl PaintLaw {
    fn from_ports(mode: i32, buildup: f32) -> Self {
        if mode == MODE_MOVE {
            Self::Move
        } else {
            Self::Deposit { buildup }
        }
    }

    fn of(ctx: &EvalContext) -> Self {
        Self::from_ports(ctx.input("mode").as_f32() as i32, ctx.input_f32("buildup"))
    }

    fn commit_law(self) -> CommitLaw {
        match self {
            Self::Deposit { .. } => CommitLaw::Deposit,
            Self::Move => CommitLaw::Move,
        }
    }

    /// Which accumulation fills each of the commit law's two slots. Under
    /// `Move` the coverage channel fills the first and the ground the
    /// second. Under `Deposit`, `(wash, build)`: at either end of the dial
    /// the ground is the only accumulation and fills its own law's slot;
    /// between, the ground is the wash half and the build channel the build
    /// half. The commit and the appearance snapshot both map through here,
    /// so the stroke a sampler reads is the one the commit lays.
    fn slots<T: Copy>(
        self,
        ground: T,
        channel: impl FnOnce(&'static str) -> T,
    ) -> (Option<T>, Option<T>) {
        match self {
            Self::Move => (Some(channel(COVERAGE_CHANNEL.name)), Some(ground)),
            Self::Deposit { buildup } => {
                let (wash_share, build_share) = shares(buildup);
                let wash = (wash_share > 0.0).then_some(ground);
                let build = if build_share <= 0.0 {
                    None
                } else if wash_share <= 0.0 {
                    Some(ground)
                } else {
                    Some(channel(BUILD_CHANNEL.name))
                };
                (wash, build)
            }
        }
    }
}

/// The laws a `paint` body stores through, with the ceiling's room
/// arithmetic they share with the commit.
const ACCUMULATE_WGSL: &str = concat!(
    include_str!("../../../shaders/lib/deposit_ceiling.wgsl"),
    "\n",
    include_str!("../../../shaders/brush/paint_accumulate.wgsl"),
);

/// Per-dab meta, in lockstep with the dab records: the footprint's size
/// (the dab's dispatch grid) and the read region (the appearance
/// snapshot's dispatch grid and origin, write-side texels), both clamped
/// to the layer. The read region is the footprint grown by the graph's
/// per-dab read reach; for a graph that reads nothing beyond its
/// footprint it equals the footprint and is never used.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PaintDabMeta {
    grid: [u32; 2],
    read: SnapshotRecord,
}

/// How much of each dab goes to each half, from the `buildup` dial.
///
/// `(wash, build)`. At `0` the whole dab washes, at `1` the whole dab stacks,
/// and between it splits. A share of exactly zero means that half does not
/// exist for this brush: no channel, no storage binding, no commit slot.
/// The upstream graph's premultiplied RGBA expression for one dab.
///
/// Unwired, it falls back to opaque white modulated by the soft disc the
/// wrapper's `local_dist` gives us, so a graph of just pen to paint still
/// produces something visible.
fn rgba_expr(cctx: &CompileWgslCtx) -> String {
    match cctx.inputs.get("rgba") {
        Some(InputBinding::Wired(expr)) => expr.clone(),
        _ => "vec4<f32>(1.0, 1.0, 1.0, 1.0) * max(1.0 - local_dist, 0.0)".into(),
    }
}

fn shares(buildup: f32) -> (f32, f32) {
    let b = buildup.clamp(0.0, 1.0);
    (1.0 - b, b)
}

// ── Per-brush pipeline ──────────────────────────────────────────────────

/// Per-brush resources built on the first `flush_dabs` call for a
/// brush with a given `topology_hash`. Cached on [`PaintPipeline`].
struct PerBrushPipeline {
    /// Per-dab pipeline. The ground is a coverage accumulator and only
    /// paints alpha *up*; which law it accumulates under is the brush's
    /// `buildup` choice, compiled into the body. Engine-level
    /// paint-vs-erase is a stroke decision applied at commit by
    /// `commit_brush_dab`, not here. (Branching the per-dab pass on
    /// `blend_mode` to a destination-out law was a regression: the ground
    /// starts at (0,0,0,0), so `dst*(1-src.a)` stays zero and the commit's
    /// `destination_out` then sees zero alpha and no-ops.)
    pipeline: wgpu::ComputePipeline,
    uniform_ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
    dabs_buffer: wgpu::Buffer,
    /// `@group(1)`: the dab records, the dab slot, the ground and the
    /// storage channels. The bind group is built per flush, because the
    /// scratch and its channels can be reallocated by a grow.
    dabs_bgl: wgpu::BindGroupLayout,
    /// Total size of the uniform block (intrinsic + node fields), in bytes.
    uniform_size: usize,
    /// `@group(3)` graph-texture bind group, present when the brush
    /// graph contains `image`-style nodes. Built once at pipeline
    /// build from the engine's
    /// [`crate::gpu::texture_registry::TextureRegistry`]; reused for
    /// every dab. `None` when the brush requests no graph textures
    /// (the pipeline layout also omits group 3 in that case).
    graph_textures_bind_group: Option<wgpu::BindGroup>,
}

impl PerBrushPipeline {
    fn build(ctx: &BuildContext, compiled: &CompiledBrush) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("paint-brush"),
                source: wgpu::ShaderSource::Wgsl(compiled.stroke_wgsl.clone().into()),
            });

        // group(1): the dab records, the dab slot (a dynamic offset into
        // the static index buffer) and one read-write storage texture per
        // binding the compiled module declares: the ground, then the
        // storage channels. The list comes from the compile output, so
        // the layout matches the shader by construction.
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(16),
                },
                count: None,
            },
        ];
        entries.extend(compiled.storage_bindings().into_iter().map(|b| {
            wgpu::BindGroupLayoutEntry {
                binding: b.binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::ReadWrite,
                    format: b.format,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            }
        }));
        let dabs_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("paint-dabs-bgl"),
                entries: &entries,
            });

        // Optional `@group(3)` graph-texture bind group. Present only
        // when the brush graph requested at least one `image`-style
        // texture. Paint has no terminal bindings of its own, so the
        // graph-textures layout sits at slot 3 directly, since WebGPU's
        // default `max_bind_groups = 4` rules out anything higher.
        // The compile walk rejects graphs that combine an `image`
        // node with a terminal that also claims @group(3) (e.g.
        // watercolor's pickup atlas).
        // `@group(3)` texture count: every slot the graph requested,
        // whatever kind. Live slots (the stroke snapshot, the live stroke
        // appearance) occupy a binding exactly like a named texture; only
        // the moment their view resolves differs.
        let graph_tex_count = compiled.graph_sources.len();
        let graph_layout = if graph_tex_count == 0 {
            None
        } else {
            Some(
                ctx.texture_registry
                    .layout_for_count(ctx.device, graph_tex_count),
            )
        };
        let layout = match &graph_layout {
            None => ctx
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("paint-layout"),
                    bind_group_layouts: &[
                        Some(ctx.uniform_bgl),
                        Some(&dabs_bgl),
                        Some(ctx.selection_bgl),
                    ],
                    immediate_size: 0,
                }),
            Some(gl) => ctx
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("paint-layout-with-graph-textures"),
                    bind_group_layouts: &[
                        Some(ctx.uniform_bgl),
                        Some(&dabs_bgl),
                        Some(ctx.selection_bgl),
                        Some(gl.as_ref()),
                    ],
                    immediate_size: 0,
                }),
        };

        let pipeline = ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("paint"),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
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
        let dabs_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("paint-dabs-buffer"),
            size: (MAX_DABS_PER_PHASE as u64) * (dab_record_size as u64),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Resolve the brush's named graph textures against the
        // engine registry and build the `@group(3)` bind group.
        // Missing names fall back to the registry's `_fallback`
        // texture so the pipeline always builds, surfacing a
        // `log::warn` instead of crashing while the artist types in
        // the node editor.
        // A graph with any live slot rebuilds its bind group every
        // `flush_dabs` from whatever the producing nodes published, so
        // there is nothing to cache here. Wholly static graphs (named
        // textures, baked tiles) build once.
        let graph_textures_bind_group = if compiled.graph_sources.iter().any(|s| s.is_live())
            || compiled.graph_sources.is_empty()
        {
            None
        } else {
            let (_layout, bg) = ctx.texture_registry.make_bind_group(
                ctx.device,
                ctx.queue,
                ctx.baked_sources,
                &compiled.graph_sources,
                &[],
            );
            Some(bg)
        };

        Self {
            pipeline,
            uniform_ring,
            uniform_bind_group,
            dabs_buffer,
            dabs_bgl,
            uniform_size,
            graph_textures_bind_group,
        }
    }

    /// The `@group(1)` bind group for this flush: the records, the index
    /// buffer's first slot (the dynamic offset selects the rest), and the
    /// scratch's current views for the ground and each storage channel.
    fn dabs_bind_group(
        &self,
        device: &wgpu::Device,
        compiled: &CompiledBrush,
        index_buffer: &wgpu::Buffer,
        scratch: &crate::brush::scratch::Scratch,
    ) -> wgpu::BindGroup {
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: self.dabs_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: index_buffer,
                    offset: 0,
                    size: NonZeroU64::new(16),
                }),
            },
        ];
        for b in compiled.storage_bindings() {
            let view = if b.name == GROUND_NAME {
                scratch.write_view()
            } else {
                scratch
                    .channel_view(b.name)
                    .expect("a declared storage channel is allocated before the flush")
            };
            entries.push(wgpu::BindGroupEntry {
                binding: b.binding,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint-dabs-bg"),
            layout: &self.dabs_bgl,
            entries: &entries,
        })
    }
}

// ── Pipeline registry entry ─────────────────────────────────────────────

/// The single registry entry for the `paint` terminal. Holds
/// a cache of per-brush pipelines keyed by `topology_hash`. Pipelines
/// are built lazily on first use.
pub struct PaintPipeline {
    cache: RefCell<HashMap<u64, PerBrushPipeline>>,
    /// The static dab index: slot `i` holds `i`, one slot per
    /// [`MAX_DABS_PER_PHASE`], written once. A dispatch selects its dab
    /// by binding the slot at a dynamic offset of `i * DAB_SLOT_STRIDE`.
    index_buffer: wgpu::Buffer,
}

impl PaintPipeline {
    fn build(ctx: &BuildContext) -> Self {
        let mut index_bytes = vec![0u8; (DAB_SLOT_STRIDE * MAX_DABS_PER_PHASE as u64) as usize];
        for i in 0..MAX_DABS_PER_PHASE {
            let at = (i as u64 * DAB_SLOT_STRIDE) as usize;
            index_bytes[at..at + 4].copy_from_slice(&i.to_le_bytes());
        }
        let index_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("paint-dab-index"),
            size: index_bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&index_buffer, 0, &index_bytes);
        Self {
            cache: RefCell::new(HashMap::new()),
            index_buffer,
        }
    }

    /// Build (or look up) the per-brush pipeline for `compiled`. Called
    /// on every `flush_dabs`: the first call for a hash builds; later
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
        // safely return references through the RefCell: the frame
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
        scratch_format: PACKED_GROUND_FORMAT,
        dab_pass: DabPass::DispatchPerDab,
        node: NodeRegistration {
            type_id: TYPE_ID,
            category: "output",
            display_name: "Paint",
            description: "Output that deposits a brush mark onto the canvas. Plug a Stamp Tip (or any colored mark) into the dab input: this is where paint actually lands.",
            ports: vec![
                PortDef::input("position", BrushWireType::Vec2)
                    .with_description("Canvas-pixel pen tip for this dab"),
                PortDef::input("size", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Size")
                    .with_unit(UnitType::Percent)
                    .with_description(
                        "Per-touch size multiplier (wire pressure here for pressure-sensitive size). Multiplies onto the brush's base size, owned by pen_input.",
                    ),
                PortDef::input("mode", BrushWireType::Enum)
                    .with_enum_options(["Deposit", "Move"])
                    .with_value(InputValue::Int(MODE_DEPOSIT))
                    .with_label("Law")
                    .stroke_constant()
                    .with_description(
                        "Deposit: each touch lays down pigment, the way a pen or brush does; \
                         Build-up sets how repeated touches add up. Move: each touch carries \
                         what was already under it along the stroke, the way a finger \
                         smudges; Coverage sets how much of the spot it takes with it.",
                    ),
                PortDef::input("coverage", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Coverage")
                    .with_unit(UnitType::Percent)
                    .with_visible_when("mode", [MODE_MOVE])
                    .with_description(
                        "Under Move: how much of what is under this spot the finger takes \
                         with each touch. Wire the tip shape here, multiplied by the Canvas \
                         Sampler's Coverage.",
                    ),
                PortDef::input("wash_flow", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Flow (Wash)")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:droplet")
                    .with_visible_when("mode", [MODE_DEPOSIT])
                    .exposed()
                    .with_description(
                        "Per-dab strength of the Wash half. Inactive at Build-up 100%.",
                    ),
                PortDef::input("build_flow", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Flow (Build-up)")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:droplet")
                    .exposed()
                    .with_description(
                        "Per-dab strength of the Build-up half. Inactive at Build-up 0%.",
                    ),
                PortDef::input("opacity", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Opacity")
                    .with_unit(UnitType::Percent)
                    .with_icon("mdi:texture-box")
                    .exposed()
                    .with_description("Stroke-level opacity cap (applied at commit)"),
                // A share of each dab, not an interpolated law. The commit
                // lays the wash slot through the ceiling and the build slot
                // over it, and a single ground mixing both halves could not
                // be split there, so what is continuous is the *input*: the
                // dab is split between two accumulations, each running its
                // own law untouched. Both halves ride the one dispatch, so
                // 1px spacing stays affordable.
                PortDef::input("buildup", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Build-up")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:layer-group")
                    .with_visible_when("mode", [MODE_DEPOSIT])
                    .stroke_constant()
                    .exposed()
                    .with_description(
                        "How much repeated passes build: 0% = a mark never darkens, 100% = toward opaque.",
                    ),
                // Typed as `Texture` to match the upstream `stamp.dab`
                // output's wire type; the wire-type label is shared
                // with the per-dab dispatch model where it'd be a
                // texture handle. In the compiled path it's a
                // `vec4<f32>` expression. Without this match, the
                // graph compiler rejects the connection at brush load.
                PortDef::input("rgba", BrushWireType::Vec4).with_description(
                    "Premultiplied RGBA from the upstream compiled graph (typically `stamp.dab`)",
                ),
                PortDef::output("dab_size", BrushWireType::Vec2)
                    .with_description("Brush mark size in canvas pixels"),
            ],
            is_gpu: true,
            is_terminal: true,
            supports_erase: true,
            preview_staging: None,
        },
    }
}

pub struct PaintEvaluator;

impl PaintEvaluator {
    fn effective_radius(ctx: &EvalContext) -> f32 {
        crate::brush::read_mirror_terminal::effective_radius(ctx)
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
        let Some(compiled) = gpu.dab_batch.compiled_brush.clone() else {
            // Compiled brush wasn't attached: programming error in
            // the engine wiring. Panic in debug, drop dab silently in
            // release so we don't blow up an in-flight stroke.
            debug_assert!(false, "paint requires compiled_brush on gpu_context");
            return vec![];
        };
        let Some(stroke) = gpu.stroke.as_ref() else {
            return vec![];
        };
        let paint_target = &stroke.paint_target;
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
        // Publish the footprint; `None` means the dab is entirely off-extent
        // and has no pixels to draw. The dispatch grid covers the clamped
        // rect, so its size rides the batch's per-dab meta beside the record.
        let Some(footprint) =
            gpu.dab_batch
                .record_dab_footprint(paint_target, position, bbox_radius)
        else {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        };
        // The read region: the footprint grown by what the graph reads
        // beyond it this dab, under the same clamp, in write-side texels.
        let extent = paint_target.canvas_extent();
        let reach = gpu.dab_batch.read_reach;
        let read = extent
            .clamp_f32(
                position[0] - bbox_radius - reach[0],
                position[1] - bbox_radius - reach[1],
                position[0] + bbox_radius + reach[0],
                position[1] + bbox_radius + reach[1],
            )
            .expect("the read region encloses a footprint that overlaps the layer");
        gpu.dab_batch
            .meta_bytes
            .extend_from_slice(bytemuck::bytes_of(&PaintDabMeta {
                grid: [footprint.width, footprint.height],
                read: SnapshotRecord {
                    origin: [
                        (read.x0() - extent.x0()) as u32,
                        (read.y0() - extent.y0()) as u32,
                    ],
                    size: [read.width, read.height],
                },
            }));

        gpu.dab_batch
            .queue_dab(&compiled, position, bbox_radius, radius);

        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    fn flush_dabs(&self, ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.dab_batch.count == 0 {
            return;
        }
        let Some(compiled) = gpu.dab_batch.compiled_brush.clone() else {
            debug_assert!(false, "paint::flush_dabs requires compiled_brush");
            return;
        };

        let (union_w, union_h) = gpu.dab_batch.batch_extent();
        let (dab_bytes, total_dabs) = gpu.dab_batch.take();
        let meta_bytes = gpu.dab_batch.take_meta();
        if total_dabs == 0 {
            return;
        }
        gpu.perf
            .record_dab_flush_workload(total_dabs, union_w, union_h);
        let metas: &[PaintDabMeta] = bytemuck::cast_slice(&meta_bytes);
        debug_assert_eq!(
            metas.len(),
            total_dabs as usize,
            "paint queues one meta per dab record"
        );

        let pipeline_ref = gpu.pipelines.get::<PaintPipeline>("paint");

        // Build the per-brush pipeline if this is the first dab for
        // this hash. The BuildContext borrows pieces from
        // BrushPipelines via private accessors; we use a minimal
        // local BuildContext built from the gpu_context's wgpu refs.
        // Note: this is a one-shot build per brush, so the cost is
        // amortised across thousands of dabs.
        ensure_per_brush_pipeline(gpu, pipeline_ref, &compiled);

        let stroke = gpu
            .stroke
            .as_ref()
            .expect("paint::flush_dabs requires stroke resources");
        let scratch = &*stroke.scratch;
        let paint_target = &stroke.paint_target;
        let canvas_ext = paint_target.canvas_extent();
        let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];
        let layer_size = [canvas_ext.width, canvas_ext.height];

        // Build the uniform buffer: intrinsic header + node fields.
        // Per-stroke not per-dab, but still no need to clone.
        let mut uniform_bytes: Vec<u8> = Vec::with_capacity(MAX_UNIFORM_BYTES);
        pack_intrinsic_uniforms(
            &mut uniform_bytes,
            gpu.intrinsic_header(layer_offset, layer_size),
        );
        let outputs = gpu
            .dab_batch
            .slot_outputs
            .as_ref()
            .expect("paint::flush_dabs requires dab_batch.slot_outputs");
        pack_uniforms(&compiled, outputs, &mut uniform_bytes);

        // The appearance snapshot, for a graph that samples the live
        // stroke: its records, its flags from the same slot mapping the
        // commit uses, and the mirror it writes. Group 1 per flush, since a
        // grow reallocates every view in it.
        let pre_stroke_view = stroke
            .pre_stroke_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let snapshot = gpu
            .pipelines
            .get::<AppearanceSnapshotPipeline>(APPEARANCE_SNAPSHOT_ID);
        let law = PaintLaw::of(ctx);
        let snapshots = compiled.reads_stroke_appearance().then(|| {
            let (wash, build) = law.slots(scratch.write_view(), |name| {
                scratch
                    .channel_view(name)
                    .expect("a brush declares the channel its law accumulates into")
            });
            let records: Vec<SnapshotRecord> = metas.iter().map(|m| m.read).collect();
            snapshot.begin_flush(
                gpu.device,
                gpu.queue,
                &pipeline_ref.index_buffer,
                &records,
                SnapshotSources {
                    wash,
                    build,
                    law: law.commit_law(),
                    pre_stroke: &pre_stroke_view,
                    appearance: scratch
                        .appearance_view()
                        .expect("begin_stroke allocates the mirror for a brush that reads it"),
                },
            )
        });

        // `@group(3)` for graphs with a live slot: rebuilt here each flush
        // from the stroke textures this terminal publishes. Both are stroke
        // resources, so the terminal that owns the stroke publishes them;
        // an unpublished slot resolves to `_fallback` inside
        // `make_bind_group`.
        let live_bind_group = if compiled.graph_sources.iter().any(|s| s.is_live()) {
            let source = stroke
                .source_texture()
                .create_view(&wgpu::TextureViewDescriptor::default());
            gpu.dab_batch
                .publish_live_texture(LiveSource::StrokeSnapshot, source);
            if let Some(view) = scratch.appearance_view() {
                gpu.dab_batch
                    .publish_live_texture(LiveSource::StrokeAppearance, view.clone());
            }
            let published: Vec<Option<&wgpu::TextureView>> = compiled
                .graph_sources
                .iter()
                .map(|s| match s {
                    ResolvedSource::Live(kind) => gpu.dab_batch.live_texture(*kind),
                    _ => None,
                })
                .collect();
            let (_layout, bg) = gpu.pipelines.texture_registry().make_bind_group(
                gpu.device,
                gpu.queue,
                gpu.pipelines.baked_sources(),
                &compiled.graph_sources,
                &published,
            );
            Some(bg)
        } else {
            None
        };

        pipeline_ref.with_pipeline(compiled.topology_hash, |per_brush| {
            // Pad uniform bytes up to the per-brush uniform size so the
            // ring entry's binding_size matches.
            if uniform_bytes.len() < per_brush.uniform_size {
                uniform_bytes.resize(per_brush.uniform_size, 0);
            }
            // Reset the ring before each flush: the ring is per-
            // brush and isn't shared with other terminals, so this is
            // safe (we own all live writes in this `flush_dabs`).
            per_brush.uniform_ring.reset();
            let uniform_offset = per_brush.uniform_ring.write(gpu.queue, &uniform_bytes);

            // Upload the dab records.
            gpu.queue
                .write_buffer(&per_brush.dabs_buffer, 0, &dab_bytes);

            // Rebuilt per flush: a grow can have reallocated the ground
            // and the channels since the last one.
            let dabs_bind_group = per_brush.dabs_bind_group(
                gpu.device,
                &compiled,
                &pipeline_ref.index_buffer,
                scratch,
            );

            // The accumulation law is compiled into this pipeline.
            // Paint-vs-erase routes through `gpu.blend_mode` in
            // `commit_brush_dab`; see the `pipeline` field's doc on
            // `PerBrushPipeline`.
            let mut pass = gpu
                .encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("paint-flush"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(&per_brush.pipeline);
            pass.set_bind_group(0, &per_brush.uniform_bind_group, &[uniform_offset]);
            pass.set_bind_group(2, gpu.selection_bind_group, &[]);
            // `@group(3)` holds the brush's graph textures: paper grain,
            // baked noise, the stroke snapshot, the stroke appearance.
            // Graphs with a live slot bind the group assembled above;
            // wholly static ones bind the pipeline's cached group.
            // Paint never uses group 3 for anything else.
            if let Some(live_bg) = live_bind_group.as_ref() {
                pass.set_bind_group(3, live_bg, &[]);
            } else if let Some(graph_bg) = per_brush.graph_textures_bind_group.as_ref() {
                pass.set_bind_group(3, graph_bg, &[]);
            }
            // One dispatch per dab over its clamped footprint; the shader
            // rejects the threads past the footprint's rounding and
            // outside the dab's bbox. Dispatches in a pass are ordered, so
            // a dab's snapshot sees every earlier dab and the dab sees its
            // snapshot.
            for (i, meta) in metas.iter().enumerate() {
                if let Some(flush) = &snapshots {
                    // Its own pipeline and group 1. Group 0 is an equal
                    // layout entry in both pipelines, so it stays bound
                    // across the switch; groups 2 and 3 lie outside the
                    // snapshot's layout and are still bound when the brush
                    // pipeline returns.
                    snapshot.dispatch(&mut pass, flush, i as u32);
                    pass.set_pipeline(&per_brush.pipeline);
                }
                let groups = |px: u32| px.div_ceil(DAB_WORKGROUP).max(1);
                pass.set_bind_group(1, &dabs_bind_group, &[(i as u64 * DAB_SLOT_STRIDE) as u32]);
                pass.dispatch_workgroups(groups(meta.grid[0]), groups(meta.grid[1]), 1);
            }
        });

        gpu.perf.record_dab_flush(total_dabs);
        let per_dab = if snapshots.is_some() { 2 } else { 1 };
        gpu.perf.record_dispatches(per_dab * total_dabs);
    }

    fn commit(&self, ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let Some(stroke) = gpu.stroke.as_ref() else {
            return;
        };
        let opacity = ctx.input_f32("opacity").clamp(0.0, 1.0);
        // Each accumulation goes in the slot that commits it under the
        // brush's law.
        let law = PaintLaw::of(ctx);
        let (wash, build) = law.slots(stroke.scratch.write_bind_group(), |name| {
            stroke
                .scratch
                .channel_bind_group(name)
                .expect("a brush declares the channel its law accumulates into")
        });
        stroke.paint_target.commit_brush_dab(
            &mut gpu.encoder,
            gpu.pipelines,
            gpu.queue,
            CommitForegrounds {
                format: stroke.scratch.format(),
                wash,
                build,
                law: law.commit_law(),
            },
            stroke.pre_stroke_bind_group,
            opacity,
            gpu.blend_mode,
        );
    }

    /// Hover-cursor preview: reuses the shared
    /// [`crate::brush::wgsl::render_compiled_cursor_preview`] helper.
    /// `paint`'s stroke body and preview body are the same
    /// source (no `compile_cursor_preview_body` override), so the cursor
    /// shows the brush color × shape × flow as the stroke would
    /// deposit.
    fn render_cursor_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let radius = Self::effective_radius(ctx);
        let _ = crate::brush::wgsl::render_compiled_cursor_preview(gpu, radius);
        vec![]
    }

    /// Emit the compute body's terminal: returns when the upstream
    /// graph's premultiplied RGBA has no coverage under the selection
    /// mask, otherwise multiplies it by the flow and the mask and folds it
    /// into each ground through the brush's law, storing a ground only
    /// where the law changed its texel. The framework's
    /// [`crate::brush::wgsl::assemble_shader`] places the node bodies
    /// inside `cs_main` already bound with `d`, `u`, `local_uv`,
    /// `local_dist`, `theta`, `target_pos`, `sel`, `layer_px` and the
    /// `ground`.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl {
            decls: ACCUMULATE_WGSL.to_string(),
            ..NodeWgsl::default()
        };
        let rgba_expr = rgba_expr(cctx);
        let mut body = format!("    let rgba = {rgba_expr};\n");
        // A ground's read-modify-write. `law` is the law's call with `DST`
        // standing for the loaded texel. The store is skipped where the
        // packed result equals the loaded word (a refused wash dab, say),
        // which is exact: an equal store changes nothing.
        let store = |target: &str, law: &str| {
            let law = law.replace("DST", &format!("unpack_ground({target}_was)"));
            format!(
                "    let {target}_was = textureLoad({target}, layer_px);\n\
                 \x20   let {target}_now = pack_ground({law});\n\
                 \x20   if ({target}_now.r != {target}_was.r) {{\n\
                 \x20       textureStore({target}, layer_px, {target}_now);\n\
                 \x20   }}\n"
            )
        };
        if cctx.input("mode").enum_index() == MODE_MOVE {
            // Move: the finger displaces `cov` of the pixel and leaves what
            // it brought. The pigment lerps in the ground; the coverage
            // accumulates in its channel as a bare alpha under source-over,
            // which is `1 - prod(1 - cov_i)`.
            let flow = cctx.input("build_flow").as_f32();
            let coverage = cctx.input("coverage").as_f32();
            body.push_str(&format!(
                "    let flow = clamp({flow}, 0.0, 1.0);\n\
                 \x20   let cov = clamp({coverage}, 0.0, 1.0) * flow * sel;\n\
                 \x20   if (cov == 0.0) {{\n        return;\n    }}\n\
                 \x20   let src = rgba * flow * sel;\n"
            ));
            body.push_str(&store(GROUND_NAME, "accumulate_move(src, cov, DST)"));
            body.push_str(&store(
                COVERAGE_CHANNEL.name,
                "accumulate_build(vec4<f32>(0.0, 0.0, 0.0, cov), DST)",
            ));
            wgsl.channels = vec![COVERAGE_CHANNEL];
            wgsl.body = body;
            return Ok(wgsl);
        }
        // Per-dab flow, one per half, folded into the premultiplied rgba
        // (multiply all four components) the way the original terminal's
        // `color[3] *= flow` was. Wired values flow through their dab-record
        // field; unwired ones are the port default literal.
        //
        // The dial is stroke-constant, so it is always a literal here, and
        // the shares it yields decide the shape of the pass: which law the
        // ground accumulates under, whether a second accumulation exists,
        // and what the body stores.
        let buildup = cctx.input("buildup").as_f32_literal().ok_or_else(|| {
            "paint.buildup picks the pass's accumulation laws and storage bindings when the \
             brush compiles, so a per-dab wire cannot drive it"
                .to_string()
        })?;
        let (wash_share, build_share) = shares(buildup);
        // No coverage at this pixel: both laws would hand back the texel
        // they were given, so the thread skips the flows and the grounds.
        body.push_str("    if (rgba.a * sel == 0.0) {\n        return;\n    }\n");
        if wash_share > 0.0 {
            let expr = cctx.input("wash_flow").as_f32();
            body.push_str(&format!("    let wash_flow = clamp({expr}, 0.0, 1.0);\n"));
        }
        if build_share > 0.0 {
            let expr = cctx.input("build_flow").as_f32();
            body.push_str(&format!("    let build_flow = clamp({expr}, 0.0, 1.0);\n"));
        }
        if build_share <= 0.0 {
            // Wash alone: the ground takes the strongest dab.
            body.push_str("    let src = rgba * wash_flow * sel;\n");
            body.push_str(&store(GROUND_NAME, "accumulate_wash(src, DST)"));
        } else if wash_share <= 0.0 {
            // Build-up alone: the ground composites every dab over the last.
            body.push_str("    let src = rgba * build_flow * sel;\n");
            body.push_str(&store(GROUND_NAME, "accumulate_build(src, DST)"));
        } else {
            // Both: the one dispatch writes each half into the
            // accumulation that runs its law, scaled by its share.
            body.push_str(&format!(
                "    let wash_src = rgba * wash_flow * sel * {wash_share:.6};\n"
            ));
            body.push_str(&format!(
                "    let build_src = rgba * build_flow * sel * {build_share:.6};\n"
            ));
            body.push_str(&store(GROUND_NAME, "accumulate_wash(wash_src, DST)"));
            body.push_str(&store(
                BUILD_CHANNEL.name,
                "accumulate_build(build_src, DST)",
            ));
            wgsl.channels = vec![BUILD_CHANNEL];
        }
        wgsl.body = body;
        Ok(wgsl)
    }

    /// Hover-cursor preview body.
    ///
    /// The preview skeleton is a fragment module rendering one dab to a
    /// thumbnail with a single output, so it has no ground to store into.
    /// It shows the one dab as the stroke would deposit it on blank
    /// ground, blending the two flows by the dial: exact at either end,
    /// and to first order between (it drops the cross term of compositing
    /// a dab's own two halves).
    fn compile_cursor_preview_body(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        let rgba_expr = rgba_expr(cctx);
        let buildup = cctx.input("buildup").as_f32_literal().unwrap_or(1.0);
        let (_, build_share) = if cctx.input("mode").enum_index() == MODE_MOVE {
            (0.0, 1.0)
        } else {
            shares(buildup)
        };
        let wash_expr = cctx.input("wash_flow").as_f32();
        let build_expr = cctx.input("build_flow").as_f32();
        wgsl.body = format!(
            "    let rgba = {rgba_expr};\n\
             \x20   let wash_flow = clamp({wash_expr}, 0.0, 1.0);\n\
             \x20   let build_flow = clamp({build_expr}, 0.0, 1.0);\n\
             \x20   return rgba * mix(wash_flow, build_flow, {build_share:.6}) * sel;\n"
        );
        Ok(wgsl)
    }
}

// ── Per-brush pipeline build helper ─────────────────────────────────────

/// Build the per-brush pipeline for `compiled` if it isn't already
/// cached. Reconstructs a [`BuildContext`] from the `BrushGpuContext`'s
/// shared state: same BGLs and shared limits used at the original
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
        canvas_copy: gpu.pipelines.canvas_copy_layout(),
        canvas_copy_layouts: gpu.pipelines.canvas_copy_layouts(),
        min_uniform_align: gpu.device.limits().min_uniform_buffer_offset_alignment,
        texture_registry: gpu.pipelines.texture_registry(),
        baked_sources: gpu.pipelines.baked_sources(),
    };
    pipe.ensure_pipeline(&ctx, compiled);
}
