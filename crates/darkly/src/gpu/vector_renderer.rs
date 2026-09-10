//! GPU realization backend for vector-object layers.
//!
//! Owns a single [`vello::Renderer`] shared by every vector layer on this
//! device. The engine builds a `vello::Scene` from the document's vector
//! objects (text shaped by parley, paths from kurbo) and hands it to the
//! compositor, which calls [`VectorRenderer::render`] to rasterize it into the
//! layer's `Rgba8Unorm` + `STORAGE_BINDING` texture. The existing blend
//! pipeline then composites that texture like any other layer.
//!
//! The renderer is a swappable backend behind the kurbo/peniko data model:
//! a future move to `vello_cpu`/`vello_hybrid` is a contained change here,
//! not a document-model rewrite (see the text-tool plan, §0).

use std::num::NonZeroUsize;

use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};

/// Raise `base` device limits to what Vello's compute pipelines require.
///
/// Vello renders via compute shaders that bind more storage buffers/textures
/// than the conservative WebGL2/downlevel floors Darkly otherwise targets. The
/// bumped values sit within the WebGPU spec's *guaranteed minimums*, so any
/// real WebGPU device satisfies them; the only cost is dropping pre-WebGPU
/// (WebGL2) fallback, an accepted trade for GPU-rendered text (text-tool plan
/// §0, "Chromium-first WebGPU / STORAGE_BINDING surface"). Every device that
/// may host a vector layer (production wasm + native test devices) must be
/// created through this so the lazy [`VectorRenderer::new`] can't fail.
pub fn required_limits(base: wgpu::Limits) -> wgpu::Limits {
    wgpu::Limits {
        max_storage_buffers_per_shader_stage: base.max_storage_buffers_per_shader_stage.max(8),
        max_storage_textures_per_shader_stage: base.max_storage_textures_per_shader_stage.max(4),
        ..base
    }
}

/// One Vello renderer per device, lazily created when the first vector layer is
/// realized so projects with no vector layers never pay its shader-compile cost.
pub struct VectorRenderer {
    renderer: Renderer,
}

impl VectorRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let renderer = Renderer::new(
            device,
            RendererOptions {
                use_cpu: false,
                // Area AA is the recommended default and keeps startup cheap:
                // no MSAA pipeline permutations to compile.
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: NonZeroUsize::new(1),
                pipeline_cache: None,
            },
        )
        .expect("vello renderer initialization");
        VectorRenderer { renderer }
    }

    /// Rasterize `scene` into `view` (the vector layer's storage texture).
    /// `view` must back an `Rgba8Unorm` + `STORAGE_BINDING` texture of the
    /// given dimensions; see [`crate::gpu::atlas::LayerTexture::with_bounds_storage`].
    /// Vello submits its own command buffer; call this before the compositor's
    /// blend pass so the downstream sample reads fresh pixels.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        let params = RenderParams {
            // Transparent background: the layer composites over what's below.
            base_color: peniko::Color::from_rgba8(0, 0, 0, 0),
            width,
            height,
            antialiasing_method: AaConfig::Area,
        };
        self.renderer
            .render_to_texture(device, queue, scene, view, &params)
            .expect("vello render_to_texture");
    }
}
