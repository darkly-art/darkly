//! Procedural (void) layer content on [`Compositor`]: the per-layer
//! [`ProceduralContent`] state, void realization and parameter/transform
//! updates, and source-image install and copy. The generic regenerate and
//! canvas-resync passes that drive it are in `layer_content.rs`.

use crate::coord::CanvasRect;
use crate::gpu::atlas::LayerTexture;
use crate::gpu::compositor::Compositor;
use crate::gpu::effect::EffectCache;
use crate::gpu::layer_content::LayerContent;
use crate::gpu::params::ParamValue;
use crate::gpu::void::Void;
use crate::layer::LayerId;

/// Per-instance procedural-content state. The "needs re-encode" flag lives
/// on the [`Void`] itself ([`Void::take_dirty`]): the compositor neither
/// stores nor reconciles it.
pub(super) struct ProceduralContent {
    /// The procedural-content trait object. Owned here (one per layer)
    /// because animation mutates its `time` field.
    pub(super) void: Box<dyn Void>,
    /// Per-instance GPU resources for the void's own pipeline (uniform
    /// buffer + bind groups built off the registry's shared pipeline).
    pub(super) cache: EffectCache,
}

impl ProceduralContent {
    pub(super) fn take_dirty(&mut self) -> bool {
        self.void.take_dirty()
    }

    /// Voids caching canvas geometry in their sampling uniform rewrite it
    /// (the uniform is window-local and, for a cover fit, derived from the
    /// canvas size). The texture was just reallocated empty, so the void
    /// redraws even if its own state is unchanged.
    pub(super) fn canvas_rect_changed(&mut self, queue: &wgpu::Queue, canvas: CanvasRect) {
        self.void.set_canvas_rect(queue, &self.cache, canvas);
        self.void.mark_dirty();
    }

    pub(super) fn regenerate(&self, encoder: &mut wgpu::CommandEncoder, dst: &wgpu::TextureView) {
        self.void.encode(encoder, &self.cache, dst);
    }
}

impl Compositor {
    /// Construct a `Box<dyn Void>` via the compositor-owned registry without
    /// exposing the registry itself. Used by [`crate::layer::VoidLayer`]'s
    /// [`crate::gpu::compositor::LayerKindGpu::realize_in`] so the registry
    /// stays private to the compositor.
    pub(crate) fn create_void_box(
        &mut self,
        device: &wgpu::Device,
        type_id: &str,
        params: &[ParamValue],
    ) -> Box<dyn Void> {
        let format = self.canvas_content_format();
        self.void_registry
            .create_void(type_id, params, device, format)
    }

