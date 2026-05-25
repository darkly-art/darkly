//! Watercolor (batched) — single-pass-per-phase instanced fragment
//! terminal for the Wet Media brush family.
//!
//! ## What this terminal does
//!
//! Each `evaluate_gpu` call (one per dab) only **queues** a
//! [`WatercolorDabRecord`] on the shared dab queue. No render passes are
//! opened per dab. At the end of the rendering phase, `flush_dabs` opens
//! **two** render passes:
//!
//! 1. **Pickup atlas pass** — one render pass, N instances. Each
//!    instance writes its 1×1 alpha-weighted neighborhood average to its
//!    own cell in a pre-allocated `pickup_atlas` texture, sampling from
//!    [`pre_stroke_texture`][gpc] (the immutable layer snapshot captured
//!    at stroke start). Cells are laid out as
//!    `(instance % ATLAS_WIDTH, instance / ATLAS_WIDTH)`.
//!
//! 2. **Composite pass** — one render pass on `scratch.write_view()`,
//!    N instanced quads. Each instance emits a quad covering its dab
//!    footprint, samples the atlas at its own cell, runs the procedural
//!    shape mask + selection mask + watercolor load math, and emits
//!    premultiplied `(load_rgb * fg_a, fg_a)`. Hardware blend
//!    `(One, OneMinusSrcAlpha, Add)` handles per-pixel source-over.
//!
//! ## Alpha & semantics
//!
//! - Pickup always reads `pre_stroke_texture`. Each dab samples the
//!   original layer underneath it; intra-stroke deposits never feed
//!   back into a later dab's pickup. This is the
//!   user-confirmed semantic — see `paint-compute-perf-tracking.md`'s
//!   sibling watercolor doc for the rationale.
//! - Composite emits premultiplied `(load * fg_a, fg_a)`; the scratch
//!   accumulates premultiplied watercolor dabs. `commit_scratch_blit`
//!   blits the finished scratch onto the layer (the per-dab load math
//!   already burned in the right pixels, so no separate source-over at
//!   commit time).
//!
//! ## Why this is the only watercolor terminal we ship
//!
//! - vs per-dab fragment passes (the `watercolor.rs` reference): one
//!   pass per phase eliminates per-dab `begin_render_pass` overhead,
//!   which is the killer at tight-spacing strokes.
//! - vs compute (`watercolor_compute`, retired by this change): no
//!   buffer round-trip. Hardware ROP writes the scratch directly, so
//!   per-flush cost scales with `Σ(dab_area)` instead of
//!   `union_bbox_area` (the dominant cost in `watercolor_compute`).
//!
//! [gpc]: crate::brush::gpu_context::BrushGpuContext::pre_stroke_texture

