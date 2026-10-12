//! How a content layer's node texture gets its pixels ([`LayerContent`]),
//! and the compositor passes that are generic over it: regenerating dirty
//! content before each composite, and following the canvas window when it
//! moves. Each kind's own state and behaviour lives in its module
//! (`void_content.rs`, `vector_content.rs`); this file only delegates.

use crate::coord::CanvasRect;
use crate::gpu::atlas::LayerTexture;
use crate::gpu::compositor::Compositor;
use crate::gpu::vector_content::VectorContent;
use crate::gpu::vector_renderer::VectorRenderer;
use crate::gpu::void_content::ProceduralContent;
use crate::layer::LayerId;

/// How a layer's pixels reach `node_textures[id]`.
///
/// - `Raster`: pixels arrive via paint / paste / fill; the texture is
///   authoritative and plane-anchored, and the compositor never regenerates
///   it.
/// - `Procedural`: a [`crate::gpu::void::Void`] regenerates the texture
///   whenever its own dirty flag reports stale.
/// - `Vector`: a plane-space `vello::Scene` the engine built from the
///   document's objects is rasterized into the texture when it changes.
///
/// Regenerated content fills the canvas window by definition, so its
/// texture is window-anchored and follows the window on resize.
pub(super) enum LayerContent {
    Raster,
    Procedural(ProceduralContent),
    Vector(VectorContent),
}

/// Renderers shared by every layer of one content kind, created on first use
/// so a project that never has that kind never pays for it. Content borrows
/// them while it regenerates.
#[derive(Default)]
pub(super) struct ContentBackends {
    vector: Option<VectorRenderer>,
}

impl ContentBackends {
    /// The vector renderer, compiling its pipelines on first call (a >1s
    /// one-time cost).
    pub(super) fn vector_renderer(&mut self, device: &wgpu::Device) -> &mut VectorRenderer {
        self.vector
            .get_or_insert_with(|| VectorRenderer::new(device))
    }
}

impl LayerContent {
    /// Whether the texture is the canvas window by definition, so a resize
    /// reallocates it at the new window. Raster textures are plane-anchored:
    /// a crop keeps their off-window pixels.
    fn window_anchored(&self) -> bool {
        match self {
            LayerContent::Raster => false,
            LayerContent::Procedural(_) | LayerContent::Vector(_) => true,
        }
    }

    /// Return whether the texture needs regenerating, and clear the flag.
    fn take_dirty(&mut self) -> bool {
        match self {
            LayerContent::Raster => false,
            LayerContent::Procedural(p) => p.take_dirty(),
            LayerContent::Vector(v) => v.take_dirty(),
        }
    }

    /// The canvas window moved to `canvas`, and the texture was reallocated
    /// there (empty).
    fn canvas_rect_changed(&mut self, queue: &wgpu::Queue, canvas: CanvasRect) {
        match self {
            LayerContent::Raster => {}
            LayerContent::Procedural(p) => p.canvas_rect_changed(queue, canvas),
            LayerContent::Vector(v) => v.canvas_rect_changed(),
        }
    }

    /// Produce the texture's pixels in `dst`, ordered before the composite
    /// recorded into `encoder`: either recorded into it, or submitted to
    /// `queue` ahead of it.
    fn regenerate(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        backends: &mut ContentBackends,
        dst: &LayerTexture,
    ) {
        match self {
            LayerContent::Raster => {}
            LayerContent::Procedural(p) => p.regenerate(encoder, dst.view()),
            LayerContent::Vector(v) => v.regenerate(device, queue, backends, dst),
        }
    }

    pub(super) fn procedural(&self) -> Option<&ProceduralContent> {
        match self {
            LayerContent::Procedural(p) => Some(p),
            _ => None,
        }
    }

    pub(super) fn procedural_mut(&mut self) -> Option<&mut ProceduralContent> {
        match self {
            LayerContent::Procedural(p) => Some(p),
            _ => None,
        }
    }

    pub(super) fn vector_mut(&mut self) -> Option<&mut VectorContent> {
        match self {
            LayerContent::Vector(v) => Some(v),
            _ => None,
        }
    }
}

impl Compositor {
    /// Regenerate every dirty content layer's texture. Runs at the top of the
    /// compositor's encode pass so the blend in `compose_children` samples
    /// up-to-date pixels. Each content clears its own flag through
    /// [`LayerContent::take_dirty`], so a layer regenerates at most once per
    /// state change.
    pub(super) fn encode_dirty_layer_content(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        // Two-phase: collect the dirty ids (consuming the flags), then
        // regenerate each, borrowing `node_textures`, `layer_cache` and
        // `backends` as the disjoint fields they are. The scratch keeps its
        // capacity across frames.
        self.dirty_content_scratch.clear();
        self.dirty_content_scratch.extend(
            self.layer_cache
                .iter_mut()
                .filter_map(|(id, c)| c.content.take_dirty().then_some(*id)),
        );
        for i in 0..self.dirty_content_scratch.len() {
            let id = self.dirty_content_scratch[i];
            let (Some(slot), Some(cache)) =
                (self.node_textures.get(&id), self.layer_cache.get(&id))
            else {
                continue;
            };
            cache
                .content
                .regenerate(device, queue, encoder, &mut self.backends, &slot.texture);
        }
        // No stale LayerIds outlive the pass (dispose may follow).
        self.dirty_content_scratch.clear();
    }

    /// Reallocate every window-anchored content texture at the current canvas
    /// window and tell its content the window moved, so it redraws there.
    /// Called from [`Self::set_canvas_rect`]; without it a resize or crop
    /// leaves the content drawing into the old window's footprint.
    pub(super) fn resync_window_content_to_canvas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let canvas = self.canvas_rect();
        let ids: Vec<LayerId> = self
            .layer_cache
            .iter()
            .filter_map(|(id, c)| c.content.window_anchored().then_some(*id))
            .collect();
        for id in ids {
            // `realloc_node_texture` goes through `swap_node_texture`, which
            // also refreshes the blend uniform's extent and drops bind groups
            // naming the old view: without that the composite samples the new
            // texture through the old extent.
            self.realloc_node_texture(device, queue, id, canvas, None);
            if let Some(cache) = self.layer_cache.get_mut(&id) {
                cache.content.canvas_rect_changed(queue, canvas);
            }
        }
    }
}
