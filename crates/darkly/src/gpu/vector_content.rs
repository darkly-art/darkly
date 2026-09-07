//! Vector-object layer realization on [`Compositor`]: the shared Vello
//! renderer, per-layer scene state ([`VectorSubsystem`]), and the
//! rasterization of dirty scenes into their storage textures before each
//! composite.

use crate::gpu::atlas::LayerTexture;
use crate::gpu::compositor::Compositor;
use crate::gpu::void_content::LayerContent;
use crate::layer::LayerId;
use std::collections::HashMap;

/// Realization input for a vector-object layer: the `vello::Scene` the engine
/// built from the document's objects, plus a "needs re-rasterize" flag. Held in
/// a separate map (not `LayerContent`) because vector layers reuse the raster
/// blend path verbatim — only their texture source differs. `dirty` flips when
/// the engine pushes a new scene (object/style/transform change) and clears
/// after [`Compositor::realize_dirty_vector_layers`] rasterizes it — never on
/// view zoom/pan (raster-first).
pub(super) struct VectorContent {
    scene: vello::Scene,
    dirty: bool,
}

/// The compositor's vector-layer state: the one lazily-created Vello renderer
/// shared by every vector layer, and the per-layer realization inputs keyed by
/// layer id.
pub(super) struct VectorSubsystem {
    /// One Vello renderer shared by every vector layer, created lazily on the
    /// first vector-layer realization so projects with none never pay its
    /// shader-compile cost.
    pub(super) renderer: Option<crate::gpu::vector_renderer::VectorRenderer>,
    /// Per-vector-layer realization input (the `vello::Scene` + dirty flag).
    /// Keyed by layer id; entries are created by
    /// [`Compositor::ensure_vector_layer`] and removed alongside the layer's
    /// other GPU resources on dispose.
    pub(super) scenes: HashMap<LayerId, VectorContent>,
}

impl Compositor {
    /// Allocate the per-instance GPU state for a new vector-object layer:
    /// a canvas-sized `Rgba8Unorm` + `STORAGE_BINDING` texture (Vello renders
    /// into it as a storage image) and a `LayerCache` with blend uniforms.
    ///
    /// Unlike a void, a vector layer carries no procedural sidecar — its
    /// `LayerContent` is `Raster` so the void animation/dirty machinery skips
    /// it. The realization is driven separately: the engine builds a
    /// `vello::Scene` from the document objects and pushes it via
    /// [`Self::set_vector_scene`], and [`Self::realize_dirty_vector_layers`]
    /// rasterizes dirty scenes before each composite. Idempotent.
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
        self.insert_content_layer(device, queue, layer_id, layer_tex, LayerContent::Raster);
        // Empty scene, dirty so the first composite produces a fresh texture.
        self.vector.scenes.insert(
            layer_id,
            VectorContent {
                scene: vello::Scene::new(),
                dirty: true,
            },
        );
        self.mark_dirty();
    }

    /// Replace the realized `vello::Scene` for a vector layer and mark it dirty
    /// so the next composite re-rasterizes. The engine builds the scene from
    /// the document's authoritative objects (text shaped by parley, paths from
    /// kurbo) — the compositor stays ignorant of fonts and geometry. No-op if
    /// the layer wasn't ensured.
    pub fn set_vector_scene(&mut self, layer_id: LayerId, scene: vello::Scene) {
        if let Some(vc) = self.vector.scenes.get_mut(&layer_id) {
            vc.scene = scene;
            vc.dirty = true;
            self.mark_dirty();
        }
    }

    /// Compile the vector renderer's pipelines now (if not already), so the first
    /// vector layer doesn't stall on the shader-compile cost. Building it compiles
    /// Vello's full compute-pipeline set (a >1s one-time cost). Called when the
    /// text tool is selected — the compile then overlaps the gap before the user
    /// commits a text box, rather than blocking the frame that would show it.
    /// Idempotent: a no-op once the renderer exists.
    pub fn ensure_vector_renderer(&mut self, device: &wgpu::Device) {
        self.vector
            .renderer
            .get_or_insert_with(|| crate::gpu::vector_renderer::VectorRenderer::new(device));
    }

    /// Rasterize every dirty vector layer's scene into its storage texture.
    /// Runs before the composite pass (in `render_offscreen`) so the blend
    /// walk samples up-to-date pixels. Lazily constructs the shared
    /// [`VectorRenderer`](crate::gpu::vector_renderer::VectorRenderer) on
    /// first use (see [`Self::ensure_vector_renderer`]).
    /// Vello submits its own command buffer per layer; those submits are ordered
    /// before the compositor's, so GPU ordering is preserved.
    pub(super) fn realize_dirty_vector_layers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let dirty: Vec<LayerId> = self
            .vector
            .scenes
            .iter()
            .filter_map(|(id, vc)| vc.dirty.then_some(*id))
            .collect();
        if dirty.is_empty() {
            return;
        }
        let renderer = self
            .vector
            .renderer
            .get_or_insert_with(|| crate::gpu::vector_renderer::VectorRenderer::new(device));
        for id in dirty {
            let Some(tex) = self.node_textures.get(&id).map(|s| &s.texture) else {
                continue;
            };
            let extent = tex.layer_extent();
            let Some(vc) = self.vector.scenes.get_mut(&id) else {
                continue;
            };
            renderer.render(
                device,
                queue,
                &vc.scene,
                tex.view(),
                extent.width,
                extent.height,
            );
            vc.dirty = false;
        }
    }
}