use std::any::Any;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::{BrushGpuContext, MAX_DABS_PER_PHASE};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipeline::{
    BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::{ParamDef, ParamValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

/// Canvas-pixel reference for `size_input * size = 1.0`. Mirrors stamp /
/// paint so a brush built around this terminal feels identical to one
/// built around `paint` at the same port values.
const SIZE_REFERENCE_PX: f32 = crate::brush::dab_pool::DAB_REFERENCE_SIZE as f32;

/// θ-samples for the CPU-side centroid integration. Mirrors
/// `circle.rs::CENTROID_SAMPLES` — keep the two in sync.
const CENTROID_SAMPLES: usize = 256;

const ALGO_SINE: u32 = 0;
const ALGO_PERLIN: u32 = 1;
const ALGO_SUPERFORMULA: u32 = 2;

/// Pickup atlas dimensions. The atlas holds one cell per queued dab;
/// `ATLAS_WIDTH * ATLAS_HEIGHT >= MAX_DABS_PER_PHASE` is the only
/// constraint. 128×128 = 16384 fits exactly. Shape is not load-bearing
/// — pickup writes `(idx % W, idx / W)`, composite reads the same.
/// WebGPU's `maxTextureDimension2D` floor is 8192, so a 1D-shape
/// (e.g. 16384×1) wouldn't be portable; 128 stays well under.
const ATLAS_WIDTH: u32 = 128;
const ATLAS_HEIGHT: u32 = 128;
const _: () = assert!(ATLAS_WIDTH * ATLAS_HEIGHT >= MAX_DABS_PER_PHASE);

// ── Dab record ──────────────────────────────────────────────────────────

/// One queued watercolor dab. Layout MUST match the `Dab` struct in
/// `shaders/brush/watercolor_batched_pickup.wgsl` and
/// `shaders/brush/watercolor_batched_composite.wgsl` — both shaders
/// reinterpret these bytes verbatim.
///
/// 96 bytes per record. std430-compatible (vec2 → 8-byte aligned, vec4
/// → 16-byte aligned, total size multiple of largest alignment).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WatercolorDabRecord {
    pub pos: [f32; 2],
    /// Natural-unit reference disc radius in canvas pixels.
    pub radius: f32,
    /// Conservative upper bound on `r(θ)` over the full revolution.
    /// Sizes the per-dab bbox so the modulated silhouette never falls
    /// outside the rasterized quad.
    pub r_max_unit: f32,
    /// Natural-unit centroid offset — CPU-integrated per dab so
    /// asymmetric shapes land their geometric centre on the pen tip.
    pub centroid: [f32; 2],
    pub softness: f32,
    pub deposit: f32,
    pub wetness: f32,
    pub stroke_opacity: f32,
    pub algorithm: u32,
    pub amplitude: f32,
    pub frequency: f32,
    pub phase: f32,
    pub persistence: f32,
    pub seed: f32,
    pub octaves: u32,
    pub n1: f32,
    pub n2: f32,
    pub n3: f32,
    /// Straight-alpha paint colour with `flow` pre-folded into `a`.
    /// Composite shader reads `color.rgb` plus `color.a` (= flow ×
    /// paint alpha) as the load-alpha ceiling.
    pub color: [f32; 4],
}

// ── Uniforms ────────────────────────────────────────────────────────────

/// Per-flush uniforms for the pickup pass.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PickupUniforms {
    /// Canvas-pixel origin of the pre-stroke texture's (0,0) pixel.
    /// Equals the layer's canvas offset at stroke-start time.
    pre_stroke_origin: [i32; 2],
    /// pre_stroke texture dimensions in pixels.
    pre_stroke_size: [u32; 2],
    /// Atlas width — vertex shader maps `instance_index` to atlas pixel
    /// `(idx % atlas_width, idx / atlas_width)`.
    atlas_width: u32,
    atlas_height: u32,
    _pad0: u32,
    _pad1: u32,
}

/// Per-flush uniforms for the composite pass.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CompositeUniforms {
    layer_offset: [i32; 2],
    layer_size: [u32; 2],
    canvas_size: [u32; 2],
    atlas_width: u32,
    atlas_height: u32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PreviewUniforms {
    softness: f32,
    _pad: [f32; 3],
}

// ── Pipeline ────────────────────────────────────────────────────────────

pub struct WatercolorBatchedPipeline {
    pickup_pipeline: wgpu::RenderPipeline,
    pickup_uniform_ring: DynamicUniformRing,
    pickup_uniform_bind_group: wgpu::BindGroup,

    composite_pipeline: wgpu::RenderPipeline,
    composite_uniform_ring: DynamicUniformRing,
    composite_uniform_bind_group: wgpu::BindGroup,

    /// Shared storage buffer holding the queued dab records. Re-written
    /// per flush via `queue.write_buffer`. Bound at group(1) of both
    /// pickup (fragment-only) and composite (vertex+fragment) — the
    /// shared BGL declares VERTEX_FRAGMENT visibility.
    dabs_buffer: wgpu::Buffer,
    dabs_bind_group: wgpu::BindGroup,

    /// 128×128 RGBA8 pickup atlas. Pickup pass writes to it (one cell
    /// per dab); composite samples it. Allocated once at pipeline build.
    _atlas_texture: wgpu::Texture,
    atlas_attachment_view: wgpu::TextureView,
    /// canvas_copy_bgl bind group over the atlas, for the composite to
    /// sample. Sampler is irrelevant — the shader uses `textureLoad`.
    atlas_bind_group: wgpu::BindGroup,

    /// Procedural soft-disc preview, same approach as `paint` /
    /// `watercolor_compute`. Used for the hover cursor.
    preview_pipeline: wgpu::RenderPipeline,
    preview_uniform_buffer: wgpu::Buffer,
    preview_uniform_bind_group: wgpu::BindGroup,
}

