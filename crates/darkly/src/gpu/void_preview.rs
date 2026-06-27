//! Offscreen void preview renderer.
//!
//! Produces small, looping thumbnail frames of a single void for the "Add Void"
//! picker. Unlike a veil (which post-processes the user's current canvas through
//! a ping-pong texture pair), a void *generates* its content from scratch — so
//! there is no source to downscale and no pair to ping-pong between. The void
//! renders straight into one preview-sized destination texture, which is read
//! back. A void instance is built fresh from the registry each generation, so
//! the preview never touches the live layer stack, the compositor's surface, or
//! the document.
//!
//! The shared sizing primitives ([`fit_preview_dims`], the frame-count / fps
//! constants) live in [`super::preview`]; the engine drives per-frame async
//! readback (`engine/voids.rs`) using the same `ReadbackScheduler` pattern as
//! the veil path — no blocking GPU readbacks.

use super::params::ParamValue;
use super::preview::fit_preview_dims;
use super::void::{Void, VoidRegistry};

/// Single preview-sized destination texture the void renders into.
struct PreviewTexture {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// Renders void preview frames into an offscreen RGBA texture. One instance is
/// reusable across voids and renders; it lazily allocates its sampler and
/// output target, reallocating the target only when the preview dimensions
/// change.
pub struct VoidPreviewRenderer {
    target: Option<PreviewTexture>,
    sampler: Option<wgpu::Sampler>,
}

impl VoidPreviewRenderer {
    pub fn new() -> Self {
        Self {
            target: None,
            sampler: None,
        }
    }

    /// Build a void instance + its GPU cache targeting the preview output
    /// texture, (re)allocating that texture if the aspect-fit dimensions of
    /// `canvas_w × canvas_h` changed. Returns the void (its `needs_animation()`
    /// decides the frame count) and its cache; the caller then encodes frames
    /// via [`encode_frame`](Self::encode_frame) and reads back
    /// [`output_texture`](Self::output_texture).
    #[allow(clippy::too_many_arguments)]
    pub fn build_void(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        registry: &mut VoidRegistry,
        type_id: &str,
        params: &[ParamValue],
        canvas_w: u32,
        canvas_h: u32,
        format: wgpu::TextureFormat,
    ) -> (Box<dyn Void>, super::effect::EffectCache) {
        self.ensure_sampler(device);

        let (pw, ph) = fit_preview_dims(canvas_w, canvas_h);
        let realloc = match &self.target {
            Some(t) => t.width != pw || t.height != ph,
            None => true,
        };
        if realloc {
            self.target = Some(make_texture(device, pw, ph, format));
        }

        let target = self.target.as_ref().unwrap();
        let void = registry.create_void(type_id, params, device, format);
        let cache = void.create_cache(
            device,
            queue,
            &target.view,
            self.sampler.as_ref().unwrap(),
            target.width,
            target.height,
        );
        (void, cache)
    }

    /// The preview dimensions (width, height) of the currently built target, or
    /// `(0, 0)` if nothing is built.
    pub fn preview_size(&self) -> (u32, u32) {
        self.target
            .as_ref()
            .map(|t| (t.width, t.height))
            .unwrap_or((0, 0))
    }

    /// Encode the void's render passes for one frame into the output texture.
    pub fn encode_frame(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        void: &dyn Void,
        cache: &super::effect::EffectCache,
    ) {
        let target = self.target.as_ref().unwrap();
        void.encode(encoder, cache, &target.view);
    }

    /// The texture holding the most recently encoded frame — readback source.
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.target.as_ref().unwrap().texture
    }

    fn ensure_sampler(&mut self, device: &wgpu::Device) {
        if self.sampler.is_none() {
            self.sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("void-preview-sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }));
        }
    }
}

impl Default for VoidPreviewRenderer {
    fn default() -> Self {
        Self::new()
    }
}

fn make_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> PreviewTexture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("void-preview-output"),
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
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    PreviewTexture {
        width,
        height,
        texture,
        view,
    }
}
