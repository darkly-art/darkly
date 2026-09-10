//! Watercolor terminal: two-pass batched watercolor with a per-brush
//! compiled composite shader.
//!
//! Structural shape mirrors [`paint`](super::paint),
//! with one extra pass at the front:
//!
//! 1. **Pickup atlas pass.** One instanced draw, N quads: each writes the
//!    8×8 alpha-weighted neighborhood average of the *pre-stroke snapshot*
//!    at the dab's footprint into its cell in a 128×128 atlas. That is the
//!    dry colour the pigment mixes away from, and it is frozen for the
//!    whole stroke: buildup across passes comes from the deposit channel
//!    below, not from feeding the mark back into its own input. The shader
//!    is brush-agnostic in math but built per-brush so its `DabRecord`
//!    struct stride matches the compiled brush's. Cell layout is
//!    `(idx % atlas_w, idx / atlas_w)`.
//! 2. **Composite pass, one draw per dab.** The fragment shader is the
//!    framework-assembled per-brush WGSL: upstream nodes (`circle`,
//!    `paint_color`, etc.) compile inline; this terminal contributes the
//!    watercolor blend math and the `@group(3)` bindings.
//!
//! ## How a mark builds up
//!
//! The scratch carries *coverage* and saturates almost immediately. How
//! much pigment has been delivered is a second, independent quantity, and
//! it lives in a [`StrokeChannel`] hung off the composite draw as a second
//! colour attachment (see [`DEPOSIT_CHANNEL`]). Each dab folds its own
//! delivery rate in under source-over, giving `1 − Π(1−rᵢ)`, and reads the
//! value under itself to decide how far from the dry canvas toward the
//! pigment its colour sits. A texel touched once sits near the canvas; one
//! dwelt on converges on the brush colour.
//!
//! A dab reads that field **once**, at its own centre, and resolves one
//! solid colour before it goes down. The stamp is flat; only its coverage
//! varies across the footprint. See `compile_wgsl`.
//!
//! ## Differences from `watercolor_batched`
//!
//! - **Shape lives upstream.** `watercolor_batched` had `algorithm`,
//!   `amplitude`, `frequency`, etc. as ports on the terminal and
//!   evaluated the procedural shape inline. Here the upstream graph
//!   provides a scalar `mask` input (typically wired from
//!   `circle.mask`), and the composite's fragment shader inlines
//!   whatever WGSL the circle node emits.
//! - **No CPU centroid integration.** `watercolor_batched` integrated
//!   the asymmetric shape's centroid on the CPU and packed it into
//!   the dab record to pin the shape to the pen tip. The compiled
//!   `circle` currently emits its silhouette centered on the local origin
//!   without translation. If the compiled shape's centroid drifts off
//!   the pen tip noticeably, restoring a centroid step is a focused
//!   follow-up.
//! - **Bind groups.** The framework's three (uniforms, dabs,
//!   selection) plus `@group(3)` for the pickup atlas. Declared via
//!   `NodeWgsl.terminal_bindings` so the extension stays scoped to
//!   this one terminal.

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
use crate::brush::scratch::StrokeChannel;
use crate::brush::wgsl::{
    pack_intrinsic_uniforms, pack_uniforms, CompileWgslCtx, CompiledBrush, NodeWgsl, WgslType,
    INTRINSIC_UNIFORMS_SIZE,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

const ATLAS_WIDTH: u32 = 128;
const ATLAS_HEIGHT: u32 = 128;

/// The atlas holds one cell per dab, addressed by instance index as
/// `(idx % ATLAS_WIDTH, idx / ATLAS_WIDTH)`. If it ever holds fewer cells
/// than a phase can queue, dab `ATLAS_WIDTH * ATLAS_HEIGHT` silently
/// aliases onto cell 0 and picks up the wrong canvas colour: no panic, no
/// validation error, just wrong pixels. The two constants are currently
/// equal, so this is exactly load-bearing.
const _: () = assert!(
    (ATLAS_WIDTH as u64) * (ATLAS_HEIGHT as u64) >= MAX_DABS_PER_PHASE as u64,
    "watercolor pickup atlas has fewer cells than MAX_DABS_PER_PHASE; \
     grow the atlas or lower the cap",
);

const MAX_UNIFORM_BYTES: usize = 1024;

/// How much pigment this stroke has delivered to each texel, in `[0, 1]`.
///
/// The scratch's own alpha is *coverage* (what the mark looks like) and
/// saturates almost immediately, which is why a dwelling mark used to stop
/// changing: every dab after the first few was compositing a fixed colour
/// under an alpha that had nowhere left to go. Deposit is the other
/// quantity: it starts at zero, each dab folds its own delivery rate in
/// under source-over, and the dab's *colour* is a function of it. A texel
/// that has been visited once sits near the canvas colour; one that has
/// been dwelt on sits at the pigment.
///
/// Source-over accumulation gives `1 − Π(1−rᵢ)` over the dabs that touched
/// the texel. Being a product it has no memory of how it was parenthesised,
/// so the value is unchanged by how dabs were grouped into pointer events
/// or by a checkpoint replay re-deriving them in different batches.
const DEPOSIT_CHANNEL: StrokeChannel = StrokeChannel {
    name: "deposit",
    format: wgpu::TextureFormat::R8Unorm,
    blend: wgpu::BlendState {
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
    },
};

// ── Pickup uniforms ─────────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PickupUniforms {
    pre_stroke_origin: [i32; 2],
    pre_stroke_size: [u32; 2],
    atlas_width: u32,
    atlas_height: u32,
    /// Fraction of the dab's nominal radius the pickup grid spans
    /// (half-extent in canvas-pixel terms is
    /// `pickup_size / dab.inv_radius_target_px`, valid in stroke mode
    /// where target px ≡ canvas px). Stroke-constant: see the
    /// `pickup_size` port on `watercolor`. It is measured against the
    /// nominal radius rather than the shape bbox because the bbox is
    /// extent-inflated (~1.4× the visible disc for Rough Watercolor),
    /// which sampled visibly wider than where the brush is marking.
    pickup_size: f32,
    _pad: f32,
}