    /// Allocate the per-instance GPU state for a new void layer:
    /// procedural texture in [`Self::node_textures`] (canvas-sized
    /// [`wgpu::TextureFormat::Rgba8Unorm`], matching the raster path so the
    /// compositor's blend pipeline can sample it without any kind-specific
    /// branch) and a `LayerCache` holding the blend uniforms plus a
    /// [`LayerContent::Procedural`] sidecar with the trait object and its
    /// `EffectCache`. Idempotent, calling twice for the same id is a no-op.
    ///
    /// The caller constructs `void` via the engine's void registry. The
    /// compositor takes the trait object as-is and stops bookkeeping the
    /// `(type_id, params)` pair: ownership of those facts already lives on
    /// the `Void` itself.
    pub fn ensure_void_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        mut void: Box<dyn Void>,
    ) {
        if self.layer_cache.contains_key(&layer_id) {
            return;
        }
        // Canvas-sized texture so the procedural output composites against
        // the same coordinate system as raster layers.
        let bounds = self.canvas_rect();
        let layer_tex = LayerTexture::with_bounds(device, bounds);

        let cache = void.create_cache(
            device,
            queue,
            layer_tex.view(),
            &self.sampler,
            self.canvas_width,
            self.canvas_height,
        );

        self.insert_content_layer(
            device,
            queue,
            layer_id,
            layer_tex,
            LayerContent::Procedural(ProceduralContent { void, cache }),
        );
        self.mark_dirty();
    }

    /// Borrow a layer's procedural-content sidecar, if any. Returns `None`
    /// for raster layers (no sidecar) and unknown ids. Centralises the
    /// "is this a procedural layer?" lookup so the rest of the compositor
    /// never pattern-matches on [`LayerContent`] directly.
    pub(super) fn procedural_content(&self, layer_id: LayerId) -> Option<&ProceduralContent> {
        self.layer_cache
            .get(&layer_id)
            .and_then(|c| c.content.procedural())
    }

    /// Mutable counterpart to [`Self::procedural_content`].
    fn procedural_content_mut(&mut self, layer_id: LayerId) -> Option<&mut ProceduralContent> {
        self.layer_cache
            .get_mut(&layer_id)
            .and_then(|c| c.content.procedural_mut())
    }

    /// Iterate every realized procedural layer as `(id, sidecar)`. The
    /// whole-pool counterpart of [`Self::procedural_content`]: animation
    /// ticks and canvas resync walk the pool through this, so no consumer
    /// pattern-matches [`LayerContent`] itself.
    pub(super) fn procedural_entries(&self) -> impl Iterator<Item = (LayerId, &ProceduralContent)> {
        self.layer_cache
            .iter()
            .filter_map(|(id, c)| Some((*id, c.content.procedural()?)))
    }

    /// Mutable counterpart to [`Self::procedural_entries`].
    pub(super) fn procedural_entries_mut(
        &mut self,
    ) -> impl Iterator<Item = (LayerId, &mut ProceduralContent)> {
        self.layer_cache
            .iter_mut()
            .filter_map(|(id, c)| Some((*id, c.content.procedural_mut()?)))
    }

    /// Current persistent frame size of a void layer, if the void declares
    /// one. Used by the engine after `upload_void_external_image` to keep
    /// the doc's [`crate::layer::VoidLayer::frame`] in sync with the
    /// GPU-side texture so save sees the right dimensions.
    pub fn void_persistent_frame_size(&self, layer_id: LayerId) -> Option<(u32, u32)> {
        self.procedural_content(layer_id)
            .and_then(|p| p.void.persistent_frame_size())
    }

    /// Install a void layer's source image. Wraps
    /// [`crate::gpu::void::Void::set_source_pixels`]: the void reallocates its
    /// source texture at `(width, height)`, rebuilds its bind group, and writes
    /// the bytes. Two callers: document load restoring a saved frame, and
    /// placement installing a user-supplied image. `bytes` are premultiplied
    /// RGBA8 in both cases.
    ///
    /// A source allocated with mip levels gets its chain regenerated here,
    /// because the pass that does it is the compositor's. The texture declares
    /// its own need: more than one level means the void asked for a chain.
    pub fn set_void_source_pixels(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        width: u32,
        height: u32,
        bytes: &[u8],
    ) {
        let Some(proc) = self.procedural_content_mut(layer_id) else {
            return;
        };
        proc.void
            .set_source_pixels(device, queue, &mut proc.cache, width, height, bytes);

        self.regenerate_void_mips(device, queue, layer_id);
        self.mark_dirty();
    }

    /// Rebuild `layer_id`'s void source mip chain from its level 0. No-op when
    /// the source has no chain (a streaming void allocates a single level).
    ///
    /// Every writer of a void source ends here, so minification quality does
    /// not depend on which route the texels arrived by.
    fn regenerate_void_mips(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
    ) {
        let Some(tex) = self
            .procedural_content(layer_id)
            .and_then(|p| p.cache.aux_textures.first())
            .cloned()
        else {
            return;
        };
        let levels = tex.mip_level_count();
        if levels <= 1 {
            return;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("void-source-mips"),
        });
        // Void sources are stored premultiplied, so texels average as-is: no
        // premultiply/un-premultiply round trip.
        self.rescale_pass
            .generate_mip_chain(device, &mut encoder, &tex, levels, true);
        queue.submit([encoder.finish()]);
    }

    /// Install `src` as `layer_id`'s void source by GPU copy, reading a
    /// `width × height` region at `src_origin`.
    ///
    /// The ingress for texels that are already on the GPU (a trimmed layer
    /// region, another void's source), so they never make a round trip through
    /// the CPU to get here. `src` must already be in the aux texture's
    /// **premultiplied** convention and copyable (`COPY_SRC`).
    ///
    /// Only level 0 is copied; the chain is regenerated from it.
    pub fn set_void_source_from_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        src: &wgpu::Texture,
        src_origin: (u32, u32),
        width: u32,
        height: u32,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let Some(proc) = self.procedural_content_mut(layer_id) else {
            return;
        };
        proc.void
            .allocate_source(device, queue, &mut proc.cache, width, height);
        let Some(dst) = proc.cache.aux_textures.first().cloned() else {
            return;
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("void-source-from-texture"),
        });
        crate::gpu::blit_region_mip(
            &mut encoder,
            src,
            src_origin,
            &dst,
            (0, 0),
            width,
            height,
            0,
        );
        queue.submit([encoder.finish()]);

        self.regenerate_void_mips(device, queue, layer_id);
        self.mark_dirty();
    }

    /// Copy one void's source image onto another's.
    ///
    /// Duplication needs this because an externally-sourced image is not
    /// reproducible from the layer's params: the copy would otherwise render
    /// blank. Both voids must already be realized.
    ///
    /// Level 0 is copied and the destination's chain regenerated from it,
    /// rather than copying every level across. The pyramid is deterministic, so
    /// the result matches, and going through the shared ingress is what keeps
    /// this from being a third hand-written way to fill a void source.
    pub fn copy_void_source(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src_id: LayerId,
        dst_id: LayerId,
    ) {
        // Cloned out before the `&mut self` ingress call below, because the
        // source texture is borrowed from the same map the destination is
        // written through. `wgpu::Texture` is a refcounted handle, so this is
        // cheap.
        let Some(src) = self
            .procedural_content(src_id)
            .and_then(|p| p.cache.aux_textures.first())
            .cloned()
        else {
            return;
        };
        let Some((width, height)) = self
            .procedural_content(src_id)
            .and_then(|p| p.void.persistent_frame_size())
        else {
            return;
        };
        self.set_void_source_from_texture(device, queue, dst_id, &src, (0, 0), width, height);
    }

    /// Update a void's procedural inputs in place. The void mutates its
    /// own fields and rewrites the uniform buffer; the existing
    /// `EffectCache` (including any aux textures the void was using to
    /// hold stateful pixel data: e.g. the camera void's last received
    /// frame) is preserved untouched. The blend uniforms (opacity / mode
    /// / isolated) are also untouched: only the procedural side changes.
    pub fn update_void_layer_params(
        &mut self,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        params: &[ParamValue],
    ) {
        let Some(proc) = self.procedural_content_mut(layer_id) else {
            return;
        };
        proc.void.update_params(queue, &proc.cache, params);
        self.mark_dirty();
    }

    /// Apply a void's user transform in place. Sibling of
    /// [`Self::update_void_layer_params`]: delegates to [`Void::set_transform`],
    /// which rewrites the uniform without rebuilding (preserving any aux
    /// textures, e.g. the camera's live frame). No-op for non-procedural or
    /// non-transform-aware voids.
    pub fn update_void_layer_transform(
        &mut self,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        transform: &crate::transform::Transform,
    ) {
        let Some(proc) = self.procedural_content_mut(layer_id) else {
            return;
        };
        proc.void.set_transform(queue, &proc.cache, transform);
        self.mark_dirty();
    }

    /// The void's active-pixel bbox in PLANE coords, via
    /// [`Void::content_extent`]. The canvas window for most voids; the
    /// cover-fit rect for a stream; the source's natural rect for a placed
    /// image. `None` if `layer_id` isn't a realized void.
    pub fn void_content_extent(&self, layer_id: LayerId) -> Option<crate::gpu::void::ContentRect> {
        let proc = self.procedural_content(layer_id)?;
        Some(proc.void.content_extent(self.canvas_rect()))
    }

    /// Push a fresh external image frame (webcam, screenshare, …) into a
    /// void's input texture. Delegates to the void's
    /// [`Void::upload_external_image`], which handles texture allocation,
    /// bind-group rebuild on dimension changes, and the actual texel copy.
    /// Flags the void's destination texture dirty so the next compositor
    /// frame re-renders it.
    pub fn upload_void_external_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        source: crate::gpu::void::ExternalImageSource,
    ) {
        let Some(proc) = self.procedural_content_mut(layer_id) else {
            return;
        };
        if !proc.void.wants_external_input() {
            return;
        }
        proc.void
            .upload_external_image(device, queue, &mut proc.cache, source);
        // A camera frame is advanced appearance, not an authored pixel write
        // or a document edit: the composite must re-run, but thumbnails and
        // content bounds have no reason to churn at frame rate.
        self.revisions.bump_animation(layer_id);
    }
}
