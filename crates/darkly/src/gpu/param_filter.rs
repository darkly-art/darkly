//! Shared single-pass RGBA8 substrate for *parametric* filters (Curves, Levels,
//! HSV). One fullscreen fragment pass reads the source texel, transforms it from
//! a params-derived [`EffectCache`], and writes the result, with a masked
//! sibling entry point that keeps the original texel wherever an R8 selection
//! mask is unselected (`select(orig, filtered, selected)`, exactly like invert's
//! [`fs_invert_masked`](../../shaders/filters/invert.wgsl)).
//!
//! The substrate is parameterized on **whether the shader binds an auxiliary
//! texture**: more than just "optional aux", it selects between two distinct
//! bind-group shapes the shaders declare their binding numbers to match:
//!
//! - **aux present** (Curves/Levels, a 256×2 LUT): `[src(0), aux(1), uniform(2)]`,
//!   masked adds `mask(3)`.
//! - **aux absent** (HSV, packed scalars only): `[src(0), uniform(1)]`, masked
//!   adds `mask(2)`.
//!
//! The LUT family is the aux-carrying specialization built by
//! [`lut_param_filter`](super::lut_filter::lut_param_filter); HSV builds a no-aux
//! `ParamFilter` directly. Parameter-free filters (invert) do not use this; they
//! ride [`MaskedFilterPipeline`](super::effect::MaskedFilterPipeline), which also
//! serves R8 masks; parametric color filters are RGBA8-only.

use crate::gpu::effect::EffectCache;
use crate::gpu::filter::FilterEffect;
use crate::gpu::params::ParamValue;

/// Fills an [`EffectCache`] from a filter's params: the per-filter half of the
/// substrate, run from [`FilterEffect::ensure`] (never in the render loop).
/// Curves/Levels bake a LUT texture + gate uniform; HSV packs a single uniform.
type Prepare =
    Box<dyn Fn(&wgpu::Device, &wgpu::Queue, &[ParamValue], &mut EffectCache) + Send + Sync>;

/// How the shared substrate reads its `src` texel: `Load` is a coordinate-exact
/// `textureLoad` (no sampler); `Bilinear` binds `src` filterable plus a linear
/// `Filtering` sampler, so a filter can read fractional offsets, the capability
/// chromatic aberration's ghost sampling and blur need. The bind-group order is
/// `[src(0), sampler(1)?, aux(2)?, uniform, mask?]`, each optional binding
/// shifting the following numbers up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SrcSampling {
    Load,
    Bilinear,
}

/// A `textureLoad` source binding (no sampler / hardware filtering).
fn load_tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// A filterable `textureSample` source binding (paired with a `Filtering`
/// sampler) for the `Bilinear` source mode.
fn filterable_tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// The shared render half of a parametric filter: a plain and a masked RGBA8
/// pipeline over one shader, plus the two bind-group layouts. The per-filter
/// `prepare` builds the [`EffectCache`] the pipelines read.
pub struct ParamFilter {
    plain: wgpu::RenderPipeline,
    masked: wgpu::RenderPipeline,
    /// `[src(, aux), uniform]`, read by the plain entry point.
    plain_bgl: wgpu::BindGroupLayout,
    /// `plain_bgl` + a trailing mask texture, read by the masked entry point.
    masked_bgl: wgpu::BindGroupLayout,
    /// Whether the shader binds an aux texture between `src` and the uniform,
    /// which shifts every following binding number by one.
    has_aux: bool,
    /// How `src` is read; `Bilinear` also owns a `Filtering` sampler bound at
    /// binding 1 (created once here; the compositor's reusable sampler lives on
    /// `VeilChain`, the wrong layer to reach from a filter).
    sampling: SrcSampling,
    sampler: Option<wgpu::Sampler>,
    prepare: Prepare,
}

impl std::fmt::Debug for ParamFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParamFilter")
            .field("has_aux", &self.has_aux)
            .field("sampling", &self.sampling)
            .finish_non_exhaustive()
    }
}

