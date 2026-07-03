//! Shared scaffold for parametric tone filters that realize as a 256×2 LUT.
//!
//! Curves and Levels are the same GPU pipeline: each bakes eight per-channel
//! transfer functions — in Krita's RGBA virtual-channel order
//!
//!   RGB (composite), Red, Green, Blue, Alpha, Hue, Saturation, Lightness
//!
//! — into one 256×2 RGBA8 LUT read by [`shaders/filters/curves.wgsl`], then runs
//! that one fragment shader. Only the *evaluator* differs: Curves reads a
//! natural-cubic spline, Levels a black/gamma/white/output transfer. Everything
//! shared lives here — the LUT layout, the composite-over-channel fold, the
//! HSV/Lab stage gate flags, the pipeline, and the `ensure`/`render` contract —
//! so each filter is a thin "give me eight per-channel evaluators" provider.

use crate::gpu::effect::EffectCache;
use crate::gpu::filter::FilterEffect;
use crate::gpu::params::ParamValue;

/// The LUT fragment-shader source shared by every LUT filter (Curves, Levels):
/// the shared colour-space lib prepended to `curves.wgsl`. Built at load time
/// rather than with a WGSL `#include` (the render shaders are plain
/// `include_str!` sources with no preprocessor).
pub fn lut_shader_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../../shaders/lib/colorspace.wgsl"),
        include_str!("../../shaders/filters/curves.wgsl"),
    )
}

/// Number of LUT entries per row (one per 8-bit input value).
pub const LUT_LEN: usize = 256;
/// The LUT is two rows: row 0 = per-component color curves, row 1 = HSL curves.
const LUT_ROWS: usize = 2;
/// Total LUT byte length (`256 × 2 × RGBA8`).
pub const LUT_BYTES: usize = LUT_LEN * LUT_ROWS * 4;

/// The eight virtual channels a LUT filter exposes, in Krita's RGBA order. The
/// discriminants match the positional order both filters declare their `PARAMS`
/// in, so a provider can index its per-channel state with `ch as usize`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Rgb = 0,
    Red = 1,
    Green = 2,
    Blue = 3,
    Alpha = 4,
    Hue = 5,
    Saturation = 6,
    Lightness = 7,
}

/// A baked LUT plus the stage-gating flags the shader reads.
pub struct Baked {
    /// 256×2 RGBA8, row-major. Row 0 = `[rgb∘red, rgb∘green, rgb∘blue, alpha]`;
    /// row 1 = `[hue, saturation, lightness, 0]`.
    pub lut: [u8; LUT_BYTES],
    /// Whether the Hue or Saturation channel is non-identity (gates the HSV pass).
    pub hsv_active: bool,
    /// Whether the Lightness channel is non-identity (gates the Lab pass).
    pub lightness_active: bool,
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Evaluate one channel across the 256 input steps into a u8 ramp.
fn ramp(eval: &impl Fn(Channel, f32) -> f32, ch: Channel) -> [u8; LUT_LEN] {
    let mut out = [0u8; LUT_LEN];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = to_u8(eval(ch, i as f32 / (LUT_LEN - 1) as f32));
    }
    out
}

/// True when a u8 ramp maps every input to itself (an identity transfer, up to
/// 8-bit rounding).
fn is_identity(ramp: &[u8; LUT_LEN]) -> bool {
    ramp.iter().enumerate().all(|(i, &v)| v as usize == i)
}