// ── Per-brush pipeline ──────────────────────────────────────────────────

struct PerBrushPipeline {
    pickup_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    /// Uniform ring for the pickup pass. Pickup uniforms are small and
    /// per-flush, so one entry per flush is plenty.
    pickup_uniform_ring: DynamicUniformRing,
    pickup_uniform_bind_group: wgpu::BindGroup,
    /// Uniform ring for the composite pass: sized for this brush's
    /// (intrinsic + node-contributed) uniform layout.
    composite_uniform_ring: DynamicUniformRing,
    composite_uniform_bind_group: wgpu::BindGroup,
    composite_uniform_size: usize,
    /// Dab buffer shared between pickup and composite passes.
    dabs_buffer: wgpu::Buffer,
    dabs_bind_group_pickup: wgpu::BindGroup,
    dabs_bind_group_composite: wgpu::BindGroup,
    /// Pickup atlas texture and its two views: one to render into, one
    /// for the composite shader to sample at `@group(3)`.
    _atlas_texture: wgpu::Texture,
    atlas_attachment_view: wgpu::TextureView,
    atlas_sample_view: wgpu::TextureView,
    /// Per-dab deposit probe, written by the pickup pass and read by the
    /// composite at the same cell.
    _deposit_atlas: wgpu::Texture,
    deposit_atlas_attachment_view: wgpu::TextureView,
    deposit_atlas_sample_view: wgpu::TextureView,
    /// Layout for `@group(3)`. Held rather than a prebuilt bind group
    /// because the deposit mirror in it is reallocated whenever the layer
    /// grows, so the group is rebuilt each flush.
    composite_group3_bgl: wgpu::BindGroupLayout,
    canvas_copy_sampler: wgpu::Sampler,
}