impl ParamFilter {
    /// Build the plain + masked RGBA8 pipelines from one shader source (its
    /// vertex entry point must be `vs_main`). `plain_entry` reads `[src(, aux),
    /// uniform]`; `masked_entry` reads the same plus a trailing mask texture and
    /// blends to the original where the mask is unselected. `has_aux` selects the
    /// bind-group shape; `prepare` fills the per-instance [`EffectCache`].
    pub fn new(
        device: &wgpu::Device,
        label: &str,
        shader_src: &str,
        plain_entry: &str,
        masked_entry: &str,
        has_aux: bool,
        sampling: SrcSampling,
        prepare: impl Fn(&wgpu::Device, &wgpu::Queue, &[ParamValue], &mut EffectCache)
            + Send
            + Sync
            + 'static,
    ) -> ParamFilter {
        // Binding layout: `src` is always 0; then, in order, an optional sampler
        // (Bilinear source mode), an optional aux texture, the uniform, and
        // (in the masked variant) the mask. Each optional binding shifts the
        // following numbers up by one.
        let bilinear = sampling == SrcSampling::Bilinear;
        let mut next = 1u32;
        let sampler_binding = bilinear.then(|| {
            let b = next;
            next += 1;
            b
        });
        let aux_binding = has_aux.then(|| {
            let b = next;
            next += 1;
            b
        });
        let uniform_binding = next;
        next += 1;
        let mask_binding = next;

        let mut plain_entries = vec![if bilinear {
            filterable_tex_entry(0)
        } else {
            load_tex_entry(0)
        }];
        if let Some(sb) = sampler_binding {
            plain_entries.push(sampler_entry(sb));
        }
        if let Some(ab) = aux_binding {
            plain_entries.push(load_tex_entry(ab));
        }
        plain_entries.push(uniform_entry(uniform_binding));
        let mut masked_entries = plain_entries.clone();
        masked_entries.push(load_tex_entry(mask_binding));

        // Linear, ClampToEdge sampler for the Bilinear mode. The lib's sample
        // helper returns transparent for out-of-bounds UV, so the address mode
        // only matters at the sub-texel edge, where ClampToEdge is correct.
        let sampler = bilinear.then(|| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(&format!("{label}-sampler")),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        });

        let plain_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{label}-plain-bgl")),
            entries: &plain_entries,
        });
        let masked_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{label}-masked-bgl")),
            entries: &masked_entries,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&format!("{label}-shader")),
            source: wgpu::ShaderSource::Wgsl(shader_src.into()),
        });

        let make = |bgl: &wgpu::BindGroupLayout, entry: &str, lbl: &str| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(lbl),
                bind_group_layouts: &[Some(bgl)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(lbl),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    // Parametric color filters serve RGBA8 only (the filter-layer
                    // compose path and the destructive apply over a raster layer);
                    // R8 masks stay on the parameter-free `MaskedFilterPipeline`.
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
            })
        };

        ParamFilter {
            plain: make(&plain_bgl, plain_entry, &format!("{label}-plain")),
            masked: make(&masked_bgl, masked_entry, &format!("{label}-masked")),
            plain_bgl,
            masked_bgl,
            has_aux,
            sampling,
            sampler,
            prepare: Box::new(prepare),
        }
    }
}

impl FilterEffect for ParamFilter {
    fn ensure(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &[ParamValue],
        cache: &mut EffectCache,
    ) {
        (self.prepare)(device, queue, params, cache);
    }

    fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        mask: Option<&wgpu::TextureView>,
        out: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        cache: &EffectCache,
    ) {
        // Parametric color filters are RGBA8-only; an R8 target would fail
        // pipeline validation, so skip rather than crash (the frontend never
        // offers these on a mask node).
        if format != wgpu::TextureFormat::Rgba8Unorm {
            return;
        }
        // `ensure` runs in the pre-compose sync phase, so the uniform (and aux,
        // when carried) are present. Guard defensively rather than panic in the
        // render loop.
        let Some(uniform) = cache.uniform_bufs.first() else {
            return;
        };
        let aux = if self.has_aux {
            match cache.aux_views.first() {
                Some(v) => Some(v),
                None => return,
            }
        } else {
            None
        };

        let (pipeline, bgl) = match mask {
            Some(_) => (&self.masked, &self.masked_bgl),
            None => (&self.plain, &self.plain_bgl),
        };

        // `ensure` runs before render; the Bilinear sampler is built in `new`.
        // Guard rather than panic if the mode/sampler ever disagree.
        if self.sampling == SrcSampling::Bilinear && self.sampler.is_none() {
            return;
        }

        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(src),
        }];
        let mut next = 1u32;
        if let Some(s) = &self.sampler {
            entries.push(wgpu::BindGroupEntry {
                binding: next,
                resource: wgpu::BindingResource::Sampler(s),
            });
            next += 1;
        }
        if let Some(a) = aux {
            entries.push(wgpu::BindGroupEntry {
                binding: next,
                resource: wgpu::BindingResource::TextureView(a),
            });
            next += 1;
        }
        entries.push(wgpu::BindGroupEntry {
            binding: next,
            resource: uniform.as_entire_binding(),
        });
        next += 1;
        if let Some(mv) = mask {
            entries.push(wgpu::BindGroupEntry {
                binding: next,
                resource: wgpu::BindingResource::TextureView(mv),
            });
        }

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("param-filter-bg"),
            layout: bgl,
            entries: &entries,
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("param-filter-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: out,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