impl WatercolorBatchedPipeline {
    fn build(ctx: &BuildContext) -> Self {
        // ── Shared dab storage buffer + BGL ──────────────────────────
        // VERTEX_FRAGMENT visibility — composite vertex shader reads
        // `pos`/`radius`/`r_max_unit` to size each instance's quad;
        // both fragment shaders read the rest of the record.
        let dabs_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("watercolor-batched-dabs-bgl"),
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

        let dabs_buffer_size =
            (MAX_DABS_PER_PHASE as u64) * (std::mem::size_of::<WatercolorDabRecord>() as u64);
        let dabs_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("watercolor-batched-dabs-buffer"),
            size: dabs_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dabs_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor-batched-dabs-bg"),
            layout: &dabs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dabs_buffer.as_entire_binding(),
            }],
        });

        // ── Pickup atlas (RGBA8, 128×128) ────────────────────────────
        let atlas_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("watercolor-batched-pickup-atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_WIDTH,
                height: ATLAS_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let atlas_attachment_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("watercolor-batched-atlas-attachment"),
            ..Default::default()
        });
        let atlas_sampled_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("watercolor-batched-atlas-sampled"),
            ..Default::default()
        });
        let atlas_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor-batched-atlas-bg"),
            layout: ctx.canvas_copy_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_sampled_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(ctx.canvas_copy_sampler),
                },
            ],
        });

        // ── Pickup pipeline ──────────────────────────────────────────
        let pickup_shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("watercolor-batched-pickup"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../../../shaders/brush/watercolor_batched_pickup.wgsl")
                        .into(),
                ),
            });
        let pickup_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("watercolor-batched-pickup-layout"),
                // group(0) = uniforms, group(1) = dabs, group(2) = pre_stroke.
                bind_group_layouts: &[ctx.uniform_bgl, &dabs_bgl, ctx.canvas_copy_bgl],
                immediate_size: 0,
            });
        let pickup_pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("watercolor-batched-pickup"),
                layout: Some(&pickup_layout),
                vertex: wgpu::VertexState {
                    module: &pickup_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &pickup_shader,
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
        let (pickup_uniform_ring, pickup_uniform_bind_group) = ctx
            .make_uniform_ring::<PickupUniforms>(
                "watercolor-batched-pickup-uniforms",
                "watercolor-batched-pickup-uniform-bg",
            );

        // ── Composite pipeline ───────────────────────────────────────
        let composite_shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("watercolor-batched-composite"),
                source: wgpu::ShaderSource::Wgsl(
                    concat!(
                        include_str!("../../../../../shaders/brush/_shape.wgsl"),
                        "\n",
                        include_str!(
                            "../../../../../shaders/brush/watercolor_batched_composite.wgsl"
                        ),
                    )
                    .into(),
                ),
            });
        let composite_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("watercolor-batched-composite-layout"),
                // group(0) = uniforms, group(1) = dabs, group(2) =
                // selection, group(3) = atlas (sampled via canvas_copy_bgl).
                bind_group_layouts: &[
                    ctx.uniform_bgl,
                    &dabs_bgl,
                    ctx.selection_bgl,
                    ctx.canvas_copy_bgl,
                ],
                immediate_size: 0,
            });
        // Premultiplied source-over: `out = src + dst * (1 - src.a)`. The
        // composite shader emits premultiplied `(load_rgb * fg_a, fg_a)`
        // so hardware blend handles the source-over read-modify-write
        // atomically per pixel — same setup as paint #4.
        let composite_blend = wgpu::BlendState {
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
        let composite_pipeline =
            ctx.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("watercolor-batched-composite"),
                    layout: Some(&composite_layout),
                    vertex: wgpu::VertexState {
                        module: &composite_shader,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &composite_shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            blend: Some(composite_blend),
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
        let (composite_uniform_ring, composite_uniform_bind_group) = ctx
            .make_uniform_ring::<CompositeUniforms>(
                "watercolor-batched-composite-uniforms",
                "watercolor-batched-composite-uniform-bg",
            );

        // ── Preview pipeline (procedural disc) ───────────────────────
        // Lifted from `watercolor_compute` so the hover cursor matches
        // the existing Wet Media brush feel.
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
                label: Some("watercolor-batched-preview"),
                source: wgpu::ShaderSource::Wgsl(preview_shader_src.into()),
            });
        let preview_uniform_bgl =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("watercolor-batched-preview-uniform-bgl"),
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
                label: Some("watercolor-batched-preview-layout"),
                bind_group_layouts: &[&preview_uniform_bgl],
                immediate_size: 0,
            });
        let preview_pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("watercolor-batched-preview"),
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
            label: Some("watercolor-batched-preview-uniform"),
            size: std::mem::size_of::<PreviewUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let preview_uniform_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor-batched-preview-uniform-bg"),
            layout: &preview_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: preview_uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            pickup_pipeline,
            pickup_uniform_ring,
            pickup_uniform_bind_group,
            composite_pipeline,
            composite_uniform_ring,
            composite_uniform_bind_group,
            dabs_buffer,
            dabs_bind_group,
            _atlas_texture: atlas_texture,
            atlas_attachment_view,
            atlas_bind_group,
            preview_pipeline,
            preview_uniform_buffer,
            preview_uniform_bind_group,
        }
    }
}

