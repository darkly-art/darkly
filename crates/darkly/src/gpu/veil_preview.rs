//! Offscreen veil preview renderer.
//!
//! Produces small, looping thumbnail frames of a single veil applied to a
//! bundled sample image — what the veil picker shows so users can see an
//! effect before adding it. Entirely self-contained: its own preview-sized
//! ping-pong textures and a veil instance built fresh from the registry, so
//! generating a preview never touches the live veil chain, the compositor's
//! surface, or the user's document.
//!
//! Mirrors the brush editor's offscreen approach
//! (`brush/preview_renderer.rs`): a reusable renderer that holds its scratch
//! target between calls and reallocates only on size change. The engine drives
//! per-frame async readback (`engine/veils.rs`) using the same
//! `ReadbackScheduler` pattern as export — no blocking GPU readbacks.
//!
//! Data flow per frame: the sample image lives in `ping_pong[0]`; the veil
//! reads it (`src_idx = 0`) and writes its output to `ping_pong[1]`, which is
//! read back. Animated veils advance via `update_time` between frames; static
//! veils render a single frame.

use super::effect::EffectCache;
use super::params::ParamValue;
use super::veil::{Veil, VeilRegistry};

/// Preview thumbnail width in pixels (16:9 with [`PREVIEW_HEIGHT`]).
pub const PREVIEW_WIDTH: u32 = 256;
/// Preview thumbnail height in pixels.
pub const PREVIEW_HEIGHT: u32 = 144;
/// Frames captured for an animated veil (≈2s at [`PREVIEW_FPS`]). Static
/// veils render a single frame.
pub const ANIMATED_FRAMES: u32 = 48;
/// Capture / playback rate, in frames per second.
pub const PREVIEW_FPS: u32 = 24;
/// Per-frame delta time (seconds) fed to animated veils' `update_time`.
const PREVIEW_DT: f32 = 1.0 / PREVIEW_FPS as f32;

/// Sample artwork the veils are applied to. Reused from the "Fill Background"
/// command's bundled image so previews show the effect on real photographic
/// content.
const SAMPLE_JPEG: &[u8] = include_bytes!("../../resources/backgrounds/quiet-night.jpg");

/// Ping-pong textures sized to the preview thumbnail. `pingpong[0]` holds the
/// sample image (veil input); `pingpong[1]` receives each veil output frame.
struct PreviewTextures {
    pingpong: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
}

/// Renders veil preview frames into an offscreen RGBA texture. One instance is
/// reusable across veils and renders; it lazily allocates its target and sample
/// upload on first use and keeps them between calls.
pub struct VeilPreviewRenderer {
    textures: Option<PreviewTextures>,
    sampler: Option<wgpu::Sampler>,
}

impl VeilPreviewRenderer {
    pub fn new() -> Self {
        Self {
            textures: None,
            sampler: None,
        }
    }

    /// Build a veil instance + its GPU cache and load the sample image into the
    /// input texture. Returns the veil (its `needs_animation()` decides the
    /// frame count) and its cache; the caller then encodes frames via
    /// [`encode_frame`](Self::encode_frame) and reads back
    /// [`output_texture`](Self::output_texture).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        registry: &mut VeilRegistry,
        type_id: &str,
        params: &[ParamValue],
        format: wgpu::TextureFormat,
    ) -> (Box<dyn Veil>, EffectCache) {
        self.ensure_resources(device, queue, format);
        let textures = self.textures.as_ref().unwrap();
        let sampler = self.sampler.as_ref().unwrap();

        let veil = registry.create_veil(type_id, params, device, format);
        let cache = veil.create_cache(
            device,
            queue,
            &textures.views,
            sampler,
            PREVIEW_WIDTH,
            PREVIEW_HEIGHT,
        );
        (veil, cache)
    }

    /// Per-frame delta time animated veils should be stepped by between
    /// [`encode_frame`](Self::encode_frame) calls.
    pub fn frame_dt(&self) -> f32 {
        PREVIEW_DT
    }

    /// Encode the veil's render passes for one frame: read the sample from
    /// `ping_pong[0]`, write the result to `ping_pong[1]`.
    pub fn encode_frame(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        veil: &dyn Veil,
        cache: &EffectCache,
    ) {
        let textures = self.textures.as_ref().unwrap();
        veil.encode(encoder, cache, 0, &textures.views[1]);
    }

    /// The texture holding the most recently encoded frame — readback source.
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.textures.as_ref().unwrap().pingpong[1]
    }

    /// Allocate the sampler + ping-pong textures and upload the (decoded,
    /// scaled) sample image into the input texture. Idempotent — does nothing
    /// once allocated, since nothing else writes `ping_pong[0]`.
    fn ensure_resources(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) {
        if self.sampler.is_none() {
            self.sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("veil-preview-sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }));
        }

        if self.textures.is_some() {
            return;
        }

        let make_texture = |label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: PREVIEW_WIDTH,
                    height: PREVIEW_HEIGHT,
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
        let pingpong = [
            make_texture("veil-preview-input"),
            make_texture("veil-preview-output"),
        ];
        let views = [
            pingpong[0].create_view(&wgpu::TextureViewDescriptor::default()),
            pingpong[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];

        // Upload the sample image into ping_pong[0]. Veils only ever read this
        // texture (writing to ping_pong[1]), so a single upload suffices.
        let sample = decode_sample(PREVIEW_WIDTH, PREVIEW_HEIGHT);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &pingpong[0],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &sample,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(PREVIEW_WIDTH * 4),
                rows_per_image: Some(PREVIEW_HEIGHT),
            },
            wgpu::Extent3d {
                width: PREVIEW_WIDTH,
                height: PREVIEW_HEIGHT,
                depth_or_array_layers: 1,
            },
        );

        self.textures = Some(PreviewTextures { pingpong, views });
    }
}

impl Default for VeilPreviewRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode the bundled sample JPEG, center-crop it to the preview aspect ratio,
/// and scale it to `width × height`, returning tightly-packed RGBA8 bytes.
fn decode_sample(width: u32, height: u32) -> Vec<u8> {
    let img = image::load_from_memory(SAMPLE_JPEG).expect("decode bundled veil-preview sample");
    let (iw, ih) = (img.width(), img.height());

    // Center-crop to the target aspect ratio so the resize doesn't distort.
    let target_ar = width as f32 / height as f32;
    let src_ar = iw as f32 / ih as f32;
    let (cw, ch) = if src_ar > target_ar {
        (((ih as f32) * target_ar).round() as u32, ih)
    } else {
        (iw, ((iw as f32) / target_ar).round() as u32)
    };
    let cx = (iw - cw.min(iw)) / 2;
    let cy = (ih - ch.min(ih)) / 2;
    let cropped = img.crop_imm(cx, cy, cw.min(iw), ch.min(ih));
    let scaled = cropped.resize_exact(width, height, image::imageops::FilterType::Triangle);
    scaled.to_rgba8().into_raw()
}
