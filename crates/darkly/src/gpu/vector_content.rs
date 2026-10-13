//! Vector-object layer content on [`Compositor`]: the per-layer scene state
//! ([`VectorContent`]), its realization slot, and its rasterization through
//! the shared Vello renderer. The generic regenerate and canvas-resync passes
//! that drive it are in `layer_content.rs`.

use crate::gpu::atlas::LayerTexture;
use crate::gpu::compositor::Compositor;
use crate::gpu::layer_content::{ContentBackends, LayerContent};
use crate::layer::LayerId;

/// Realization input for a vector-object layer: the plane-space
/// `vello::Scene` the engine built from the document's objects, plus a
/// "needs re-rasterize" flag. The flag is set when the engine pushes a new
/// scene (object/style/transform change) or the canvas window moves, and is
/// cleared when the scene is rasterized: never on view zoom/pan
/// (raster-first).
///
/// The scene stays in plane coordinates; the move into the texture's frame
/// happens at render time, so a reallocated texture re-renders it with no
/// engine round trip.
pub(super) struct VectorContent {
    scene: vello::Scene,
    dirty: bool,
}

impl VectorContent {
    /// An empty scene, dirty so the first composite produces a fresh texture.
    fn new() -> Self {
        VectorContent {
            scene: vello::Scene::new(),
            dirty: true,
        }
    }

    fn set_scene(&mut self, scene: vello::Scene) {
        self.scene = scene;
        self.dirty = true;
    }

    pub(super) fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The texture was reallocated at the new window, empty.
    pub(super) fn canvas_rect_changed(&mut self) {
        self.dirty = true;
    }

    /// Rasterize the scene into `dst`. Vello submits its own command buffer.
    pub(super) fn regenerate(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        backends: &mut ContentBackends,
        dst: &LayerTexture,
    ) {
        backends
            .vector_renderer(device)
            .render(device, queue, &self.scene, dst);
    }
}

impl Compositor {
    /// Allocate the per-instance GPU state for a new vector-object layer: a
    /// canvas-window-sized `Rgba8Unorm` + `STORAGE_BINDING` texture (Vello
    /// renders into it as a storage image) and a `LayerCache` with blend
    /// uniforms and an empty [`LayerContent::Vector`]. The engine builds the
    /// `vello::Scene` from the document objects and pushes it via
    /// [`Self::set_vector_scene`]. Idempotent.
    ///
    /// The texture is window-sized, like a void's, so it costs a full canvas
    /// of VRAM whatever the objects cover. Sizing it to the objects' padded
    /// bounds instead (plane-anchored, grown in chunks like a raster layer,
    /// left alone by a resize) would make the cost proportional to content;
    /// it needs the drawn bounds of strokes and shaped text before
    /// rendering, and is a change to this variant only.
    pub fn ensure_vector_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
    ) {
        if self.layer_cache.contains_key(&layer_id) {
            return;
        }
        let bounds = self.canvas_rect();
        let layer_tex = LayerTexture::with_bounds_storage(device, bounds);
        self.insert_content_layer(
            device,
            queue,
            layer_id,
            layer_tex,
            LayerContent::Vector(VectorContent::new()),
        );
        self.mark_dirty();
    }

    /// Replace the realized `vello::Scene` for a vector layer and mark it dirty
    /// so the next composite re-rasterizes. The engine builds the scene from
    /// the document's authoritative objects (text shaped by parley, paths from
    /// kurbo) in plane coordinates: the compositor stays ignorant of fonts and
    /// geometry. No-op if the layer wasn't ensured.
    pub fn set_vector_scene(&mut self, layer_id: LayerId, scene: vello::Scene) {
        let Some(vc) = self
            .layer_cache
            .get_mut(&layer_id)
            .and_then(|c| c.content.vector_mut())
        else {
            return;
        };
        vc.set_scene(scene);
        self.mark_dirty();
    }

    /// Compile the vector renderer's pipelines now (if not already), so the first
    /// vector layer doesn't stall on the shader-compile cost. Building it compiles
    /// Vello's full compute-pipeline set (a >1s one-time cost). Called when the
    /// text tool is selected: the compile then overlaps the gap before the user
    /// commits a text box, rather than blocking the frame that would show it.
    /// Idempotent: a no-op once the renderer exists.
    pub fn ensure_vector_renderer(&mut self, device: &wgpu::Device) {
        self.backends.vector_renderer(device);
    }
}
