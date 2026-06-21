//! Offscreen veil preview renderer.
//!
//! Produces small, looping thumbnail frames of a single veil applied to the
//! user's **current canvas** — so the picker shows what each effect would do to
//! their actual art, not a stock sample. Entirely self-contained: its own
//! preview-sized ping-pong textures and a veil instance built fresh from the
//! registry, so generating a preview never touches the live veil chain, the
//! compositor's surface, or the document.
//!
//! Mirrors the brush editor's offscreen approach
//! (`brush/preview_renderer.rs`): a reusable renderer that holds its scratch
//! target between calls and reallocates only on size change. The engine drives
//! per-frame async readback (`engine/veils.rs`) using the same
//! `ReadbackScheduler` pattern as export — no blocking GPU readbacks.
//!
//! Data flow per frame: the (downscaled) composite lives in `ping_pong[0]`; the
//! veil reads it (`src_idx = 0`) and writes its output to `ping_pong[1]`, which
//! is read back. Animated veils advance via `update_time` between frames; static
//! veils render a single frame. Previews are **not cached** — each time the
//! picker opens, frames are regenerated against the current canvas.

use super::effect::{self, EffectPipeline};
use super::params::ParamValue;
use super::preview::{fit_preview_dims, PREVIEW_DT};
use super::veil::{Veil, VeilRegistry};

/// Ping-pong textures sized to the preview thumbnail. `pingpong[0]` holds the
/// downscaled composite (veil input); `pingpong[1]` receives each veil output.
struct PreviewTextures {
    width: u32,
    height: u32,
    pingpong: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
}

/// Renders veil preview frames into an offscreen RGBA texture. One instance is
/// reusable across veils and renders; it lazily allocates its sampler, downscale
/// pipeline, and target, reallocating the target only when the preview
/// dimensions change.
pub struct VeilPreviewRenderer {
    textures: Option<PreviewTextures>,
    sampler: Option<wgpu::Sampler>,
    /// Soft multi-tap downscale used to copy the (often much larger) composite
    /// into the preview-sized input texture without hard aliasing.
    downscale: Option<EffectPipeline>,
}

impl VeilPreviewRenderer {
    pub fn new() -> Self {
        Self {
            textures: None,
            sampler: None,
            downscale: None,
        }
    }

    /// Downscale `source` (the current composite) into the preview input
    /// texture, (re)allocating the target if the preview dimensions changed.
    /// Must be called before [`build_veil`](Self::build_veil) /
    /// [`encode_frame`](Self::encode_frame) each generation, since the source
    /// content (and possibly the canvas size) may have changed.
    pub fn load_source(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_view: &wgpu::TextureView,
        source_width: u32,
        source_height: u32,
        format: wgpu::TextureFormat,
    ) {
        self.ensure_sampler(device);
        self.ensure_downscale(device, format);

        let (pw, ph) = fit_preview_dims(source_width, source_height);
        let realloc = match &self.textures {
            Some(t) => t.width != pw || t.height != ph,
            None => true,
        };
        if realloc {
            self.textures = Some(make_textures(device, pw, ph, format));
        }

        let textures = self.textures.as_ref().unwrap();
        let downscale = self.downscale.as_ref().unwrap();
        let source_bg = effect::create_blit_bind_group(
            device,
            &downscale.bind_group_layout,
            source_view,
            self.sampler.as_ref().unwrap(),
            "veil-preview-source-bg",
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("veil-preview-load-source"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("veil-preview-downscale"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &textures.views[0],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(&downscale.pipeline);
            rpass.set_bind_group(0, &source_bg, &[]);
            rpass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
    }

    /// Build a veil instance + its GPU cache over the loaded composite. Returns
    /// the veil (its `needs_animation()` decides the frame count) and its cache;
    /// the caller then encodes frames via [`encode_frame`](Self::encode_frame)
    /// and reads back [`output_texture`](Self::output_texture).
    ///
    /// [`load_source`](Self::load_source) must have run first.
    pub fn build_veil(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        registry: &mut VeilRegistry,
        type_id: &str,
        params: &[ParamValue],
        format: wgpu::TextureFormat,
    ) -> (Box<dyn Veil>, super::effect::EffectCache) {
        let textures = self
            .textures
            .as_ref()
            .expect("load_source must run before build_veil");
        let veil = registry.create_veil(type_id, params, device, format);
        let cache = veil.create_cache(
            device,
            queue,
            &textures.views,
            self.sampler.as_ref().unwrap(),
            textures.width,
            textures.height,
        );
        (veil, cache)
    }

    /// The preview dimensions (width, height) of the currently loaded target,
    /// or `(0, 0)` if nothing is loaded.
    pub fn preview_size(&self) -> (u32, u32) {
        self.textures
            .as_ref()
            .map(|t| (t.width, t.height))
            .unwrap_or((0, 0))
    }

    /// Per-frame delta time animated veils should be stepped by between
    /// [`encode_frame`](Self::encode_frame) calls.
    pub fn frame_dt(&self) -> f32 {
        PREVIEW_DT
    }

    /// Encode the veil's render passes for one frame: read the composite from
    /// `ping_pong[0]`, write the result to `ping_pong[1]`.
    pub fn encode_frame(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        veil: &dyn Veil,
        cache: &super::effect::EffectCache,
    ) {
        let textures = self.textures.as_ref().unwrap();
        veil.encode(encoder, cache, 0, &textures.views[1]);
    }

    /// The texture holding the most recently encoded frame — readback source.
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.textures.as_ref().unwrap().pingpong[1]
    }

    fn ensure_sampler(&mut self, device: &wgpu::Device) {
        if self.sampler.is_none() {
            self.sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("veil-preview-sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }));
        }
    }

    fn ensure_downscale(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if self.downscale.is_none() {
            self.downscale = Some(effect::create_downscale_pipeline(
                device,
                format,
                "veil-preview-downscale",
            ));
        }
    }
}

impl Default for VeilPreviewRenderer {
    fn default() -> Self {
        Self::new()
    }
}

fn make_textures(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> PreviewTextures {
    let make = |label: &str| {
        device.create_texture(&wgpu::TextureDescriptor {
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
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    };
    let pingpong = [make("veil-preview-input"), make("veil-preview-output")];
    let views = [
        pingpong[0].create_view(&wgpu::TextureViewDescriptor::default()),
        pingpong[1].create_view(&wgpu::TextureViewDescriptor::default()),
    ];
    PreviewTextures {
        width,
        height,
        pingpong,
        views,
    }
}
