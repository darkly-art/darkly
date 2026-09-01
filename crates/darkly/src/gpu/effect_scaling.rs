//! Running an effect at less than its target's resolution.
//!
//! Spatial effects cost per output texel, and several of them cost enough that
//! full resolution is not worth the difference — so an effect can be rendered
//! into a smaller pair and the result scaled back up. That wrapping is the same
//! wherever the effect runs, which is why it lives here rather than inside any
//! one caller: downscale the source, run the effect at the reduced size,
//! upscale the result into the destination.
//!
//! Two scales feed it, because the two spaces are different things. The
//! screen-space chain treats the viewport as a viewing surface and defaults
//! below 1.0; canvas space is document content and defaults to 1.0, since
//! shipping a layer's pixels through a reduced-resolution round trip would bake
//! the loss into what the user exports.
//!
//! Effects are resolution-agnostic and never learn any of this: an effect sees
//! the pair it was handed and its size, and nothing about why.

use crate::gpu::effect::{self, Effect, EffectCache, EffectPipeline};

/// Below this the scale reads as 1.0 and the whole reduced-resolution path is
/// skipped, so a config value a hair under 1.0 costs nothing.
const FULL_SCALE_EPSILON: f32 = 1.0e-3;

/// Floor on the effective scale. Past this an effect has too few texels to
/// produce anything recognizable, and a zero would be a zero-sized texture.
const MIN_SCALE: f32 = 0.05;

/// Fraction of native viewport resolution to render screen-space effects at.
pub fn screen_scale() -> f32 {
    (crate::config::get_f64("rendering.screen_effect_scale") as f32).clamp(MIN_SCALE, 1.0)
}

/// Fraction of canvas resolution to render canvas-space effects at. Defaults to
/// 1.0 — this is document content, and the result is what gets exported.
pub fn canvas_scale() -> f32 {
    (crate::config::get_f64("rendering.canvas_effect_scale") as f32).clamp(MIN_SCALE, 1.0)
}

/// The two shared pipelines the reduced-resolution path needs, built once by
/// whoever owns a set of scaled effects.
///
/// The downscale is a multi-tap soft filter rather than a blit: single-tap
/// bilinear is a fixed 2×2 box regardless of ratio, so it aliases hard below
/// about 0.7 on any effect sensitive to small input differences (Painting
/// especially). Upscaling has no such problem — each output texel reads a
/// sub-texel position — so that direction is a plain blit.
pub struct ScalingPipelines {
    downscale: EffectPipeline,
    upscale: EffectPipeline,
}

impl ScalingPipelines {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, label: &str) -> Self {
        ScalingPipelines {
            downscale: effect::create_downscale_pipeline(
                device,
                format,
                &format!("{label}-downscale"),
            ),
            upscale: effect::create_blit_pipeline(device, format, &format!("{label}-upscale")),
        }
    }

    /// The two pipelines share a bind-group layout, so either one can build the
    /// bind groups for both directions.
    fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.downscale.bind_group_layout
    }
}

/// One effect's reduced-resolution scaffolding, or the absence of it.
///
/// [`Full`](ScaledEffect::Full) is not a degenerate case of the scaled one: at
/// full scale the effect reads and writes the caller's own pair directly, with
/// no intermediate textures and no extra passes at all. Making that a variant
/// rather than a scale of 1.0 is what keeps the common path free.
pub enum ScaledEffect {
    /// The effect runs directly on the caller's ping-pong pair.
    Full,
    /// The effect runs on its own smaller pair, wrapped in downscale/upscale.
    /// Boxed because it carries two textures, two views and three bind groups
    /// while `Full` carries nothing, and every instance would otherwise pay the
    /// larger variant's size for the common case.
    Reduced(Box<Reduced>),
}

/// Reduced-resolution textures and the bind groups that move data in and out.
pub struct Reduced {
    /// Kept alive so the GPU textures aren't dropped.
    _textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    /// `[i]` reads native `ping_pong[i]` and draws into `views[0]`.
    downscale_bgs: [wgpu::BindGroup; 2],
    /// Reads `views[1]` — the effect's output — and draws into the destination.
    upscale_bg: wgpu::BindGroup,
}