impl PerBrushPipeline {
    fn build(ctx: &BuildContext, compiled: &CompiledBrush) -> Self {
        // ── Composite shader (framework-assembled per-brush) ──
        let composite_shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("watercolor-composite"),
                source: wgpu::ShaderSource::Wgsl(compiled.stroke_wgsl.clone().into()),
            });

        // ── Pickup shader (brush-specific dab record stride) ──
        let pickup_wgsl = build_pickup_shader(compiled);
        let pickup_shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("watercolor-pickup"),
                source: wgpu::ShaderSource::Wgsl(pickup_wgsl.into()),
            });

        // ── Bind group layouts ──
        let dabs_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("watercolor-dabs-bgl"),
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

        // ── Composite `@group(3)`: the pickup atlas plus the deposit
        // mirror. Both are terminal-private reads, and WebGPU's default
        // `max_bind_groups = 4` leaves no slot 4 to put the second one in,
        // so they share the group rather than the atlas reusing
        // `canvas_copy_bgl`. The mirror is read with `textureLoad`, so it
        // needs no sampler and imposes no filterability constraint on the
        // channel format.
        let composite_group3_bgl =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("watercolor-composite-group3-bgl"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                    ],
                });

        // ── Composite pipeline layout: group(0..2) standard, group(3) as above ──
        let composite_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("watercolor-composite-layout"),
                bind_group_layouts: &[
                    Some(ctx.uniform_bgl),
                    Some(&dabs_bgl),
                    Some(ctx.selection_bgl),
                    Some(&composite_group3_bgl),
                ],
                immediate_size: 0,
            });

        // ── Pickup pipeline layout ──
        let pickup_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("watercolor-pickup-layout"),
                bind_group_layouts: &[
                    Some(ctx.uniform_bgl),
                    Some(&dabs_bgl),
                    Some(ctx.canvas_copy_bgl), // pre_stroke texture+sampler
                    Some(ctx.canvas_copy_bgl), // deposit channel texture+sampler
                ],
                immediate_size: 0,
            });

        // ── Composite blend: premultiplied source-over ──
        //
        // A dab is a stamp of one solid colour, soft-edged in alpha. The
        // colour is resolved before the dab goes down (see `compile_wgsl`),
        // so the only thing varying across the footprint is coverage, and
        // the ROP composites the stamp onto whatever is already there.
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
                    label: Some("watercolor-composite"),
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
                        // Two targets, in the order the generated `FsOut`
                        // declares them: the scratch at `@location(0)`,
                        // the deposit channel at `@location(1)`.
                        targets: &[
                            Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rgba8Unorm,
                                blend: Some(composite_blend),
                                write_mask: wgpu::ColorWrites::ALL,
                            }),
                            Some(wgpu::ColorTargetState {
                                format: DEPOSIT_CHANNEL.format,
                                blend: Some(DEPOSIT_CHANNEL.blend),
                                write_mask: wgpu::ColorWrites::ALL,
                            }),
                        ],
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

        let pickup_pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("watercolor-pickup"),
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
                    // Cell-per-dab probes, written not blended: the dry
                    // canvas colour, and the deposit already under the dab.
                    targets: &[
                        Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format: DEPOSIT_CHANNEL.format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                    ],
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

        // ── Composite uniform ring ──
        let composite_uniform_size =
            (INTRINSIC_UNIFORMS_SIZE + compiled.uniform_size).max(INTRINSIC_UNIFORMS_SIZE);
        let composite_uniform_ring = DynamicUniformRing::new(
            ctx.device,
            "watercolor-composite-uniforms",
            composite_uniform_size as u64,
            ctx.min_uniform_align,
        );
        let composite_uniform_bind_group =
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("watercolor-composite-uniform-bg"),
                layout: ctx.uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &composite_uniform_ring.buffer,
                        offset: 0,
                        size: Some(composite_uniform_ring.binding_size()),
                    }),
                }],
            });

        // ── Pickup uniform ring ──
        let pickup_uniform_ring = DynamicUniformRing::new(
            ctx.device,
            "watercolor-pickup-uniforms",
            std::mem::size_of::<PickupUniforms>() as u64,
            ctx.min_uniform_align,
        );
        let pickup_uniform_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor-pickup-uniform-bg"),
            layout: ctx.uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &pickup_uniform_ring.buffer,
                    offset: 0,
                    size: Some(pickup_uniform_ring.binding_size()),
                }),
            }],
        });

        // ── Dab buffer (shared by pickup + composite) ──
        let dab_record_size = compiled.dab_record_size.max(16);
        let dabs_buffer_size = (MAX_DABS_PER_PHASE as u64) * (dab_record_size as u64);
        let dabs_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("watercolor-dabs-buffer"),
            size: dabs_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dabs_bind_group_pickup = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor-dabs-bg-pickup"),
            layout: &dabs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dabs_buffer.as_entire_binding(),
            }],
        });
        let dabs_bind_group_composite = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor-dabs-bg-composite"),
            layout: &dabs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dabs_buffer.as_entire_binding(),
            }],
        });

        // ── Pickup atlas texture ──
        let atlas_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("watercolor-atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_WIDTH,
                height: ATLAS_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let atlas_attachment_view =
            atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sample_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // ── Deposit atlas ──
        // Same cell layout as the colour atlas, one scalar per dab: the
        // mean deposit already under that dab. Computed once per dab in the
        // pickup pass rather than per fragment in the composite, where it
        // would be the same number recomputed for every texel of the stamp.
        let deposit_atlas = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("watercolor-deposit-atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_WIDTH,
                height: ATLAS_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPOSIT_CHANNEL.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let deposit_atlas_attachment_view =
            deposit_atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let deposit_atlas_sample_view =
            deposit_atlas.create_view(&wgpu::TextureViewDescriptor::default());

        let _ = dab_record_size;

        Self {
            pickup_pipeline,
            composite_pipeline,
            pickup_uniform_ring,
            pickup_uniform_bind_group,
            composite_uniform_ring,
            composite_uniform_bind_group,
            composite_uniform_size,
            dabs_buffer,
            dabs_bind_group_pickup,
            dabs_bind_group_composite,
            _atlas_texture: atlas_texture,
            atlas_attachment_view,
            atlas_sample_view,
            _deposit_atlas: deposit_atlas,
            deposit_atlas_attachment_view,
            deposit_atlas_sample_view,
            composite_group3_bgl,
            canvas_copy_sampler: ctx.canvas_copy_sampler.clone(),
        }
    }
}

// ── Pipeline registry entry ─────────────────────────────────────────────

pub struct WatercolorPipeline {
    cache: RefCell<HashMap<u64, PerBrushPipeline>>,
}

impl WatercolorPipeline {
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

impl BrushPipelineEntry for WatercolorPipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        None
    }
    fn rings(&self) -> Vec<&DynamicUniformRing> {
        // Rings owned per-brush; reset in flush_dabs.
        Vec::new()
    }
}

fn watercolor_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "watercolor",
        build: |ctx| Box::new(WatercolorPipeline::build(ctx)),
    }
}

// ── Pickup shader assembly ──────────────────────────────────────────────

/// Static portion of the pickup shader. Brush-agnostic: the pickup
/// math is identical for every watercolor brush. The per-brush
/// `DabRecord` struct is spliced in at compile time by
/// [`build_pickup_shader`] so the dab buffer stride matches the
/// composite pipeline's. Lives as a Rust string instead of a
/// standalone `.wgsl` file because the `DabRecord` struct must be
/// generated per brush: the file-level shader-compile test parses
/// every `.wgsl` in isolation and a placeholder-bearing template
/// fails that pass.
const PICKUP_SHADER_TAIL: &str = r#"
struct PickupUniforms {
    pre_stroke_origin: vec2<i32>,
    pre_stroke_size:   vec2<u32>,
    atlas_width:       u32,
    atlas_height:      u32,
    pickup_size:       f32,
    _pad:              f32,
}

@group(0) @binding(0) var<uniform> u: PickupUniforms;
@group(1) @binding(0) var<storage, read> dabs: array<DabRecord>;
@group(2) @binding(0) var t_pre_stroke: texture_2d<f32>;
@group(2) @binding(1) var s_pre_stroke: sampler;
// The deposit channel as it stands *now*. Legal to sample here because this
// pass renders to the atlas, not to the channel: the composite writes it,
// this pass only reads it, and they are separate passes.
@group(3) @binding(0) var t_deposit: texture_2d<f32>;
@group(3) @binding(1) var s_deposit: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) instance_idx: u32,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @builtin(instance_index) ii: u32,
) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vi];

    let atlas_x = f32(ii % u.atlas_width);
    let atlas_y = f32(ii / u.atlas_width);
    let pixel = vec2<f32>(atlas_x, atlas_y) + corner;
    let aw = f32(u.atlas_width);
    let ah = f32(u.atlas_height);
    let ndc = vec2<f32>(
        pixel.x / aw * 2.0 - 1.0,
        1.0 - pixel.y / ah * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.instance_idx = ii;
    return out;
}

