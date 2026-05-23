//! Watercolor compute terminal — folds the procedural-circle stamp,
//! alpha-weighted pickup, and per-fragment watercolor composite into a
//! single compute dispatch per render-phase. Same physical model as the
//! fragment-path `watercolor` terminal (pickup → load = mix(canvas,
//! paint, deposit) → stamp through coverage × wetness × selection ×
//! stroke_opacity × load_alpha), only every dab of an event runs in one
//! workgroup loop instead of two render passes per dab.
//!
//! ## Lifecycle
//!
//! - `begin_stroke` — Copy `pre_stroke_texture` → scratch texture so the
//!   first dab's pickup reads real canvas pixels. The compute buffer is
//!   re-seeded from the texture on every `flush_compute`'s
//!   `sync_texture_to_compute_buffer`, so no explicit buffer clear is
//!   needed.
//! - `evaluate_gpu` (per dab) — Build `WatercolorDabRecord`, push to
//!   `gpu.pending_compute_dab_bytes`. No render passes.
//! - `flush_compute` (per render phase) — One compute dispatch processes
//!   the whole queue with `storageBarrier()` between dabs. Single
//!   `copy_buffer_to_texture` syncs the buffer back to the scratch
//!   texture for the upcoming commit.
//! - `commit` — `commit_scratch_blit` copies the finished scratch over
//!   the layer. `gpu.blend_mode` ignored — erase on a wet smudge brush
//!   isn't meaningful.
//! - `render_preview` — Procedural soft-disc preview, same approach as
//!   `paint_compute` (we own the procedural shape inline, no upstream
//!   `brush_preview` texture exists).

use std::any::Any;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipeline::{
    BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::{ParamDef, ParamValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

const SIZE_REFERENCE_PX: f32 = crate::brush::dab_pool::DAB_REFERENCE_SIZE as f32;

/// Max dabs queued in one compute dispatch. Same cap as `paint_compute`.
/// At 96 bytes per `WatercolorDabRecord`, 1024 dabs is ~96 KB — well
/// within a single allocation.
const MAX_DABS_PER_DISPATCH: u32 = 1024;

/// θ-samples for the CPU-side centroid integration. Mirrors
/// `circle.rs::CENTROID_SAMPLES` — keep the two in sync.
const CENTROID_SAMPLES: usize = 256;

const ALGO_SINE: u32 = 0;
const ALGO_PERLIN: u32 = 1;
const ALGO_SUPERFORMULA: u32 = 2;

// ── Dab record ──────────────────────────────────────────────────────────

/// One queued watercolor dab. Layout MUST match the `Dab` struct in
/// `shaders/brush/watercolor_compute.wgsl` — the WGSL storage buffer
/// reinterprets these bytes verbatim.
///
/// 96 bytes per record. The layout is field-by-field std430-compatible:
/// `vec2` is 8-byte aligned, `vec4` is 16-byte aligned, and `color`
/// sits at byte offset 80 (multiple of 16). Total size is divisible by
/// the largest member alignment (16) so the WGSL `array<Dab>` index
/// arithmetic lines up.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WatercolorDabRecord {
    pub pos: [f32; 2],
    /// Natural-unit reference disc radius in canvas pixels (r==1 in the
    /// shape's natural units maps to this many pixels for the
    /// unmodulated disc).
    pub radius: f32,
    /// Conservative upper bound on `r(θ)` over the full revolution.
    /// Used by the shader to size the per-dab bbox so the modulated
    /// silhouette never falls outside the tile-walked region.
    pub r_max_unit: f32,
    /// Natural-unit centroid offset — CPU-integrated per dab. The
    /// shader adds this to the pole-relative coordinate so asymmetric
    /// shapes (sine n=1, low-m superformula, Perlin) land their
    /// geometric centre on the pen tip.
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
    pub color: [f32; 4],
}

// ── Pipeline ────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct WatercolorComputeUniforms {
    union_origin: [u32; 2],
    union_size: [u32; 2],
    layer_offset: [i32; 2],
    layer_size: [u32; 2],
    canvas_size: [u32; 2],
    aligned_width: u32,
    dab_count: u32,
    _pad0: u32,
    _pad1: u32,
}