impl ScaledEffect {
    /// Prepare `effect` against `native_views` at `scale`, returning the
    /// scaffolding and the cache the effect built for whichever pair it will
    /// actually read.
    ///
    /// The effective scale is `scale * effect.perf_scale_factor()`, so a global
    /// setting and an effect's own declared cost compose rather than one
    /// overriding the other.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        effect: &mut dyn Effect,
        native_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        pipelines: &ScalingPipelines,
        format: wgpu::TextureFormat,
        native_width: u32,
        native_height: u32,
        scale: f32,
    ) -> (Self, EffectCache) {
        let effective = (scale * effect.perf_scale_factor()).clamp(MIN_SCALE, 1.0);
        if effective >= 1.0 - FULL_SCALE_EPSILON {
            let cache = effect.create_cache(
                device,
                queue,
                native_views,
                sampler,
                native_width,
                native_height,
            );
            return (ScaledEffect::Full, cache);
        }

        let rw = ((native_width as f32 * effective).round() as u32).max(1);
        let rh = ((native_height as f32 * effective).round() as u32).max(1);
        let reduced = Reduced::new(device, sampler, pipelines, format, rw, rh, native_views);
        let cache = effect.create_cache(device, queue, &reduced.views, sampler, rw, rh);
        (ScaledEffect::Reduced(Box::new(reduced)), cache)
    }

    /// Encode the effect, reading `native_views[src_idx]` and writing
    /// `dst_view`. At full scale that is the effect's own pass and nothing
    /// else; reduced, it is downscale → effect → upscale.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        effect: &dyn Effect,
        cache: &EffectCache,
        pipelines: &ScalingPipelines,
        src_idx: usize,
        dst_view: &wgpu::TextureView,
    ) {
        match self {
            ScaledEffect::Full => effect.encode(encoder, cache, src_idx, dst_view),
            ScaledEffect::Reduced(reduced) => {
                blit_pass(
                    encoder,
                    &pipelines.downscale.pipeline,
                    &reduced.downscale_bgs[src_idx],
                    &reduced.views[0],
                    "effect-downscale",
                );
                // The effect always reads slot 0 of its own reduced pair, which
                // is where the downscale just landed.
                effect.encode(encoder, cache, 0, &reduced.views[1]);
                blit_pass(
                    encoder,
                    &pipelines.upscale.pipeline,
                    &reduced.upscale_bg,
                    dst_view,
                    "effect-upscale",
                );
            }
        }
    }
}

impl Reduced {
    fn new(
        device: &wgpu::Device,
        sampler: &wgpu::Sampler,
        pipelines: &ScalingPipelines,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        native_views: &[wgpu::TextureView; 2],
    ) -> Self {
        let make_tex = |label: &str| -> (wgpu::Texture, wgpu::TextureView) {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (tex, view)
        };

        let (t0, v0) = make_tex("effect-scaled-0");
        let (t1, v1) = make_tex("effect-scaled-1");
        let layout = pipelines.layout();

        let downscale_bgs: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            effect::create_blit_bind_group(
                device,
                layout,
                &native_views[i],
                sampler,
                &format!("effect-downscale-{i}"),
            )
        });
        let upscale_bg =
            effect::create_blit_bind_group(device, layout, &v1, sampler, "effect-upscale");

        Reduced {
            _textures: [t0, t1],
            views: [v0, v1],
            downscale_bgs,
            upscale_bg,
        }
    }
}

/// Execute a fullscreen blit render pass.
pub fn blit_pass(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    dst_view: &wgpu::TextureView,
    label: &'static str,
) {
    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: dst_view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    rpass.set_pipeline(pipeline);
    rpass.set_bind_group(0, bind_group, &[]);
    rpass.draw(0..3, 0..1);
}