impl BrushPipelineEntry for WatercolorBatchedPipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    /// This entry owns two rings — pickup + composite — both written
    /// once per `flush_dabs`. Returning both via `rings()` lets the
    /// central registry reset and overflow-check them in lock-step
    /// with every other pipeline. (The default `ring()` impl returns
    /// `None` since neither ring is "primary".)
    fn rings(&self) -> Vec<&DynamicUniformRing> {
        vec![&self.pickup_uniform_ring, &self.composite_uniform_ring]
    }
}

fn watercolor_batched_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "watercolor_batched",
        build: |ctx| Box::new(WatercolorBatchedPipeline::build(ctx)),
    }
}

// ── Node registration ───────────────────────────────────────────────────

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![watercolor_batched_pipeline_reg()],
        node: NodeRegistration {
            type_id: "watercolor_batched",
            category: "output",
            display_name: "Watercolor",
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
                    .with_range(0.0, 4.0, 0.5)
                    .with_label("Size")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-up-right-and-down-left-from-center")
                    .exposed()
                    .with_preview_value(0.1)
                    .with_description("Overall brush size"),
                PortDef::input("softness", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.5)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Softness")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-feather")
                    .exposed()
                    .with_description("Edge softness (0% = hard, 100% = feathered)"),
                PortDef::input("color", BrushWireType::Color)
                    .with_description("Paint color"),
                PortDef::input("deposit", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.5)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Deposit")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-fill-drip")
                    .exposed()
                    .with_description(
                        "How much new paint to add vs. smear existing color. 0% smudges without adding paint; 100% paints normally.",
                    ),
                PortDef::input("wetness", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.5)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Wetness")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-water")
                    .exposed()
                    .with_description(
                        "How strongly each brush touch leaves a mark. 0% leaves nothing; 100% applies the brush at full strength.",
                    ),
                PortDef::input("flow", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Flow")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-droplet")
                    .with_description("Per-dab paint strength multiplier (folded into paint color alpha)."),
                PortDef::input("opacity", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Opacity")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-droplet")
                    .exposed()
                    .with_description("Overall stroke strength. Lower values make the brush lighter."),

                // ── Shape modulation ────────────────────────────────
                PortDef::input("amplitude", BrushWireType::Scalar)
                    .with_range(0.0, 0.5, 0.0)
                    .with_natural_range(0.0, 0.5)
                    .with_label("Amplitude")
                    .with_unit(UnitType::Percent)
                    .with_visible_when("algorithm", [ALGO_SINE as i32, ALGO_PERLIN as i32])
                    .with_description("Bump amplitude as a fraction of the base radius."),
                PortDef::input("frequency", BrushWireType::Scalar)
                    .with_range(1.0, 16.0, 6.0)
                    .with_natural_range(1.0, 16.0)
                    .with_step(1.0)
                    .with_label("Frequency")
                    .with_unit(UnitType::Raw)
                    .with_description(
                        "Sine: number of bumps. Perlin: base period in cells per revolution. Superformula: symmetry order. Must be an integer.",
                    ),
                PortDef::input("phase_input", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                    .with_label("Phase Input")
                    .with_unit(UnitType::Degrees)
                    .with_description(
                        "Per-dab phase, summed with `phase`. Wire `pen.tilt_direction` or `pen.drawing_angle` so the shape rotates with the pen.",
                    ),
                PortDef::input("phase", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                    .with_label("Phase")
                    .with_unit(UnitType::Degrees)
                    .with_description(
                        "Static rotation of the shape around its own centre, summed with `phase_input`.",
                    ),
                PortDef::input("persistence", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.5)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Persistence")
                    .with_unit(UnitType::Percent)
                    .with_visible_when("algorithm", [ALGO_PERLIN as i32])
                    .with_description("Per-octave amplitude falloff. Higher = rougher edge."),
                PortDef::input("seed", BrushWireType::Scalar)
                    .with_range(0.0, 1024.0, 0.0)
                    .with_natural_range(0.0, 1024.0)
                    .with_label("Seed")
                    .with_unit(UnitType::Raw)
                    .with_visible_when("algorithm", [ALGO_PERLIN as i32])
                    .with_description("RNG seed for the noise array."),
                PortDef::input("octaves", BrushWireType::Scalar)
                    .with_range(1.0, 6.0, 3.0)
                    .with_natural_range(1.0, 6.0)
                    .with_label("Octaves")
                    .with_unit(UnitType::Raw)
                    .with_visible_when("algorithm", [ALGO_PERLIN as i32])
                    .with_description("Number of stacked frequencies."),
                PortDef::input("n1", BrushWireType::Scalar)
                    .with_range(0.1, 16.0, 1.0)
                    .with_natural_range(0.1, 16.0)
                    .with_label("n1")
                    .with_unit(UnitType::Raw)
                    .with_visible_when("algorithm", [ALGO_SUPERFORMULA as i32])
                    .with_description("Overall fatness/sharpness."),
                PortDef::input("n2", BrushWireType::Scalar)
                    .with_range(0.1, 16.0, 1.0)
                    .with_natural_range(0.1, 16.0)
                    .with_label("n2")
                    .with_unit(UnitType::Raw)
                    .with_visible_when("algorithm", [ALGO_SUPERFORMULA as i32])
                    .with_description("Shape of bump rise."),
                PortDef::input("n3", BrushWireType::Scalar)
                    .with_range(0.1, 16.0, 1.0)
                    .with_natural_range(0.1, 16.0)
                    .with_label("n3")
                    .with_unit(UnitType::Raw)
                    .with_visible_when("algorithm", [ALGO_SUPERFORMULA as i32])
                    .with_description("Shape of bump fall."),

                PortDef::output("dab_size", BrushWireType::Vec2)
                    .with_description("Brush mark size in canvas pixels (for spacing/save-points)"),
            ],
            params: &[ParamDef::Enum {
                name: "algorithm",
                options: &["Sine Harmonic", "Perlin Noise", "Superformula"],
                default: 0,
            }],
            is_gpu: true,
        },
    }
}