struct PickupOut {
    // Alpha-weighted neighbourhood average of the dry canvas.
    @location(0) canvas: vec4<f32>,
    // Mean deposit under the dab: how loaded this spot already is.
    @location(1) deposit: vec4<f32>,
}

@fragment
fn fs_main(in: VertexOutput) -> PickupOut {
    let dab = dabs[in.instance_idx];
    // Pickup samples within a fraction of the dab's *nominal* radius
    // (not the bbox-inflated extent). The visible "smudge influence"
    // should track where the brush is actually marking, not the
    // worst-case shape-bbox footprint. `pickup_size` is the brush
    // property scrub, exposed on the terminal.
    //
    // STROKE-ONLY: this shader is dispatched only from the stroke
    // pipeline (it samples `t_pre_stroke`, which is unbound at preview
    // time). Under the stroke convention the dab record's
    // `inv_radius_target_px` is `1/radius_canvas_px` (target ≡ canvas),
    // so `1 / dab.inv_radius_target_px` recovers canvas-px radius and
    // `pickup_half` ends up in canvas px as the rest of the shader
    // expects. Do not dispatch this from a preview path: the
    // conversion is invalid when target ≢ canvas.
    let pickup_half = max(u.pickup_size / dab.inv_radius_target_px, 0.5);
    let half_extent = vec2<f32>(pickup_half);

    var sum_rgb = vec3<f32>(0.0);
    var sum_a = 0.0;
    let n: u32 = 8u;
    let inv_n = 1.0 / f32(n);
    let count = f32(n * n);
    let origin_f = vec2<f32>(f32(u.pre_stroke_origin.x), f32(u.pre_stroke_origin.y));
    let size_f = vec2<f32>(f32(u.pre_stroke_size.x), f32(u.pre_stroke_size.y));
    for (var j: u32 = 0u; j < n; j = j + 1u) {
        for (var i: u32 = 0u; i < n; i = i + 1u) {
            let cell = (vec2<f32>(f32(i), f32(j)) + 0.5) * inv_n;
            let canvas_pos = dab.pos + (cell - 0.5) * 2.0 * half_extent;
            let uv = (canvas_pos - origin_f) / size_f;
            if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
                continue;
            }
            // The *dry* layer only: the pre-stroke snapshot, frozen for
            // the whole stroke. This is the colour the pigment is mixing
            // away from, and it must not move while the stroke mixes away
            // from it. Compositing the in-flight scratch in here instead
            // closes a positive feedback loop: the deposit channel already
            // says how far this texel has travelled, so mixing that far
            // from a canvas that has itself already travelled multiplies
            // the two. The distance left to the pigment becomes
            // `Π(1−laidₖ) = (1−rate)^(n(n+1)/2)`, quadratic in the
            // exponent, so even a 1% rate saturates within a few passes and
            // the `deposit` port stops meaning anything. Against the dry
            // snapshot it is `(1−rate)ⁿ`, which is what the port promises.
            let dry = textureSampleLevel(t_pre_stroke, s_pre_stroke, uv, 0.0);
            sum_rgb = sum_rgb + dry.rgb * dry.a;
            sum_a = sum_a + dry.a;
        }
    }
    let avg_rgb = select(vec3<f32>(0.0), sum_rgb / sum_a, sum_a > 0.0001);
    let avg_a = sum_a / count;

    // Deposit under the dab, on its own grid.
    //
    // Averaged rather than point-sampled at the centre, and over a window
    // tied to the dab's own radius rather than to `pickup_size`. A single
    // texel is a noisy read of a field that has structure at the dab-
    // spacing scale, and the noise lands straight in the dab's colour:
    // consecutive stamps disagree and the mark looks mottled even though
    // the field is smooth. `pickup_size` is deliberately not involved:
    // it is a look control for the colour, and letting it move the read
    // would make it move the buildup rate too.
    // The deposit channel and the pre-stroke snapshot are both layer-sized
    // and layer-anchored (`flush_dabs` asserts the scratch and snapshot
    // share the frame), so one set of origin/size uniforms addresses both.
    let dep_half = vec2<f32>(0.5 / dab.inv_radius_target_px);
    var sum_dep = 0.0;
    for (var j: u32 = 0u; j < n; j = j + 1u) {
        for (var i: u32 = 0u; i < n; i = i + 1u) {
            let cell = (vec2<f32>(f32(i), f32(j)) + 0.5) * inv_n;
            let pos = dab.pos + (cell - 0.5) * 2.0 * dep_half;
            let uv = (pos - origin_f) / size_f;
            sum_dep = sum_dep + textureSampleLevel(t_deposit, s_deposit, uv, 0.0).r;
        }
    }

    var out: PickupOut;
    out.canvas = vec4<f32>(avg_rgb, avg_a);
    out.deposit = vec4<f32>(sum_dep / count, 0.0, 0.0, 0.0);
    return out;
}
"#;