/// Bake eight per-channel evaluators into the shared LUT + gate flags.
///
/// `eval(channel, t)` maps an input `t ∈ [0, 1]` through `channel`'s transfer to
/// an output in `[0, 1]`. This function owns the channel semantics both filters
/// share, matching Krita: row 0 applies the per-channel curve first then the
/// composite "RGB" curve on top (`rgb ∘ channel`), and the composite is *not*
/// applied to alpha; row 1 holds the raw Hue/Saturation/Lightness remaps; and
/// each color-space stage is armed only when its channel is non-identity so an
/// all-identity layer is a byte-for-byte no-op.
pub fn bake_lut(eval: impl Fn(Channel, f32) -> f32) -> Baked {
    let hue = ramp(&eval, Channel::Hue);
    let sat = ramp(&eval, Channel::Saturation);
    let light = ramp(&eval, Channel::Lightness);

    let mut lut = [0u8; LUT_BYTES];
    let row1 = LUT_LEN * 4;
    for i in 0..LUT_LEN {
        let t = i as f32 / (LUT_LEN - 1) as f32;
        // Row 0: per-channel transfer first, then the composite RGB transfer on
        // top; the composite is not applied to alpha.
        lut[i * 4] = to_u8(eval(Channel::Rgb, eval(Channel::Red, t)));
        lut[i * 4 + 1] = to_u8(eval(Channel::Rgb, eval(Channel::Green, t)));
        lut[i * 4 + 2] = to_u8(eval(Channel::Rgb, eval(Channel::Blue, t)));
        lut[i * 4 + 3] = to_u8(eval(Channel::Alpha, t));
        // Row 1: the raw HSL remap ramps.
        lut[row1 + i * 4] = hue[i];
        lut[row1 + i * 4 + 1] = sat[i];
        lut[row1 + i * 4 + 2] = light[i];
        lut[row1 + i * 4 + 3] = 0;
    }

    Baked {
        lut,
        hsv_active: !is_identity(&hue) || !is_identity(&sat),
        lightness_active: !is_identity(&light),
    }
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

/// GPU realization shared by every LUT filter: one plain-RGBA pipeline that binds
/// `[src texture, lut texture, gate uniform]`, plus a per-filter `bake` function
/// that turns the layer's params into the baked LUT. The per-layer LUT texture
/// and uniform live in the compositor's [`EffectCache`], built by [`ensure`].
///
/// [`ensure`]: FilterEffect::ensure
pub struct LutFilter {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    bake: fn(&[ParamValue]) -> Baked,
}

impl LutFilter {
    /// Build the LUT pipeline from `shader_src` (its fragment entry point must be
    /// `fs_curves`) and bind the per-filter `bake` function.
    pub fn new(device: &wgpu::Device, shader_src: &str, bake: fn(&[ParamValue]) -> Baked) -> Self {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("filter-lut-bgl"),
            entries: &[
                load_tex_entry(0),
                load_tex_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("filter-lut-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("filter-lut-shader"),
            source: wgpu::ShaderSource::Wgsl(shader_src.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("filter-lut"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_curves"),
                targets: &[Some(wgpu::ColorTargetState {
                    // LUT filters only serve the filter-*layer* compose path,
                    // which is RGBA8-only (the destructive/R8 path excludes
                    // parametric filters at the engine layer).
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
        LutFilter {
            pipeline,
            bgl,
            bake,
        }
    }
}

impl std::fmt::Debug for LutFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LutFilter").finish_non_exhaustive()
    }
}

impl FilterEffect for LutFilter {
    fn ensure(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &[ParamValue],
        cache: &mut EffectCache,
    ) {
        // Allocate the LUT texture + gate uniform once; param edits reuse them.
        if cache.aux_textures.is_empty() {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("filter-lut-tex"),
                size: wgpu::Extent3d {
                    width: LUT_LEN as u32,
                    height: LUT_ROWS as u32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            cache.aux_textures.push(tex);
            cache.aux_views.push(view);
        }
        if cache.uniform_bufs.is_empty() {
            cache
                .uniform_bufs
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("filter-lut-flags"),
                    size: 16,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }

        let baked = (self.bake)(params);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &cache.aux_textures[0],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &baked.lut,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((LUT_LEN * 4) as u32),
                rows_per_image: Some(LUT_ROWS as u32),
            },
            wgpu::Extent3d {
                width: LUT_LEN as u32,
                height: LUT_ROWS as u32,
                depth_or_array_layers: 1,
            },
        );
        let flags = [
            baked.hsv_active as u32,
            baked.lightness_active as u32,
            0u32,
            0u32,
        ];
        queue.write_buffer(&cache.uniform_bufs[0], 0, bytemuck::cast_slice(&flags));
    }

    fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        _mask: Option<&wgpu::TextureView>,
        out: &wgpu::TextureView,
        _format: wgpu::TextureFormat,
        cache: &EffectCache,
    ) {
        // The compose path always runs `ensure` first (pre-compose sync phase),
        // so the LUT view + uniform are present. Guard defensively rather than
        // panic in the render loop.
        let (Some(lut_view), Some(uniform)) = (cache.aux_views.first(), cache.uniform_bufs.first())
        else {
            return;
        };
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("filter-lut-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(lut_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("filter-lut-pass"),
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