// ── Shape math (CPU side — mirrors `shaders/brush/_shape.wgsl`) ─────────

/// All shape parameters resolved from ports/params. Bit-exact mirror of
/// `watercolor_compute::ShapeParams` (which itself mirrors
/// `circle.rs::ShapeParams`) — the centroid integrator below walks the
/// same `r(θ)` at a finer resolution than the shader, and the
/// `circle_node.rs` centroid alignment test would flag any drift.
#[derive(Copy, Clone)]
struct ShapeParams {
    algorithm: u32,
    amplitude: f32,
    frequency: f32,
    phase: f32,
    persistence: f32,
    seed: f32,
    octaves: u32,
    n1: f32,
    n2: f32,
    n3: f32,
}

impl ShapeParams {
    fn from_ctx(ctx: &EvalContext) -> Self {
        let algorithm = match ctx.params.first() {
            Some(ParamValue::Int(v)) => (*v as u32).min(2),
            _ => 0,
        };
        ShapeParams {
            algorithm,
            amplitude: ctx.input_f32("amplitude").max(0.0),
            // Frequency must be an integer for r(θ) to close at θ = ±π.
            // Snap here in case a wired-in modulator bypasses the slider.
            frequency: ctx.input_f32("frequency").round().max(1.0),
            phase: ctx.input_f32("phase") + ctx.input_f32("phase_input"),
            persistence: ctx.input_f32("persistence").clamp(0.0, 1.0),
            seed: ctx.input_f32("seed"),
            octaves: (ctx.input_f32("octaves").round() as u32).clamp(1, 6),
            n1: ctx.input_f32("n1").max(0.05),
            n2: ctx.input_f32("n2").max(0.05),
            n3: ctx.input_f32("n3").max(0.05),
        }
    }

