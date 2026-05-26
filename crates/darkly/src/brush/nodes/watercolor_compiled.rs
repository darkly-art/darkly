//! Watercolor Compiled terminal — two-pass batched watercolor with a
//! per-brush compiled composite shader.
//!
//! Structural shape mirrors [`paint_compiled`](super::paint_compiled),
//! with one extra pass at the front:
//!
//! 1. **Pickup atlas pass.** N instances, each writes the 8×8 alpha-
//!    weighted neighborhood average of `pre_stroke_texture` at the
//!    dab's footprint into its cell in a 128×128 atlas. The shader
//!    (`watercolor_compiled_pickup.wgsl`) is brush-agnostic in math
//!    but built per-brush so its `DabRecord` struct stride matches
//!    the compiled brush's. Cell layout is `(idx % atlas_w, idx /
//!    atlas_w)`.
//! 2. **Composite pass.** One instanced draw, N quads. The fragment
//!    shader is the framework-assembled per-brush WGSL: upstream
//!    nodes (`circle`, `paint_color`, etc.) compile inline; this
//!    terminal contributes the watercolor blend math (atlas pickup +
//!    deposit/wetness load) and the extra atlas bind group.
//!
//! ## Differences from `watercolor_batched`
//!
//! - **Shape lives upstream.** `watercolor_batched` had `algorithm`,
//!   `amplitude`, `frequency`, etc. as ports on the terminal and
//!   evaluated the procedural shape inline. Here the upstream graph
//!   provides a scalar `mask` input (typically wired from
//!   `circle.texture`), and the composite's fragment shader inlines
//!   whatever WGSL the circle node emits.
//! - **No CPU centroid integration.** `watercolor_batched` integrated
//!   the asymmetric shape's centroid on the CPU and packed it into
//!   the dab record to pin the shape to the pen tip. The compiled
//!   `circle` currently emits its shape centered on the local origin
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
use crate::brush::wgsl_compile::{
    pack_dab_record, pack_uniforms, CompileWgslCtx, CompiledBrush, NodeWgsl, WgslType,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

/// Canvas-pixel reference for `size_input * size = 1.0`. Same
/// constant as every other brush node — see
/// [`crate::brush::DAB_REFERENCE_SIZE`].
const SIZE_REFERENCE_PX: f32 = crate::brush::DAB_REFERENCE_SIZE as f32;

const ATLAS_WIDTH: u32 = 128;
const ATLAS_HEIGHT: u32 = 128;

const MAX_UNIFORM_BYTES: usize = 1024;

// ── Intrinsic uniforms ──────────────────────────────────────────────────

/// Same `IntrinsicUniforms` shape as `paint_compiled` — the composite
/// shader's `Uniforms` struct embeds this as the first field.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct IntrinsicUniforms {
    layer_offset: [i32; 2],
    layer_size: [u32; 2],
    canvas_size: [u32; 2],
    _pad: [u32; 2],
}

const INTRINSIC_UNIFORMS_SIZE: usize = std::mem::size_of::<IntrinsicUniforms>();

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PickupUniforms {
    pre_stroke_origin: [i32; 2],
    pre_stroke_size: [u32; 2],
    atlas_width: u32,
    atlas_height: u32,
    /// Fraction of the dab's nominal radius the pickup grid spans
    /// (half-extent in canvas-pixel terms is `dab.radius * pickup_size`).
    /// Stroke-constant — see the `pickup_size` port on
    /// `watercolor_compiled`. Sampling the full bbox produced visibly
    /// too-large pickup neighborhoods (the bbox is shape-extent-
    /// inflated, ~1.4× the visible disc for Rough Watercolor); a third
    /// of the nominal radius matches the "smudge from where the brush
    /// is now" intuition closer to Krita's defaults.
    pickup_size: f32,
    _pad: f32,
}

// ── Per-brush pipeline ──────────────────────────────────────────────────