pub struct WatercolorComputePipeline {
    pipeline: wgpu::ComputePipeline,
    uniform_ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
    dabs_buffer: wgpu::Buffer,
    dabs_bind_group: wgpu::BindGroup,
    scratch_bgl: wgpu::BindGroupLayout,
    preview_pipeline: wgpu::RenderPipeline,
    preview_uniform_buffer: wgpu::Buffer,
    preview_uniform_bind_group: wgpu::BindGroup,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PreviewUniforms {
    softness: f32,
    _pad: [f32; 3],
}

impl WatercolorComputePipeline {
    fn build(ctx: &BuildContext) -> Self {
        // Compute shader = source_over prelude + shape prelude + the
        // terminal-specific compute kernel.
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("watercolor-compute"),
                source: wgpu::ShaderSource::Wgsl(
                    concat!(
                        include_str!("../../../../../shaders/source_over.wgsl"),
                        "\n",
                        include_str!("../../../../../shaders/brush/_shape.wgsl"),
                        "\n",
                        include_str!("../../../../../shaders/brush/watercolor_compute.wgsl"),
                    )
                    .into(),
                ),
            });

        let dabs_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("watercolor-compute-dabs-bgl"),
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

        let scratch_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("watercolor-compute-scratch-bgl"),
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
                label: Some("watercolor-compute-layout"),
                bind_group_layouts: &[ctx.uniform_bgl, &dabs_bgl, ctx.selection_bgl, &scratch_bgl],
                immediate_size: 0,
            });

        let pipeline = ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("watercolor-compute"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            });

        let (uniform_ring, uniform_bind_group) = ctx
            .make_uniform_ring::<WatercolorComputeUniforms>(
                "watercolor-compute-uniforms",
                "watercolor-compute-uniform-bg",
            );

        let dabs_buffer_size =
            (MAX_DABS_PER_DISPATCH as u64) * (std::mem::size_of::<WatercolorDabRecord>() as u64);
        let dabs_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("watercolor-compute-dabs-buffer"),
            size: dabs_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dabs_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor-compute-dabs-bg"),
            layout: &dabs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dabs_buffer.as_entire_binding(),
            }],
        });

        // ── Preview pipeline ─────────────────────────────────────────
        // Procedural soft disc — same approach as paint_compute. We
        // don't render the modulated r(θ) shape in the preview because
        // (a) it'd require its own quad pipeline and (b) the hover
        // cursor is a UX hint, not a precise stamp preview.
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
                label: Some("watercolor-compute-preview"),
                source: wgpu::ShaderSource::Wgsl(preview_shader_src.into()),
            });
        let preview_uniform_bgl =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("watercolor-compute-preview-uniform-bgl"),
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
                label: Some("watercolor-compute-preview-layout"),
                bind_group_layouts: &[&preview_uniform_bgl],
                immediate_size: 0,
            });
        let preview_pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("watercolor-compute-preview"),
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
            label: Some("watercolor-compute-preview-uniform"),
            size: std::mem::size_of::<PreviewUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let preview_uniform_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor-compute-preview-uniform-bg"),
            layout: &preview_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: preview_uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            uniform_ring,
            uniform_bind_group,
            dabs_buffer,
            dabs_bind_group,
            scratch_bgl,
            preview_pipeline,
            preview_uniform_buffer,
            preview_uniform_bind_group,
        }
    }

    pub fn scratch_bgl(&self) -> &wgpu::BindGroupLayout {
        &self.scratch_bgl
    }
}

impl BrushPipelineEntry for WatercolorComputePipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        Some(&self.uniform_ring)
    }
}

fn watercolor_compute_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "watercolor_compute",
        build: |ctx| Box::new(WatercolorComputePipeline::build(ctx)),
    }
}