/// Build the pickup shader source for a specific compiled brush. The
/// pickup math is brush-agnostic, but the `DabRecord` struct stride
/// must match the brush's dab layout, so each brush gets its own
/// pickup pipeline with the matching struct definition prepended.
fn build_pickup_shader(compiled: &CompiledBrush) -> String {
    let mut out = String::with_capacity(PICKUP_SHADER_TAIL.len() + 1024);
    out.push_str("struct DabRecord {\n");
    for f in &compiled.dab_layout {
        out.push_str(&format!("    {}: {},\n", f.name, f.ty.wgsl_name()));
    }
    out.push_str("};\n");
    out.push_str(PICKUP_SHADER_TAIL);
    out
}

// ── Node ────────────────────────────────────────────────────────────────

pub const TYPE_ID: &str = "watercolor";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![watercolor_pipeline_reg()],
        evaluator: || Box::new(WatercolorEvaluator),
        lifecycle: crate::brush::node::Lifecycle::ClearScratchToTransparent,
        scratch_format: crate::brush::node::COLOR_SCRATCH_FORMAT,
        node: NodeRegistration {
            type_id: TYPE_ID,
            category: "output",
            display_name: "Watercolor",
            description: "Output for wet-on-wet watercolor: pigment bleeds, pools, and darkens at the edges.",
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
                PortDef::input("flow", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Flow")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:droplet")
                    .exposed()
                    .with_description(
                        "Per-dab delivery rate multiplier: scales how much pigment this dab \
                         lays down, typically wired from pressure",
                    ),
                PortDef::input("opacity", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Opacity")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:fill-drip")
                    .exposed()
                    .with_description("Stroke-level opacity cap (applied at commit)"),
                PortDef::input("deposit", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.25)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Deposit")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:circle")
                    .exposed()
                    // A preview is one pass, and one pass is all `deposit`
                    // promises: the mark it leaves peaks at `deposit *
                    // wetness`, which at the shipped values is 17% of the
                    // pigment and reads as an empty tile. Watercolor's
                    // identity is what dwelling builds, and a still frame
                    // cannot dwell; pinning the rate is how a single pass
                    // states in one stroke what the brush arrives at over
                    // several. Measured: the stroke peaks at 142/255 here,
                    // against 36 at the shipped default.
                    .with_preview_value(0.8)
                    .with_description(
                        "Fraction of the remaining distance to the brush color that one pass of \
                         the brush closes. At 25%, one pass over white paper leaves a quarter of \
                         the way to the paint, a second pass reaches 44%, a third 58%. \
                         Independent of spacing and pressure.",
                    ),
                PortDef::input("wetness", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.7)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Wetness")
                    .with_unit(UnitType::Percent)
                    .exposed()
                    .with_description(
                        "How heavily each dab is laid down: scales the stamp's coverage, so \
                         lower values give a thinner, more translucent wash that needs more \
                         passes to read as solid.",
                    ),
                PortDef::input("pickup_size", BrushWireType::Scalar)
                    .with_range(0.0, 2.0, 0.8)
                    .with_natural_range(0.0, 2.0)
                    .with_label("Pickup Size")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:eye-dropper")
                    .exposed()
                    .with_description(
                        "Radius of the canvas-sampling neighborhood as a fraction of the dab radius. \
                         Smaller values keep the smudge influence local to the brush tip; larger \
                         values pull color from a wider area.",
                    ),
                PortDef::input("color", BrushWireType::Vec4)
                    .with_description("Brush color (typically wired from paint_color)"),
                PortDef::input("mask", BrushWireType::Scalar).with_description(
                    "Per-fragment shape mask (typically wired from circle.mask)",
                ),
                PortDef::output("dab_size", BrushWireType::Vec2)
                    .with_description("Brush mark size in canvas pixels"),
            ],
            is_gpu: true,
            is_terminal: true,
            supports_erase: false,
            preview_staging: None,
        },
    }
}

pub struct WatercolorEvaluator;

impl WatercolorEvaluator {
    fn effective_radius(ctx: &EvalContext) -> f32 {
        crate::brush::read_mirror_terminal::effective_radius(ctx)
    }
}

impl BrushNodeEvaluator for WatercolorEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let Some(compiled) = gpu.dab_batch.compiled_brush.clone() else {
            debug_assert!(false, "watercolor requires compiled_brush on gpu_context");
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