    /// Conservative upper bound on `r(θ)`. Bit-exact with
    /// `circle.rs::ShapeParams::r_max_unit`.
    fn r_max_unit(&self) -> f32 {
        match self.algorithm {
            ALGO_SINE => 1.0 + self.amplitude,
            ALGO_PERLIN => 1.0 + self.amplitude,
            ALGO_SUPERFORMULA => {
                let mut max_r: f32 = 0.0;
                for i in 0..32 {
                    let theta = -std::f32::consts::PI + (i as f32) * std::f32::consts::TAU / 32.0;
                    max_r = max_r.max(superformula_r(self, theta));
                }
                max_r.max(1.0)
            }
            _ => 1.0,
        }
    }
}

fn r_theta(p: &ShapeParams, theta: f32) -> f32 {
    let theta = theta + p.phase;
    match p.algorithm {
        ALGO_SINE => 1.0 + p.amplitude * (p.frequency * theta).sin(),
        ALGO_PERLIN => {
            let t = theta / std::f32::consts::TAU;
            let t = t - t.floor();
            1.0 + p.amplitude * (2.0 * fbm_1d(t, p) - 1.0)
        }
        ALGO_SUPERFORMULA => superformula_r(p, theta),
        _ => 1.0,
    }
}

fn superformula_r(p: &ShapeParams, theta: f32) -> f32 {
    let m_quarter = p.frequency * theta * 0.25;
    let term_a = (m_quarter.cos().abs()).powf(p.n2);
    let term_b = (m_quarter.sin().abs()).powf(p.n3);
    let sum = term_a + term_b;
    if sum <= 0.0 {
        return 0.0;
    }
    sum.powf(-1.0 / p.n1)
}

fn fbm_1d(t: f32, p: &ShapeParams) -> f32 {
    let mut sum = 0.0_f32;
    let mut norm = 0.0_f32;
    let mut amp = 1.0_f32;
    for o in 0..p.octaves {
        let freq = (p.frequency as i32).max(1) << o;
        let x = t * freq as f32;
        let i = x.floor();
        let f = x - i;
        let s = f * f * (3.0 - 2.0 * f);
        let a = hash1d(i.rem_euclid(freq as f32), p.seed);
        let b = hash1d((i + 1.0).rem_euclid(freq as f32), p.seed);
        sum += amp * (a * (1.0 - s) + b * s);
        norm += amp;
        amp *= p.persistence;
    }
    if norm > 0.0 {
        sum / norm
    } else {
        0.5
    }
}

fn hash1d(x: f32, seed: f32) -> f32 {
    let xi = x as u32;
    let si = seed as u32;
    let mut h = xi.wrapping_add(si.wrapping_mul(2654435761));
    h ^= h >> 16;
    h = h.wrapping_mul(0x85ebca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2ae35);
    h ^= h >> 16;
    (h as f32) / (u32::MAX as f32)
}

/// Numerically integrate the shape's centroid. Mirrors
/// `watercolor_compute::integrate_centroid` (and ultimately
/// `circle.rs::integrate_centroid`).
fn integrate_centroid(p: &ShapeParams) -> (f32, f32) {
    let n = CENTROID_SAMPLES;
    let dtheta = std::f32::consts::TAU / n as f32;
    let mut area2 = 0.0_f32;
    let mut mx3 = 0.0_f32;
    let mut my3 = 0.0_f32;
    for i in 0..n {
        let theta = -std::f32::consts::PI + (i as f32 + 0.5) * dtheta;
        let r = r_theta(p, theta);
        let r2 = r * r;
        let r3 = r2 * r;
        area2 += r2 * dtheta;
        mx3 += r3 * theta.cos() * dtheta;
        my3 += r3 * theta.sin() * dtheta;
    }
    let area = 0.5 * area2;
    if area.abs() < 1e-6 {
        return (0.0, 0.0);
    }
    let cx = (mx3 / 3.0) / area;
    let cy = (my3 / 3.0) / area;
    (cx, cy)
}

// ── Evaluator ───────────────────────────────────────────────────────────

pub struct WatercolorBatchedEvaluator;

impl WatercolorBatchedEvaluator {
    /// Natural-unit reference disc radius in canvas pixels — matches
    /// `paint::effective_radius` and `watercolor_compute::effective_radius`
    /// so a brush built around this terminal feels identical at the
    /// same port values.
    fn effective_radius(ctx: &EvalContext) -> f32 {
        let size_input = ctx.input_f32("size_input").max(0.0);
        let size = ctx.input_f32("size").max(0.0);
        let effective_size = size_input * size;
        (effective_size * SIZE_REFERENCE_PX * 0.5).max(0.5)
    }
}

