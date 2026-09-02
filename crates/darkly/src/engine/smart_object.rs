//! Turning existing content into a smart object.
//!
//! Two entry points converge here: a layer picked in the panel, and floating
//! content under an active gizmo (`engine/floating.rs`). Both end the same way:
//! the source layer is consumed and a smart object takes its tree slot, holding
//! the original pixels at their own resolution behind a transform, so that
//! ending lives in one place.

use darkly_macros::handlers;

use super::DarklyEngine;
use crate::document::Document;
use crate::layer::{LayerId, LayerNode};
use crate::transform::Transform;
use crate::undo::BakeSourceSlot;

/// Whether `node_id` can become a smart object, answered from the document
/// alone.
///
/// Requires a layer that owns its pixels: a smart object is a pristine source
/// shown through a transform, and only a layer with its own buffer has a source
/// at its own resolution to keep. A group or a generated layer would have to be
/// baked to canvas size first, which caps the resolution at exactly what
/// scaling a smart object is supposed to escape; flatten those and convert the
/// result.
///
/// A mask disqualifies the layer. The conversion moves the layer's texture, not
/// its composite, so a mask would be dropped without a trace; applying it first
/// is the honest order, and refusing says so rather than silently discarding
/// it.
///
/// Document-only so the layer panel can read the answer off `LayerInfo` instead
/// of asking per row, and so the rule has one home for both callers.
pub(crate) fn layer_can_become_smart_object(doc: &Document, node_id: LayerId) -> bool {
    matches!(doc.find_node(node_id), Some(LayerNode::Layer(_)))
        && doc
            .pixel_buffer(node_id)
            .is_some_and(|p| !p.bounds.is_empty())
        && doc.is_node_editable(node_id)
        && doc.mask_filter_id(node_id).is_none()
}

#[handlers]
impl DarklyEngine {
    /// Whether `node_id` can become a smart object; see
    /// [`layer_can_become_smart_object`] for the rule. The layer panel reads
    /// the same answer off `LayerInfo::can_become_smart_object`; this is the
    /// query for callers that hold an id and nothing else.
    #[handler]
    pub fn can_convert_layer_to_smart_object(&self, node_id: LayerId) -> bool {
        layer_can_become_smart_object(&self.doc, node_id)
    }

    /// Replace `node_id` with a smart object holding its pixels.
    ///
    /// Nothing is resampled: the layer's texture becomes the smart object's
    /// source at its own extent, and the transform is the translation that puts
    /// that extent back where it already was. The layer looks identical
    /// afterwards and is now scalable without loss.
    #[handler]
    pub fn convert_layer_to_smart_object(&mut self, node_id: LayerId) -> Result<LayerId, String> {
        if !self.can_convert_layer_to_smart_object(node_id) {
            return Err("Layer cannot become a smart object".into());
        }
        // Cloned, not borrowed: a `wgpu::Texture` is a refcounted handle, and
        // everything below takes `&mut self`.
        let (source, extent) = {
            let tex = self
                .compositor
                .node_texture(node_id)
                .ok_or("Layer has no texture")?;
            (tex.texture().clone(), tex.canvas_extent())
        };
        let transform =
            Transform::identity().then_translated(extent.origin.x as f32, extent.origin.y as f32);
        self.replace_layer_with_smart_object(
            node_id,
            transform,
            &source,
            (0, 0),
            extent.width,
            extent.height,
        )
    }

    /// Add a smart object holding `source`, anchored directly above `anchor`.
    ///
    /// No undo entry is pushed: every caller is already assembling a step (a
    /// paste discards its float in the same breath, a partial lift cuts a hole
    /// in its source), and an add that pushed its own would split those in two.
    pub(crate) fn add_smart_object_from_texture(
        &mut self,
        anchor: LayerId,
        transform: Transform,
        source: &wgpu::Texture,
        source_origin: (u32, u32),
        width: u32,
        height: u32,
    ) -> Result<LayerId, String> {
        let id = self
            .create_void_layer(
                crate::gpu::voids::smart_object::TYPE_ID,
                Vec::new(),
                Some(anchor),
                Some(transform),
            )
            .ok_or("Smart object void is not registered")?;
        self.compositor.set_void_source_from_texture(
            &self.gpu.device,
            &self.gpu.queue,
            id,
            source,
            source_origin,
            width,
            height,
        );
        // Mirror the installed size onto the document so the layer saves,
        // duplicates, and survives undo/redo. Without it `frame` stays `None`,
        // `owns_disposable_texture` is false, and the tombstone machinery frees
        // the source out from under a redo.
        self.sync_void_persistent_frame(id);
        Ok(id)
    }

    /// Consume `node_id` and put a smart object in its slot, carrying the
    /// layer's identity and blend props across and taking `source` as its
    /// embedded image.
    ///
    /// One undo step: the source layer and the smart object move together, so
    /// undoing lands on the document exactly as it was.
    pub(crate) fn replace_layer_with_smart_object(
        &mut self,
        node_id: LayerId,
        transform: Transform,
        source: &wgpu::Texture,
        source_origin: (u32, u32),
        width: u32,
        height: u32,
    ) -> Result<LayerId, String> {
        let parent = self.doc.parent_of(node_id);
        let position = self
            .doc
            .position_in_parent(node_id)
            .ok_or("Layer not in tree")?;
        let (name, visible, locked, opacity, blend_mode) = {
            let node = self.doc.find_node(node_id).ok_or("Layer missing")?;
            (
                node.common().name.clone(),
                node.common().visible,
                node.common().locked,
                node.blend().opacity,
                node.blend().blend_mode,
            )
        };

        let id = self
            .create_void_layer(
                crate::gpu::voids::smart_object::TYPE_ID,
                Vec::new(),
                Some(node_id),
                Some(transform),
            )
            .ok_or("Smart object void is not registered")?;
        if let Some(LayerNode::Layer(void)) = self.doc.find_node_mut(id) {
            void.common_mut().name = name;
            void.common_mut().visible = visible;
            void.common_mut().locked = locked;
            void.blend_mut().opacity = opacity;
            void.blend_mut().blend_mode = blend_mode;
        }
        self.refresh_blend_uniforms(id);

        self.compositor.set_void_source_from_texture(
            &self.gpu.device,
            &self.gpu.queue,
            id,
            source,
            source_origin,
            width,
            height,
        );
        self.sync_void_persistent_frame(id);

        self.finish_bake(
            vec![BakeSourceSlot {
                id: node_id,
                parent,
                position,
            }],
            id,
            parent,
            position,
        );
        Ok(id)
    }
}