// ── Node ────────────────────────────────────────────────────────────────

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![watercolor_compute_pipeline_reg()],
        node: NodeRegistration {
            type_id: "watercolor_compute",
            category: "output",
            display_name: "Watercolor (Compute)",
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

pub struct WatercolorComputeEvaluator;

/// All shape parameters resolved from ports/params, in the units the
/// shader and the centroid integrator both expect. Same shape as the
/// fragment-path `circle.rs::ShapeParams`.
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

    /// Conservative upper bound on `r(θ)`. Bit-exact copy of
    /// `circle.rs::ShapeParams::r_max_unit` — the centroid alignment
    /// test in `circle_node.rs` enforces this stays consistent.
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

// Bit-exact mirrors of `circle.rs` (which mirrors `circle.wgsl` /
// `_shape.wgsl`). The CPU side runs the same math at higher resolution
// for centroid integration.
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
/// `circle.rs::integrate_centroid` — see that function's docstring for
/// the math derivation.
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

impl WatercolorComputeEvaluator {
    /// Natural-unit reference disc radius in canvas pixels. The shader
    /// converts pole-relative coords back to natural units by dividing
    /// by this value.
    fn effective_radius(ctx: &EvalContext) -> f32 {
        let size_input = ctx.input_f32("size_input").max(0.0);
        let size = ctx.input_f32("size").max(0.0);
        let effective_size = size_input * size;
        (effective_size * SIZE_REFERENCE_PX * 0.5).max(0.5)
    }
}

impl BrushNodeEvaluator for WatercolorComputeEvaluator {
    fn supports_erase(&self) -> bool {
        // Watercolor's commit is a direct blit (the scratch already
        // holds the finished image), so erase doesn't have a well-
        // defined meaning here. Match the fragment-path terminal.
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
        // Fold flow into the paint colour's alpha. Watercolor's load
        // math reads `paint_color.a` as the maximum-deposit ceiling, so
        // a low-flow dab smudges proportionally less paint into the
        // load even at high deposit.
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

        // Save-point bbox in canvas coords (Storage Frame Rule).
        let bbox_x = cx0.floor() as i32;
        let bbox_y = cy0.floor() as i32;
        let bbox_w = (cx1.ceil() as i32 - bbox_x) as u32;
        let bbox_h = (cy1.ceil() as i32 - bbox_y) as u32;
        gpu.push_dab_write_bbox(crate::coord::CanvasRect::from_xywh(
            bbox_x, bbox_y, bbox_w, bbox_h,
        ));

        // The pickup pass also READS pixels from the same footprint, so
        // the row range must cover both read and write — which here are
        // identical (the dab's bbox).
        let local_y0 = (bbox_y - canvas_ext.y0()).max(0) as u32;
        let local_y1 = local_y0 + bbox_h;
        gpu.pending_dabs_row_range = Some(match gpu.pending_dabs_row_range {
            Some([y0, y1]) => [y0.min(local_y0), y1.max(local_y1)],
            None => [local_y0, local_y1],
        });

        let (cx_offset, cy_offset) = integrate_centroid(&shape);

        gpu.queue_compute_dab(&WatercolorDabRecord {
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

    /// Seed scratch from the pre-stroke layer snapshot so the first
    /// dab's pickup reads real canvas pixels. The compute buffer is
    /// re-seeded from the texture on the next `flush_compute` via
    /// `sync_texture_to_compute_buffer`, so no separate buffer-side
    /// initialisation is needed here.
    fn begin_stroke(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let scratch = gpu
            .scratch
            .as_deref_mut()
            .expect("watercolor_compute::begin_stroke requires Scratch");
        scratch.ensure_compute_buffer(gpu.device);
        gpu.clear_compute_dabs();

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

    /// Same shape as `paint_compute::flush_compute` — see that
    /// function's comments for the texture↔buffer sync rationale.
    fn flush_compute(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.pending_compute_dab_count == 0 {
            return;
        }
        let t_dispatch = web_time::Instant::now();

        let row_range = gpu.pending_dabs_row_range.unwrap_or([0, 0]);
        let union_y0 = row_range[0];
        let union_y1 = row_range[1];
        let union_h = union_y1.saturating_sub(union_y0);

        let (dab_bytes, total_dabs) = gpu.take_compute_dabs();

        let pipeline_ref = gpu
            .pipelines
            .get::<WatercolorComputePipeline>("watercolor_compute");
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("watercolor_compute::flush_compute requires Scratch");
        let Some(scratch_buf) = scratch.compute_buffer() else {
            return;
        };
        let aligned_width = scratch.compute_aligned_width();
        let (write_w, write_h) = scratch.write_dimensions();
        let scratch_bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("watercolor-compute-scratch-bg"),
            layout: pipeline_ref.scratch_bgl(),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: scratch_buf.as_entire_binding(),
            }],
        });

        let paint_target = gpu
            .paint_target
            .as_ref()
            .expect("watercolor_compute::flush_compute requires paint_target");
        let canvas_ext = paint_target.canvas_extent();
        let layer_offset = [canvas_ext.x0(), canvas_ext.y0()];

        let dabs: &[WatercolorDabRecord] = bytemuck::cast_slice(&dab_bytes);
        let union_origin = [0u32, union_y0];
        let union_size = [write_w, union_h];

        if union_h > 0 && write_h > 0 {
            scratch.sync_texture_to_compute_buffer(&mut gpu.encoder, union_y0, union_h);
        }

        for chunk in dabs.chunks(MAX_DABS_PER_DISPATCH as usize) {
            gpu.queue
                .write_buffer(&pipeline_ref.dabs_buffer, 0, bytemuck::cast_slice(chunk));

            let uniforms = WatercolorComputeUniforms {
                union_origin,
                union_size,
                layer_offset,
                layer_size: [canvas_ext.width, canvas_ext.height],
                canvas_size: [gpu.canvas_width, gpu.canvas_height],
                aligned_width,
                dab_count: chunk.len() as u32,
                _pad0: 0,
                _pad1: 0,
            };
            let uniform_offset = pipeline_ref
                .uniform_ring
                .write(gpu.queue, bytemuck::bytes_of(&uniforms));

            {
                let mut pass = gpu
                    .encoder
                    .begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("watercolor-compute-dispatch"),
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

        let t_sync = web_time::Instant::now();
        if union_h > 0 && write_h > 0 {
            scratch.sync_compute_buffer_to_texture(&mut gpu.encoder, union_y0, union_h);
        }
        gpu.perf
            .record_compute_buffer_sync(t_sync.elapsed().as_micros() as u64);

        gpu.perf.record_compute_dispatch_batch(total_dabs);
        gpu.perf
            .record_compute_dispatch(t_dispatch.elapsed().as_micros() as u64);
    }

    /// Direct blit scratch → layer. The scratch already holds the
    /// finished image (pre_stroke + watercolor dabs blended into the
    /// buffer by `flush_compute`, then synced back to the texture).
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

    /// Hover preview — procedural soft disc, same approach as
    /// `paint_compute`. We don't render the modulated r(θ) shape here
    /// because the preview is a UX hint about *where* the brush will
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

        let radius = Self::effective_radius(ctx);
        let softness = ctx.input_f32("softness").clamp(0.0, 1.0);

        let pipeline_ref = gpu
            .pipelines
            .get::<WatercolorComputePipeline>("watercolor_compute");
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
            label: Some("watercolor-compute-preview"),
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
            let half_extent = radius * ShapeParams::from_ctx(ctx).r_max_unit();
            gpu.brush_preview_info = Some(crate::brush::eval::BrushPreviewInfo {
                half_extent_canvas_px: [half_extent, half_extent],
                rotation_rad: 0.0,
            });
        }
        vec![]
    }
}