impl BrushNodeEvaluator for WatercolorBatchedEvaluator {
    fn supports_erase(&self) -> bool {
        // Erase on a wet smudge brush isn't meaningful. Match
        // `watercolor_compute` and the fragment-path `watercolor`.
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
        let Some(paint_target) = gpu.paint_target.as_ref() else {
            return vec![];
        };

        let position = ctx.input("position").as_vec2();
        let radius = Self::effective_radius(ctx);
        let softness = ctx.input_f32("softness").clamp(0.0, 1.0);
        let deposit = ctx.input_f32("deposit").clamp(0.0, 1.0);
        let wetness = ctx.input_f32("wetness").clamp(0.0, 1.0);
        let flow = ctx.input_f32("flow").clamp(0.0, 1.0);
        let stroke_opacity = ctx.input_f32("opacity").clamp(0.0, 1.0);
        let mut color = ctx.input("color").as_color();
        // Fold flow into the paint colour's alpha. The composite's load
        // math reads `paint_color.a` as the maximum-deposit ceiling, so
        // a low-flow dab smudges proportionally less paint into the load
        // even at high deposit.
        color[3] *= flow;

        let shape = ShapeParams::from_ctx(ctx);
        let r_max_unit = shape.r_max_unit();
        let half_extent = radius * r_max_unit;
        let diameter = 2.0 * half_extent;

        if diameter <= 0.0 {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        }

        // Layer-clip the dab footprint. Off-canvas dabs early-out so we
        // don't push spurious save-points.
        let canvas_ext = paint_target.canvas_extent();
        let layer_x0 = canvas_ext.x0() as f32;
        let layer_y0 = canvas_ext.y0() as f32;
        let layer_x1 = layer_x0 + canvas_ext.width as f32;
        let layer_y1 = layer_y0 + canvas_ext.height as f32;
        let cx0 = (position[0] - half_extent).max(layer_x0);
        let cy0 = (position[1] - half_extent).max(layer_y0);
        let cx1 = (position[0] + half_extent).min(layer_x1);
        let cy1 = (position[1] + half_extent).min(layer_y1);
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

        // Per-flush union bbox (layer-local), workload metric for the bench.
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

        let (cx_offset, cy_offset) = integrate_centroid(&shape);

        gpu.queue_dab(&WatercolorDabRecord {
            pos: position,
            radius,
            r_max_unit,
            centroid: [cx_offset, cy_offset],
            softness,
            deposit,
            wetness,
            stroke_opacity,
            algorithm: shape.algorithm,
            amplitude: shape.amplitude,
            frequency: shape.frequency,
            phase: shape.phase,
            persistence: shape.persistence,
            seed: shape.seed,
            octaves: shape.octaves,
            n1: shape.n1,
            n2: shape.n2,
            n3: shape.n3,
            color,
        });

        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    /// Seed scratch from the pre-stroke layer snapshot so commit's
    /// scratch→layer blit reproduces unchanged pixels outside the dab
    /// footprint. Same shape as `watercolor_compute::begin_stroke` —
    /// pickup itself reads `pre_stroke_texture` directly, not scratch,
    /// so this copy is only needed for the commit path.
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

    /// Phase-end batched draws. Two render passes:
    ///
    /// 1. Pickup atlas pass — N instances; each writes its 1×1 pickup
    ///    to its own cell in the atlas, sampling `pre_stroke_texture`.
    /// 2. Composite pass — N instanced quads; each reads its atlas
    ///    cell + selection, runs procedural shape math + watercolor
    ///    load, emits premultiplied source-over via hardware blend.
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

        gpu.perf
            .record_dab_flush_workload(total_dabs, union_w, union_h);

        let pre_stroke_bg = match gpu.pre_stroke_bind_group {
            Some(bg) => bg,
            None => return,
        };
        let pre_stroke_tex = match gpu.pre_stroke_texture {
            Some(t) => t,
            None => return,
        };
        let pre_stroke_size = [pre_stroke_tex.width(), pre_stroke_tex.height()];

        let paint_target = gpu
            .paint_target
            .as_ref()
            .expect("watercolor_batched::flush_dabs requires paint_target");
        let canvas_ext = paint_target.canvas_extent();
        // pre_stroke is anchored to the layer at stroke start; if the
        // layer grew mid-stroke the layer offset may have changed, but
        // pre_stroke still represents the original layer's content at
        // its original canvas position. Sampling at `(canvas_pos -
        // pre_stroke_origin) / pre_stroke_size` gives the right pixels
        // regardless of mid-stroke layer growth.
        let pre_stroke_origin = [canvas_ext.x0(), canvas_ext.y0()];
        let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];
        let layer_size = [canvas_ext.width, canvas_ext.height];