struct PerBrushPipeline {
    pickup_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    /// Uniform ring for the pickup pass. Pickup uniforms are small and
    /// per-flush — one entry per flush is plenty.
    pickup_uniform_ring: DynamicUniformRing,
    pickup_uniform_bind_group: wgpu::BindGroup,
    /// Uniform ring for the composite pass — sized for this brush's
    /// (intrinsic + node-contributed) uniform layout.
    composite_uniform_ring: DynamicUniformRing,
    composite_uniform_bind_group: wgpu::BindGroup,
    composite_uniform_size: usize,
    /// Dab buffer shared between pickup and composite passes.
    dabs_buffer: wgpu::Buffer,
    dabs_bind_group_pickup: wgpu::BindGroup,
    dabs_bind_group_composite: wgpu::BindGroup,
    /// Pickup atlas texture + the bind group the composite shader reads
    /// at `@group(3)`.
    _atlas_texture: wgpu::Texture,
    atlas_attachment_view: wgpu::TextureView,
    atlas_bind_group: wgpu::BindGroup,
}

impl PerBrushPipeline {
    fn build(ctx: &BuildContext, compiled: &CompiledBrush) -> Self {
        // ── Composite shader (framework-assembled per-brush) ──
        let composite_shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("watercolor_compiled-composite"),
                source: wgpu::ShaderSource::Wgsl(compiled.wgsl.clone().into()),
            });

        // ── Pickup shader (brush-specific dab record stride) ──
        let pickup_wgsl = build_pickup_shader(compiled);
        let pickup_shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("watercolor_compiled-pickup"),
                source: wgpu::ShaderSource::Wgsl(pickup_wgsl.into()),
            });

        // ── Bind group layouts ──
        let dabs_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("watercolor_compiled-dabs-bgl"),
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

        // ── Composite pipeline layout: group(0..3) standard, group(3) atlas ──
        let composite_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("watercolor_compiled-composite-layout"),
                bind_group_layouts: &[
                    ctx.uniform_bgl,
                    &dabs_bgl,
                    ctx.selection_bgl,
                    ctx.canvas_copy_bgl, // atlas: same texture+sampler layout
                ],
                immediate_size: 0,
            });

        // ── Pickup pipeline layout ──
        let pickup_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("watercolor_compiled-pickup-layout"),
                bind_group_layouts: &[
                    ctx.uniform_bgl,
                    &dabs_bgl,
                    ctx.canvas_copy_bgl, // pre_stroke texture+sampler
                ],
                immediate_size: 0,
            });

        // ── Composite blend: premultiplied source-over ──
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
                    label: Some("watercolor_compiled-composite"),
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

        let pickup_pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("watercolor_compiled-pickup"),
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
                        blend: None,
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

        // ── Composite uniform ring ──
        let composite_uniform_size =
            (INTRINSIC_UNIFORMS_SIZE + compiled.uniform_size).max(INTRINSIC_UNIFORMS_SIZE);
        let composite_uniform_ring = DynamicUniformRing::new(
            ctx.device,
            "watercolor_compiled-composite-uniforms",
            composite_uniform_size as u64,
            ctx.min_uniform_align,
        );
        let composite_uniform_bind_group =
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("watercolor_compiled-composite-uniform-bg"),
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
            "watercolor_compiled-pickup-uniforms",
            std::mem::size_of::<PickupUniforms>() as u64,
            ctx.min_uniform_align,
        );
        let pickup_uniform_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor_compiled-pickup-uniform-bg"),
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
            label: Some("watercolor_compiled-dabs-buffer"),
            size: dabs_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dabs_bind_group_pickup = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor_compiled-dabs-bg-pickup"),
            layout: &dabs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dabs_buffer.as_entire_binding(),
            }],
        });
        let dabs_bind_group_composite = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor_compiled-dabs-bg-composite"),
            layout: &dabs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dabs_buffer.as_entire_binding(),
            }],
        });

        // ── Pickup atlas texture ──
        let atlas_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("watercolor_compiled-atlas"),
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
        let atlas_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor_compiled-atlas-bg"),
            layout: ctx.canvas_copy_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_sample_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(ctx.canvas_copy_sampler),
                },
            ],
        });

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
            atlas_bind_group,
        }
    }
}

// ── Pipeline registry entry ─────────────────────────────────────────────

