//! Shared scaffold for parametric tone filters that realize as a 256×2 LUT.
//!
//! Curves and Levels are the same GPU pipeline: each bakes eight per-channel
//! transfer functions, in Krita's RGBA virtual-channel order:
//!
//!   RGB (composite), Red, Green, Blue, Alpha, Hue, Saturation, Lightness
//!
//! into one 256×2 RGBA8 LUT read by [`shaders/filters/curves.wgsl`], then runs
//! that one fragment shader. Only the *evaluator* differs: Curves reads a
//! natural-cubic spline, Levels a black/gamma/white/output transfer. Everything
//! shared lives here: the LUT layout, the composite-over-channel fold, the
//! HSV/Lab stage gate flags, the pipeline, and the `ensure`/`render` contract,
//! so each filter is a thin "give me eight per-channel evaluators" provider.

use crate::gpu::effect::EffectCache;
use crate::gpu::param_filter::{ParamFilter, SrcSampling};
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

/// Allocate (once) and refresh the LUT texture + gate uniform for a LUT filter:
/// the [`ParamFilter`] `prepare` half for the aux-carrying Curves/Levels family.
/// `bake` turns the layer's params into the baked LUT + stage-gate flags.
fn lut_prepare(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bake: fn(&[ParamValue]) -> Baked,
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

    let baked = bake(params);
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

/// Build the aux-carrying [`ParamFilter`] specialization every LUT filter
/// (Curves, Levels) shares: the `[src, lut, uniform]` bind-group shape and the
/// `fs_curves` / `fs_curves_masked` entry points, with a `prepare` that bakes
/// `bake(params)` into the LUT texture + gate uniform on each param change.
/// `shader_src`'s fragment entries must be `fs_curves` (plain) and
/// `fs_curves_masked` (selection-clipped).
pub fn lut_param_filter(
    device: &wgpu::Device,
    shader_src: &str,
    bake: fn(&[ParamValue]) -> Baked,
) -> ParamFilter {
    ParamFilter::new(
        device,
        "filter-lut",
        shader_src,
        "fs_curves",
        "fs_curves_masked",
        true, // aux-carrying (the 256×2 LUT texture at binding 1)
        SrcSampling::Load,
        move |device, queue, params, cache| lut_prepare(device, queue, bake, params, cache),
    )
}