        let pipeline_ref = gpu
            .pipelines
            .get::<WatercolorBatchedPipeline>("watercolor_batched");
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("watercolor_batched::flush_dabs requires Scratch");

        // Upload dab records once for both passes.
        gpu.queue.write_buffer(
            &pipeline_ref.dabs_buffer,
            0,
            bytemuck::cast_slice(&dab_bytes),
        );

        // ── Pass 1: pickup atlas ─────────────────────────────────────
        let pickup_uniforms = PickupUniforms {
            pre_stroke_origin,
            pre_stroke_size,
            atlas_width: ATLAS_WIDTH,
            atlas_height: ATLAS_HEIGHT,
            _pad0: 0,
            _pad1: 0,
        };
        let pickup_offset = pipeline_ref
            .pickup_uniform_ring
            .write(gpu.queue, bytemuck::bytes_of(&pickup_uniforms));
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("watercolor-batched-pickup"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &pipeline_ref.atlas_attachment_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        // Clear is cheap on a 128×128 atlas and rules
                        // out a stale-pixel bug if a future dab count
                        // shrinks between flushes within the same
                        // stroke.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(0.0, 0.0, ATLAS_WIDTH as f32, ATLAS_HEIGHT as f32, 0.0, 1.0);
            pass.set_pipeline(&pipeline_ref.pickup_pipeline);
            pass.set_bind_group(0, &pipeline_ref.pickup_uniform_bind_group, &[pickup_offset]);
            pass.set_bind_group(1, &pipeline_ref.dabs_bind_group, &[]);
            pass.set_bind_group(2, pre_stroke_bg, &[]);
            pass.draw(0..6, 0..total_dabs);
        }

        // ── Pass 2: composite ────────────────────────────────────────
        let composite_uniforms = CompositeUniforms {
            layer_offset,
            layer_size,
            canvas_size: [gpu.canvas_width, gpu.canvas_height],
            atlas_width: ATLAS_WIDTH,
            atlas_height: ATLAS_HEIGHT,
        };
        let composite_offset = pipeline_ref
            .composite_uniform_ring
            .write(gpu.queue, bytemuck::bytes_of(&composite_uniforms));
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("watercolor-batched-composite"),
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
            pass.set_pipeline(&pipeline_ref.composite_pipeline);
            pass.set_bind_group(
                0,
                &pipeline_ref.composite_uniform_bind_group,
                &[composite_offset],
            );
            pass.set_bind_group(1, &pipeline_ref.dabs_bind_group, &[]);
            pass.set_bind_group(2, gpu.selection_bind_group, &[]);
            pass.set_bind_group(3, &pipeline_ref.atlas_bind_group, &[]);
            pass.draw(0..6, 0..total_dabs);
        }

        gpu.perf.record_dab_flush(total_dabs);
    }

    /// Direct blit scratch → layer. The scratch already holds the
    /// finished image (pre_stroke + watercolor dabs accumulated via
    /// hardware blend by `flush_dabs`).
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

    /// Hover preview — procedural soft disc, lifted unchanged from
    /// `watercolor_compute`. We don't render the modulated r(θ) shape
    /// because the cursor is a UX hint about *where* the brush will
    /// land, not an exact stamp preview.
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

        let _radius = Self::effective_radius(ctx);
        let softness = ctx.input_f32("softness").clamp(0.0, 1.0);

        let pipeline_ref = gpu
            .pipelines
            .get::<WatercolorBatchedPipeline>("watercolor_batched");
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
            label: Some("watercolor-batched-preview"),
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
            let radius_px = Self::effective_radius(ctx);
            gpu.brush_preview_info = Some(crate::brush::eval::BrushPreviewInfo {
                half_extent_canvas_px: [radius_px, radius_px],
                rotation_rad: 0.0,
            });
        }

        vec![]
    }
}