pub struct WatercolorCompiledPipeline {
    cache: RefCell<HashMap<u64, PerBrushPipeline>>,
}

impl WatercolorCompiledPipeline {
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

impl BrushPipelineEntry for WatercolorCompiledPipeline {
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

fn watercolor_compiled_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "watercolor_compiled",
        build: |ctx| Box::new(WatercolorCompiledPipeline::build(ctx)),
    }
}

// ── Pickup shader assembly ──────────────────────────────────────────────

/// Static portion of the pickup shader. Brush-agnostic — the pickup
/// math is identical for every watercolor brush. The per-brush
/// `DabRecord` struct is spliced in at compile time by
/// [`build_pickup_shader`] so the dab buffer stride matches the
/// composite pipeline's. Lives as a Rust string instead of a
/// standalone `.wgsl` file because the `DabRecord` struct must be
/// generated per brush — the file-level shader-compile test parses
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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let dab = dabs[in.instance_idx];
    // Pickup samples within a fraction of the dab's *nominal* radius
    // (not the bbox-inflated extent). The visible "smudge influence"
    // should track where the brush is actually marking, not the
    // worst-case shape-bbox footprint. `pickup_size` is the brush
    // property scrub — default ≈ 0.33, exposed on the terminal.
    let pickup_half = max(dab.radius * u.pickup_size, 0.5);
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
            let s = textureSampleLevel(t_pre_stroke, s_pre_stroke, uv, 0.0);
            sum_rgb = sum_rgb + s.rgb * s.a;
            sum_a = sum_a + s.a;
        }
    }
    let avg_rgb = select(vec3<f32>(0.0), sum_rgb / sum_a, sum_a > 0.0001);
    let avg_a = sum_a / count;
    return vec4<f32>(avg_rgb, avg_a);
}
"#;

/// Build the pickup shader source for a specific compiled brush. The
/// pickup math is brush-agnostic, but the `DabRecord` struct stride
/// must match the brush's dab layout — so each brush gets its own
/// pickup pipeline with the matching struct definition prepended.
fn build_pickup_shader(compiled: &CompiledBrush) -> String {
    let mut out = String::with_capacity(PICKUP_SHADER_TAIL.len() + 256);
    out.push_str("struct DabRecord {\n");
    for f in &compiled.dab_layout {
        out.push_str(&format!("    {}: {},\n", f.name, f.ty.wgsl_name()));
    }
    out.push_str("};\n");
    out.push_str(PICKUP_SHADER_TAIL);
    out
}

