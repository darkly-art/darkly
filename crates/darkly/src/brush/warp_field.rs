//! Warp field: the stroke scratch as a displacement map, and the single
//! resample that turns it back into pixels.
//!
//! A warp terminal (liquify, and any pinch/swirl/bloat sibling) does not
//! paint. Its whole stroke is expressible as one coordinate map, so
//! rasterising the intermediate states is not just wasteful, it is
//! destructive: each dab would resample the picture, and a chain of
//! bilinear filters is a low-pass cascade, not a bilinear filter. At
//! liquify's 4 px dab spacing a pixel passes under dozens of dabs per
//! swipe and the detail is gone.
//!
//! So a warp terminal's scratch holds the *map* instead of the picture.
//! Per dab it advects and accumulates a two-channel displacement in plane
//! pixels; at commit the pre-stroke snapshot is sampled **once** through
//! the accumulated field. Detail is then independent of dab count.
//!
//! This is the architecture both reference implementations converged on:
//!
//! * GEGL's `gegl:warp` iterates a two-component float coordinate buffer
//!   (`operations/common-cxx/warp.cc:321`) and per stamp does
//!   `field'(p) = field(p + nv) + nv` (`:700-704`), the exact update
//!   [`advect_wgsl`] emits. GIMP wires the accumulated buffer through
//!   `gegl:map-relative` to sample the drawable once
//!   (`app/tools/gimpwarptool.c:913,1059-1071`).
//! * Krita's `KisLiquifyTransformWorker` keeps `originalPoints` /
//!   `transformedPoints` grids (`libs/image/kis_liquify_transform_worker.cpp:31-32`),
//!   moves only the grid per touch (`:251-253`), and rasterises once from
//!   the source device in `run()` (`:414-440`).
//!
//! ## Displacement is relative, and that is load-bearing
//!
//! The field stores `source − target`, not an absolute source coordinate.
//! `Scratch::grow_write` rebases the scratch when the layer grows
//! mid-stroke; a relative delta survives that untouched, and the
//! zero-filled new region is exactly the right identity. Absolute
//! coordinates would all have to be rewritten.
//!
//! ## Why `Rg32Float`
//!
//! At full strength liquify locks pixels to the cursor, so the field is
//! the *cumulative* drag: a 1200 px drag stores values near 1200, where
//! half-float ULP is a whole pixel (measured f16-vs-f32 error over such a
//! drag: p90 3.6 px, p99 15.7 px). GEGL and GIMP both use float32 here.
//! `Rg32Float` is renderable in core WebGPU but not *filterable*, so both
//! the per-dab advect and the resolve fetch it with `textureLoad` and
//! interpolate in [`FIELD_HELPERS_WGSL`]. That also makes the resolve
//! provably exact where the field is zero: at zero displacement the
//! interpolation weights are exactly 0 and `mix` returns the source texel
//! bit-for-bit, on every backend. Since the resolve rewrites the whole
//! layer on every pen event, anything less would re-introduce the
//! softening this module exists to remove.

use std::any::Any;

use crate::brush::pipeline::{BrushPipelineEntry, BrushPipelineRegistration, BuildContext};

/// Texel format of a warp terminal's scratch. Two channels of `f32`:
/// the displacement from target pixel to source pixel, in plane pixels.
pub const FIELD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Float;

/// Registry id of the shared resolve pipeline.
pub const RESOLVE_PIPELINE_ID: &str = "warp_field_resolve";

/// Manual bilinear fetch of a two-channel field, by `textureLoad`.
///
/// `p` is in texels from the texture's origin, integer `p` naming a texel
/// *corner*, the same convention the read-mirror UV math uses, where
/// `copy_origin` is floored to integers and `target_pos` interpolates to
/// fragment centres. Hence the `-0.5` before `floor`.
///
/// Edge-clamped: a dab clipped at the layer edge addresses outside the
/// mirror's valid region, and clamping there reads the nearest valid
/// displacement rather than a stale texel.
pub const FIELD_HELPERS_WGSL: &str = "\
fn warp_field_texel(t: texture_2d<f32>, c: vec2<i32>) -> vec2<f32> {
    let dims = vec2<i32>(textureDimensions(t));
    let cc = clamp(c, vec2<i32>(0, 0), dims - vec2<i32>(1, 1));
    return textureLoad(t, cc, 0).xy;
}
fn warp_field_bilinear(t: texture_2d<f32>, p: vec2<f32>) -> vec2<f32> {
    let q    = p - vec2<f32>(0.5, 0.5);
    let base = floor(q);
    let frac = q - base;
    let b    = vec2<i32>(base);
    let top  = mix(warp_field_texel(t, b),
                   warp_field_texel(t, b + vec2<i32>(1, 0)), frac.x);
    let bot  = mix(warp_field_texel(t, b + vec2<i32>(0, 1)),
                   warp_field_texel(t, b + vec2<i32>(1, 1)), frac.x);
    return mix(top, bot, frac.y);
}
";