        let bbox_radius = radius * compiled.brush_extent_factor + compiled.brush_extent_extra_px;
        // Publish the footprint; `None` means the dab is entirely off-extent
        // and has no pixels to draw.
        if gpu
            .dab_batch
            .record_dab_footprint(paint_target, position, bbox_radius)
            .is_none()
        {
            return vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))];
        }

        gpu.dab_batch
            .queue_dab(&compiled, position, bbox_radius, radius);

        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    fn flush_dabs(&self, ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.dab_batch.count == 0 {
            return;
        }
        let Some(compiled) = gpu.dab_batch.compiled_brush.clone() else {
            debug_assert!(false, "watercolor::flush_dabs requires compiled_brush");
            return;
        };

        let (union_w, union_h) = gpu.dab_batch.batch_extent();
        let (dab_bytes, total_dabs) = gpu.dab_batch.take();
        if total_dabs == 0 {
            return;
        }
        gpu.perf
            .record_dab_flush_workload(total_dabs, union_w, union_h);

        let Some(stroke) = gpu.stroke.as_ref() else {
            return;
        };
        let pre_stroke_bg = stroke.pre_stroke_bind_group;
        let pre_stroke_size = [
            stroke.pre_stroke_texture.width(),
            stroke.pre_stroke_texture.height(),
        ];

        let pipeline_ref = gpu.pipelines.get::<WatercolorPipeline>("watercolor");

        ensure_per_brush_pipeline(gpu, pipeline_ref, &compiled);

        // Allocate the deposit channel, idempotently. A fragment cannot
        // sample the attachment it blends into, so what a dab reads is a
        // mirror, refreshed inside the composite loop below.
        {
            let device = gpu.device;
            let Some(stroke) = gpu.stroke.as_mut() else {
                return;
            };
            stroke
                .scratch
                .ensure_channels(device, &mut gpu.encoder, &[DEPOSIT_CHANNEL]);
        }

        let stroke = gpu
            .stroke
            .as_ref()
            .expect("watercolor::flush_dabs requires stroke resources");
        let scratch = &*stroke.scratch;
        let paint_target = &stroke.paint_target;
        let canvas_ext = paint_target.canvas_extent();
        let pre_stroke_origin = [canvas_ext.x0(), canvas_ext.y0()];
        let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];
        let layer_size = [canvas_ext.width, canvas_ext.height];

        // Build composite uniforms (intrinsic + node-contributed).
        let mut composite_uniform_bytes: Vec<u8> = Vec::with_capacity(MAX_UNIFORM_BYTES);
        let mut intrinsic = gpu.intrinsic_header(layer_offset, layer_size);
        // The `deposit` knob is a per-*pass* figure; the shader divides it
        // down to a per-dab rate with this. See the body in `compile_wgsl`.
        intrinsic.dabs_per_pass = ctx.dabs_per_pass();
        pack_intrinsic_uniforms(&mut composite_uniform_bytes, intrinsic);
        let outputs = gpu
            .dab_batch
            .slot_outputs
            .as_ref()
            .expect("watercolor::flush_dabs requires dab_batch.slot_outputs");
        pack_uniforms(&compiled, outputs, &mut composite_uniform_bytes);

        // Pickup size is a stroke-level scrub: the lifecycle context
        // has an empty inputs map, so `ctx.input_f32` returns the port
        // default (or the value the brush graph baked into the port).
        // A wired-per-dab `pickup_size` would need to flow through the
        // dab record; not in scope.
        // The pickup shader maps both `t_pre_stroke` and `t_scratch` with one
        // set of origin/size uniforms, which is only valid because the two
        // share the layer frame. `StrokeBuffer` grows them together.
        debug_assert_eq!(
            scratch.write_dimensions(),
            (pre_stroke_size[0], pre_stroke_size[1]),
            "scratch and pre_stroke must share the layer frame: the pickup \
             shader derives both UVs from `pre_stroke_origin`/`pre_stroke_size`",
        );

        let pickup_size = ctx.input_f32("pickup_size").clamp(0.0, 2.0);
        let pickup_uniforms = PickupUniforms {
            pre_stroke_origin,
            pre_stroke_size,
            atlas_width: ATLAS_WIDTH,
            atlas_height: ATLAS_HEIGHT,
            pickup_size,
            _pad: 0.0,
        };

        pipeline_ref.with_pipeline(compiled.topology_hash, |per_brush| {
            if composite_uniform_bytes.len() < per_brush.composite_uniform_size {
                composite_uniform_bytes.resize(per_brush.composite_uniform_size, 0);
            }
            per_brush.composite_uniform_ring.reset();
            per_brush.pickup_uniform_ring.reset();
            let composite_offset = per_brush
                .composite_uniform_ring
                .write(gpu.queue, &composite_uniform_bytes);
            let pickup_offset = per_brush
                .pickup_uniform_ring
                .write(gpu.queue, bytemuck::bytes_of(&pickup_uniforms));

            gpu.queue
                .write_buffer(&per_brush.dabs_buffer, 0, &dab_bytes);

            // ── Per dab: probe, then stamp ──
            //
            // A dab's colour depends on the deposit earlier dabs left under
            // it. Concurrent invocations of one instanced draw cannot
            // observe each other, so batching would give every dab in the
            // flush the same pre-flush answer, making the mark a function of
            // how dabs happened to be grouped into pointer events: the
            // banding in `docs/watercolor.md` §1, whose spatial period was
            // measured as the pen's travel per pointer event.
            //
            // So each dab gets its own pickup (probe the canvas and the
            // deposit under it, into one atlas cell) followed by its own
            // composite (stamp one solid colour). The probe reads the
            // deposit channel while the composite writes it; they are
            // separate passes, so the ordering is real and no copy is
            // needed to enforce it.
            let group3_bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("watercolor-composite-group3-bg"),
                layout: &per_brush.composite_group3_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&per_brush.atlas_sample_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&per_brush.canvas_copy_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(
                            &per_brush.deposit_atlas_sample_view,
                        ),
                    },
                ],
            });
            // The pickup reads the deposit channel directly: this pass
            // targets the atlas, so there is no read/write alias.
            let channel_views = scratch.channel_views();
            let deposit_read_bg = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("watercolor-pickup-deposit-bg"),
                layout: gpu.pipelines.canvas_copy_bind_group_layout(),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&channel_views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&per_brush.canvas_copy_sampler),
                    },
                ],
            });
            let attachments = [
                Some(wgpu::RenderPassColorAttachment {
                    view: scratch.write_view(),
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &channel_views[0],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ];
            let pickup_attachments = [
                Some(wgpu::RenderPassColorAttachment {
                    view: &per_brush.atlas_attachment_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &per_brush.deposit_atlas_attachment_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ];

            for i in 0..total_dabs {
                {
                    let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("watercolor-pickup"),
                        color_attachments: &pickup_attachments,
                        ..Default::default()
                    });
                    pass.set_viewport(0.0, 0.0, ATLAS_WIDTH as f32, ATLAS_HEIGHT as f32, 0.0, 1.0);
                    pass.set_pipeline(&per_brush.pickup_pipeline);
                    pass.set_bind_group(0, &per_brush.pickup_uniform_bind_group, &[pickup_offset]);
                    pass.set_bind_group(1, &per_brush.dabs_bind_group_pickup, &[]);
                    pass.set_bind_group(2, pre_stroke_bg, &[]);
                    pass.set_bind_group(3, &deposit_read_bg, &[]);
                    pass.draw(0..6, i..i + 1);
                }

                let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("watercolor-composite"),
                    color_attachments: &attachments,
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
                pass.set_pipeline(&per_brush.composite_pipeline);
                pass.set_bind_group(
                    0,
                    &per_brush.composite_uniform_bind_group,
                    &[composite_offset],
                );
                pass.set_bind_group(1, &per_brush.dabs_bind_group_composite, &[]);
                pass.set_bind_group(2, gpu.selection_bind_group, &[]);
                pass.set_bind_group(3, &group3_bind_group, &[]);
                pass.draw(0..6, i..i + 1);
            }
        });

        gpu.perf.record_dab_flush(total_dabs);
    }

    fn commit(&self, ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let Some(stroke) = gpu.stroke.as_ref() else {
            return;
        };
        let opacity = ctx.input_f32("opacity").clamp(0.0, 1.0);
        stroke.paint_target.commit_brush_dab(
            &mut gpu.encoder,
            gpu.pipelines,
            gpu.queue,
            stroke.scratch.write_bind_group(),
            gpu.selection_bind_group,
            stroke.pre_stroke_bind_group,
            opacity,
            gpu.blend_mode,
            /* fg_premultiplied */ true,
        );
    }

    /// Hover-cursor preview. Routes through the shared preview helper.
    /// The brush color × shape (perlin/sine) modulated mask reads
    /// against `sel = 1.0` (no selection clipping for the cursor) and
    /// a neutral-load preview body (overridden via
    /// [`Self::compile_cursor_preview_body`]: the stroke body samples the
    /// `@group(3)` pickup atlas, which the preview skeleton omits).
    fn render_cursor_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let radius = Self::effective_radius(ctx);
        let _ = crate::brush::wgsl::render_compiled_cursor_preview(gpu, radius);
        vec![]
    }

    /// Emit the composite fragment body: read upstream `mask` (scalar
    /// shape coverage) and `color` (straight-alpha foreground), sample
    /// the pickup atlas at this dab's cell, run the watercolor load
    /// blend, and return premultiplied RGBA.
    ///
    /// The framework's `assemble_shader` provides `d` (DabRecord), `u`
    /// (Uniforms), `local_uv`, `local_dist`, `theta`, `target_pos`,
    /// `canvas_size`, `sel`, and the `in: VsOut` fragment input (used
    /// here for `in.dab_idx` → atlas cell).
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        let mask_expr = cctx.input("mask").as_f32();
        let color_expr = cctx.input("color").as_vec4();
        let flow_expr = cctx.input("flow").as_f32();
        let deposit_expr = cctx.input("deposit").as_f32();
        let wetness_expr = cctx.input("wetness").as_f32();

        wgsl.terminal_outputs = vec![DEPOSIT_CHANNEL.name.to_string()];
        wgsl.terminal_bindings = "@group(3) @binding(0) var atlas_tex: texture_2d<f32>;\n\
             @group(3) @binding(1) var atlas_smp: sampler;\n\
             @group(3) @binding(2) var deposit_tex: texture_2d<f32>;\n"
            .to_string();
        // Atlas dimensions are baked into the shader: the per-brush
        // pipeline owns its own 128×128 atlas, so embedding the
        // constants avoids one more uniform field. If we ever vary
        // atlas size per brush, move these into the composite
        // uniforms.
        //
        // **A dab is one solid colour.** `load_rgb` and `load_alpha` are
        // resolved before the dab goes down and are constant across its
        // whole footprint; the only thing that varies per fragment is
        // `fg_a`, the shape's coverage. The stamp then composites onto the
        // scratch in the ROP.
        //
        // That is why `prior` is read at `d.pos` (the dab's own centre)
        // and not at `target_pos`. Reading the deposit per *fragment* makes
        // `load_rgb` vary across the footprint, and since neighbouring dabs
        // then disagree about the colour of the texels they share, the mark
        // comes out mottled at dab frequency even though the deposit field
        // itself is smooth. One read per dab keeps each stamp flat, and the
        // gradient across the stroke comes from consecutive dabs differing
        // by one `deposit` step, which is what makes it look continuous.
        //
        // **`deposit` is per pass of the brush, not per dab.** An artist
        // setting 30% means a stroke over black leaves 30% grey, and that
        // has to hold whatever the spacing is. A dab is not a unit anyone
        // can see: at the default 10% spacing ten of them land on every
        // texel, so charging `deposit` once per dab compounds it ten times
        // and 30% arrives as 87%. Worse, spacing is a fraction of dab
        // *diameter* and diameter tracks pressure, so the overlap count
        // (and with it the meaning of the knob) drifts inside a single
        // stroke.
        //
        //     per_dab = 1 − (1 − deposit)^(1/dabs_per_pass)
        //
        // inverts that exactly: `dabs_per_pass` of them compose back to
        // `deposit`, and the result no longer depends on how finely the
        // stroke was subdivided.
        //
        // `rate` deliberately carries no `mask`. The shape's falloff would
        // make the outer dabs deliver less than `per_dab`, so a pass would
        // land short of `deposit` by an amount set by the tip's softness:
        // the knob would drift again, this time per brush. The mark still
        // gets its soft edge, from `fg_a`; what the channel records is how
        // many times the brush passed over a texel, which is the quantity
        // the rate is stated against. Coverage stays in `fg_a`, delivery
        // stays in `rate`.
        //
        // `flow` scales the delivery rate only, and is not folded into
        // `fg_color.a` as well: one knob, counted once.
        wgsl.body = format!(
            "    let mask = clamp({mask_expr}, 0.0, 1.0);\n\
             \x20   if (mask <= 0.0) {{ discard; }}\n\
             \x20   if (sel <= 0.0) {{ discard; }}\n\
             \x20   let fg_color: vec4<f32> = {color_expr};\n\
             \x20   let flow = clamp({flow_expr}, 0.0, 1.0);\n\
             \x20   let deposit = clamp({deposit_expr}, 0.0, 1.0);\n\
             \x20   let wetness = clamp({wetness_expr}, 0.0, 1.0);\n\
             \x20   let atlas_w: u32 = {atlas_w}u;\n\
             \x20   let atlas_h: u32 = {atlas_h}u;\n\
             \x20   let atlas_x = i32(in.dab_idx % atlas_w);\n\
             \x20   let atlas_y = i32(in.dab_idx / atlas_w);\n\
             \x20   let atlas_uv = (vec2<f32>(f32(atlas_x), f32(atlas_y)) + vec2<f32>(0.5)) /\n\
             \x20       vec2<f32>(f32(atlas_w), f32(atlas_h));\n\
             \x20   let pickup = textureSampleLevel(atlas_tex, atlas_smp, atlas_uv, 0.0);\n\
             \x20   let has_canvas = pickup.a > 0.05;\n\
             \x20   let canvas_rgb = select(fg_color.rgb, pickup.rgb, has_canvas);\n\
             \x20   let dab_local = d.pos - vec2<f32>(\n\
             \x20       f32(u.intrinsic.layer_offset.x),\n\
             \x20       f32(u.intrinsic.layer_offset.y),\n\
             \x20   );\n\
             \x20   let prior = textureLoad(deposit_tex,\n\
             \x20       vec2<i32>(atlas_x, atlas_y), 0).r;\n\
             \x20   let per_dab = 1.0 - pow(1.0 - deposit,\n\
             \x20       1.0 / max(u.intrinsic.dabs_per_pass, 1.0));\n\
             \x20   let rate = sel * flow * per_dab;\n\
             \x20   let laid = prior + (1.0 - prior) * rate;\n\
             \x20   let load_rgb = mix(canvas_rgb, fg_color.rgb, laid);\n\
             \x20   let load_alpha = mix(pickup.a, fg_color.a, laid);\n\
             \x20   let fg_a = mask * sel * wetness * load_alpha;\n\
             \x20   return FsOut(\n\
             \x20       vec4<f32>(load_rgb * fg_a, fg_a),\n\
             \x20       vec4<f32>(rate, 0.0, 0.0, rate),\n\
             \x20   );\n",
            atlas_w = ATLAS_WIDTH,
            atlas_h = ATLAS_HEIGHT,
        );
        // Touch WgslType to avoid an unused-import warning if no other
        // path here references it after future edits: the type is
        // used implicitly through `pack_uniforms` / `pack_dab_record`
        // but only as a value flowing through the framework. Removing
        // the import would still compile today; leaving the touch
        // documents intent.
        let _ = std::marker::PhantomData::<WgslType>;
        Ok(wgsl)
    }

    /// Preview-mode body. The stroke body samples the `@group(3)`
    /// pickup atlas: the preview skeleton omits `@group(3)`, so this
    /// override emits a body that doesn't sample it. We keep the
    /// shape modulation (so Rough Watercolor shows its bumpy
    /// silhouette) and the brush color, but drop the atlas pickup /
    /// wetness blend: preview shows what the brush *would* deposit,
    /// not what it'd pick up.
    fn compile_cursor_preview_body(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        let mask_expr = cctx.input("mask").as_f32();
        let color_expr = cctx.input("color").as_vec4();
        let flow_expr = cctx.input("flow").as_f32();
        wgsl.body = format!(
            "    let mask = clamp({mask_expr}, 0.0, 1.0);\n\
             \x20   if (mask <= 0.0) {{ discard; }}\n\
             \x20   var fg_color: vec4<f32> = {color_expr};\n\
             \x20   let flow = clamp({flow_expr}, 0.0, 1.0);\n\
             \x20   let a = mask * flow * fg_color.a;\n\
             \x20   return vec4<f32>(fg_color.rgb * a, a);\n"
        );
        Ok(wgsl)
    }
}

// ── Per-brush pipeline build helper ─────────────────────────────────────

fn ensure_per_brush_pipeline(
    gpu: &BrushGpuContext,
    pipe: &WatercolorPipeline,
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
        texture_registry: gpu.pipelines.texture_registry(),
        baked_sources: gpu.pipelines.baked_sources(),
    };
    pipe.ensure_pipeline(&ctx, compiled);
}