// ── Node ────────────────────────────────────────────────────────────────

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![watercolor_compiled_pipeline_reg()],
        node: NodeRegistration {
            type_id: "watercolor_compiled",
            category: "output",
            display_name: "Watercolor (Compiled)",
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
                    .with_description("Per-dab flow (folded into color alpha → max-deposit ceiling)"),
                PortDef::input("opacity", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Opacity")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-fill-drip")
                    .exposed()
                    .with_description("Stroke-level opacity cap (applied at commit)"),
                PortDef::input("deposit", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.5)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Deposit")
                    .with_unit(UnitType::Percent)
                    .exposed()
                    .with_description(
                        "How strongly the brush color replaces the pickup canvas color",
                    ),
                PortDef::input("wetness", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.7)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Wetness")
                    .with_unit(UnitType::Percent)
                    .exposed()
                    .with_description("How much pickup color tints the load"),
                PortDef::input("pickup_size", BrushWireType::Scalar)
                    .with_range(0.0, 2.0, 0.5)
                    .with_natural_range(0.0, 2.0)
                    .with_label("Pickup Size")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-eye-dropper")
                    .exposed()
                    .with_description(
                        "Radius of the canvas-sampling neighborhood as a fraction of the dab radius. \
                         Smaller values keep the smudge influence local to the brush tip; larger \
                         values pull color from a wider area.",
                    ),
                PortDef::input("color", BrushWireType::Color)
                    .with_description("Brush color (typically wired from paint_color)"),
                // Typed as `Texture` to match the upstream
                // `circle.texture` output's wire type. In the compiled
                // path the wire's underlying WGSL expression is `f32`
                // (the shape coverage), but the framework checks
                // wire types for compatibility — so the declared
                // wire type must match the source. Same pattern as
                // `paint_compiled.rgba` matching `stamp.dab`.
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

pub struct WatercolorCompiledEvaluator;

impl WatercolorCompiledEvaluator {
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
}

impl BrushNodeEvaluator for WatercolorCompiledEvaluator {
    fn is_compiled_terminal(&self) -> bool {
        true
    }

    fn supports_erase(&self) -> bool {
        // Erase on wet media doesn't read naturally — match the
        // dispatch-path watercolor terminals.
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
                "watercolor_compiled requires compiled_brush on gpu_context"
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

        let record_start = gpu.pending_dab_bytes.len();
        Self::pack_intrinsic_header(&mut gpu.pending_dab_bytes, position, radius, bbox_radius);
        let outputs = gpu
            .slot_outputs_owned
            .as_ref()
            .expect("watercolor_compiled requires slot_outputs_owned");
        pack_dab_record(&compiled, outputs, &mut gpu.pending_dab_bytes);
        let written = gpu.pending_dab_bytes.len() - record_start;
        if written < compiled.dab_record_size {
            gpu.pending_dab_bytes
                .resize(record_start + compiled.dab_record_size, 0);
        }
        gpu.pending_dab_count = gpu.pending_dab_count.saturating_add(1);
        debug_assert!(
            gpu.pending_dab_count <= MAX_DABS_PER_PHASE,
            "watercolor_compiled dab queue overflowed MAX_DABS_PER_PHASE"
        );

        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    /// Seed scratch from pre_stroke so commit's scratch→layer blit
    /// reproduces unchanged pixels outside the dab footprint. Lifted
    /// verbatim from `watercolor_batched::begin_stroke`.
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

    fn flush_dabs(&self, ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.pending_dab_count == 0 {
            return;
        }
        let Some(compiled) = gpu.compiled_brush.clone() else {
            debug_assert!(
                false,
                "watercolor_compiled::flush_dabs requires compiled_brush"
            );
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

        let pre_stroke_bg = match gpu.pre_stroke_bind_group {
            Some(bg) => bg,
            None => return,
        };
        let pre_stroke_tex = match gpu.pre_stroke_texture {
            Some(t) => t,
            None => return,
        };
        let pre_stroke_size = [pre_stroke_tex.width(), pre_stroke_tex.height()];

        let pipeline_ref = gpu
            .pipelines
            .get::<WatercolorCompiledPipeline>("watercolor_compiled");

        ensure_per_brush_pipeline(gpu, pipeline_ref, &compiled);

        let scratch = gpu
            .scratch
            .as_deref()
            .expect("watercolor_compiled::flush_dabs requires Scratch");
        let paint_target = gpu
            .paint_target
            .as_ref()
            .expect("watercolor_compiled::flush_dabs requires paint_target");
        let canvas_ext = paint_target.canvas_extent();
        let pre_stroke_origin = [canvas_ext.x0(), canvas_ext.y0()];
        let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];
        let layer_size = [canvas_ext.width, canvas_ext.height];

        // Build composite uniforms (intrinsic + node-contributed).
        let mut composite_uniform_bytes: Vec<u8> = Vec::with_capacity(MAX_UNIFORM_BYTES);
        Self::pack_intrinsic_uniforms(
            &mut composite_uniform_bytes,
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
            .expect("watercolor_compiled::flush_dabs requires slot_outputs_owned");
        pack_uniforms(&compiled, outputs, &mut composite_uniform_bytes);

        // Pickup size is a stroke-level scrub — the lifecycle context
        // has an empty inputs map, so `ctx.input_f32` returns the port
        // default (or the value the brush graph baked into the port).
        // A wired-per-dab `pickup_size` would need to flow through the
        // dab record; not in scope.
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

            // ── Pass 1: pickup atlas ──
            {
                let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("watercolor_compiled-pickup"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &per_brush.atlas_attachment_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_viewport(0.0, 0.0, ATLAS_WIDTH as f32, ATLAS_HEIGHT as f32, 0.0, 1.0);
                pass.set_pipeline(&per_brush.pickup_pipeline);
                pass.set_bind_group(0, &per_brush.pickup_uniform_bind_group, &[pickup_offset]);
                pass.set_bind_group(1, &per_brush.dabs_bind_group_pickup, &[]);
                pass.set_bind_group(2, pre_stroke_bg, &[]);
                pass.draw(0..6, 0..total_dabs);
            }

            // ── Pass 2: composite ──
            {
                let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("watercolor_compiled-composite"),
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
                pass.set_pipeline(&per_brush.composite_pipeline);
                pass.set_bind_group(
                    0,
                    &per_brush.composite_uniform_bind_group,
                    &[composite_offset],
                );
                pass.set_bind_group(1, &per_brush.dabs_bind_group_composite, &[]);
                pass.set_bind_group(2, gpu.selection_bind_group, &[]);
                pass.set_bind_group(3, &per_brush.atlas_bind_group, &[]);
                pass.draw(0..6, 0..total_dabs);
            }
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

    fn render_preview(
        &self,
        _ctx: &EvalContext,
        _gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        // Compiled-brush previews are a separate handoff
        // (`handoff-brush-preview.md`). No-op for now — hover cursor
        // disappears like paint_compiled.
        vec![]
    }

    /// Emit the composite fragment body: read upstream `mask` (scalar
    /// shape coverage) and `color` (straight-alpha foreground), sample
    /// the pickup atlas at this dab's cell, run the watercolor load
    /// blend, and return premultiplied RGBA.
    ///
    /// The framework's `assemble_shader` provides `d` (DabRecord), `u`
    /// (Uniforms), `local_uv`, `local_dist`, `theta`, `canvas_pos`,
    /// `canvas_size`, `sel`, and the `in: VsOut` fragment input (used
    /// here for `in.dab_idx` → atlas cell).
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        let mask_expr = cctx.input("mask").as_f32();
        let color_expr = cctx.input("color").as_vec4();
        let flow_expr = cctx.input("flow").as_f32();
        let deposit_expr = cctx.input("deposit").as_f32();
        let wetness_expr = cctx.input("wetness").as_f32();

        wgsl.terminal_bindings = "@group(3) @binding(0) var atlas_tex: texture_2d<f32>;\n\
             @group(3) @binding(1) var atlas_smp: sampler;\n"
            .to_string();
        // Atlas dimensions are baked into the shader — the per-brush
        // pipeline owns its own 128×128 atlas, so embedding the
        // constants avoids one more uniform field. If we ever vary
        // atlas size per brush, move these into the composite
        // uniforms.
        wgsl.body = format!(
            "    let mask = clamp({mask_expr}, 0.0, 1.0);\n\
             \x20   if (mask <= 0.0) {{ discard; }}\n\
             \x20   if (sel <= 0.0) {{ discard; }}\n\
             \x20   var fg_color: vec4<f32> = {color_expr};\n\
             \x20   let flow = clamp({flow_expr}, 0.0, 1.0);\n\
             \x20   fg_color.a = fg_color.a * flow;\n\
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
             \x20   let load_rgb = mix(canvas_rgb, fg_color.rgb, deposit);\n\
             \x20   let load_alpha = mix(pickup.a, fg_color.a, deposit);\n\
             \x20   let fg_a = mask * sel * wetness * load_alpha;\n\
             \x20   return vec4<f32>(load_rgb * fg_a, fg_a);\n",
            atlas_w = ATLAS_WIDTH,
            atlas_h = ATLAS_HEIGHT,
        );
        // Touch WgslType to avoid an unused-import warning if no other
        // path here references it after future edits — the type is
        // used implicitly through `pack_uniforms` / `pack_dab_record`
        // but only as a value flowing through the framework. Removing
        // the import would still compile today; leaving the touch
        // documents intent.
        let _ = std::marker::PhantomData::<WgslType>;
        Ok(wgsl)
    }
}

// ── Per-brush pipeline build helper ─────────────────────────────────────

fn ensure_per_brush_pipeline(
    gpu: &BrushGpuContext,
    pipe: &WatercolorCompiledPipeline,
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