/// The per-dab fragment tail every warp terminal shares: advect the
/// accumulated field by this dab's offset, add the offset, write it back.
///
/// `offset_expr` must evaluate to a `vec2<f32>`: the displacement this
/// dab contributes at the current fragment, already shaped by whatever
/// falloff, selection and mask attenuation the terminal wants. Pointing
/// it *backward* along travel makes content move *forward* with the
/// cursor (GEGL does the same: `motion_x = priv->last_x - x`,
/// `warp.cc:412`).
///
/// A new warp behaviour is therefore one file supplying one expression;
/// `scale`/`rotate` offsets are functions of `local`, which the framework
/// wrapper already provides.
pub fn advect_wgsl(offset_expr: &str, copy_origin_field: &str) -> String {
    format!(
        "    let nv = {offset_expr};\n\
         \x20   let prev = warp_field_bilinear(\n\
         \x20       scratch_mirror_tex, target_pos + nv - d.{copy_origin_field});\n\
         \x20   return vec4<f32>(prev + nv, 0.0, 0.0);\n"
    )
}

// ── Resolve pipeline ────────────────────────────────────────────────────

const RESOLVE_WGSL: &str = r#"
@group(0) @binding(0) var field_tex:  texture_2d<f32>;
@group(0) @binding(1) var source_tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // Oversized triangle covering the viewport.
    var xy = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var out: VsOut;
    out.clip_pos = vec4<f32>(xy[vi], 0.0, 1.0);
    return out;
}

fn source_texel(c: vec2<i32>) -> vec4<f32> {
    let dims = vec2<i32>(textureDimensions(source_tex));
    let cc = clamp(c, vec2<i32>(0, 0), dims - vec2<i32>(1, 1));
    return textureLoad(source_tex, cc, 0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // `clip_pos.xy` is the fragment centre in layer-local pixels.
    let p = in.clip_pos.xy;
    let disp = warp_field_bilinear(field_tex, p);
    let src  = p + disp;

    // Manual bilinear rather than a sampler: where `disp` is exactly
    // zero the weights are exactly zero and this returns the source
    // texel bit-for-bit. The resolve rewrites the entire layer every
    // pen event, so an off-by-half-texel here would soften everything
    // the stroke did not touch.
    let q    = src - vec2<f32>(0.5, 0.5);
    let base = floor(q);
    let frac = q - base;
    let b    = vec2<i32>(base);
    let top  = mix(source_texel(b),
                   source_texel(b + vec2<i32>(1, 0)), frac.x);
    let bot  = mix(source_texel(b + vec2<i32>(0, 1)),
                   source_texel(b + vec2<i32>(1, 1)), frac.x);
    return mix(top, bot, frac.y);
}
"#;

/// Resolves an accumulated warp field against a source snapshot, straight
/// onto the paint target.
///
/// Two pipelines, one per destination format: raster layers are
/// `Rgba8Unorm`, mask layers `R8Unorm`. Per the type-owned-dispatch
/// principle the branch lives in [`WarpFieldResolve::pipeline`], not at
/// the call site, mirroring `CompositePipeline`.
pub struct WarpFieldResolve {
    pipeline_rgba: wgpu::RenderPipeline,
    pipeline_r8: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
}

/// Harvested by `BrushPipelines::new` alongside the other plumbing
/// pipelines, since the resolve belongs to no single node: every warp
/// terminal shares it.
pub fn warp_field_resolve_registration() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: RESOLVE_PIPELINE_ID,
        build: |ctx| Box::new(WarpFieldResolve::build(ctx)),
    }
}

impl WarpFieldResolve {
    fn build(ctx: &BuildContext) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("warp-field-resolve"),
                source: wgpu::ShaderSource::Wgsl(
                    format!("{FIELD_HELPERS_WGSL}\n{RESOLVE_WGSL}").into(),
                ),
            });

        // Both textures are fetched with `textureLoad`, so neither needs a
        // sampler and the field's non-filterability is irrelevant here.
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("warp-field-resolve-bgl"),
                entries: &[texture_entry(0), texture_entry(1)],
            });

        let layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("warp-field-resolve-layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });

        let make = |format: wgpu::TextureFormat, label: &str| {
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
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                })
        };

        Self {
            pipeline_rgba: make(wgpu::TextureFormat::Rgba8Unorm, "warp-field-resolve-rgba"),
            pipeline_r8: make(wgpu::TextureFormat::R8Unorm, "warp-field-resolve-r8"),
            bgl,
        }
    }

    fn pipeline(&self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        if format == wgpu::TextureFormat::R8Unorm {
            &self.pipeline_r8
        } else {
            &self.pipeline_rgba
        }
    }

    /// Sample `source` through `field` and write the result across
    /// `dest`'s full extent.
    ///
    /// Full extent, not a damage rect: the whole point is that the output
    /// is a pure function of the pre-stroke snapshot and the current
    /// field, so it stays correct when the stabiliser rewinds and discards
    /// dabs; a tracked rect would leave a stale warped fringe behind the
    /// truncation. It also costs no more than the full-extent scratch copy
    /// a colour terminal's commit already does.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        field_view: &wgpu::TextureView,
        source_view: &wgpu::TextureView,
        dest_view: &wgpu::TextureView,
        dest_format: wgpu::TextureFormat,
        dest_size: (u32, u32),
    ) {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("warp-field-resolve-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(field_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("warp-field-resolve"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dest_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_viewport(0.0, 0.0, dest_size.0 as f32, dest_size.1 as f32, 0.0, 1.0);
        pass.set_pipeline(self.pipeline(dest_format));
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

impl BrushPipelineEntry for WarpFieldResolve {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&crate::brush::pipeline::DynamicUniformRing> {
        None
    }
    fn rings(&self) -> Vec<&crate::brush::pipeline::DynamicUniformRing> {
        Vec::new()
    }
}
