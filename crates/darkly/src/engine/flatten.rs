//! Flatten Image: composite every visible top-level layer into one raster,
//! discarding the rest. Photoshop-style — hidden layers are lost; visible
//! ones are baked into a single "Background" layer at the root.

use darkly_macros::handlers;

use super::DarklyEngine;
use crate::layer::{Layer, LayerId, LayerNode};
use crate::undo::{BakeLayersAction, BakeSourceSlot};

#[handlers]
impl DarklyEngine {
    /// Flatten the entire document into a single raster layer at root.
    /// Returns the id of the resulting raster.
    #[handler]
    pub fn flatten_image(&mut self) -> Result<LayerId, String> {
        let root_id = self.doc.root_id();
        let top_level: Vec<LayerId> = self.doc.children_of(root_id).to_vec();
        if top_level.is_empty() {
            return Err("Document has no layers to flatten".into());
        }

        // Visible top-level nodes — these get composited into the result.
        // The walk respects only direct-child visibility; descendants of a
        // visible group whose own visible flag is false are filtered by
        // the compositor.
        let visible_ids: Vec<LayerId> = top_level
            .iter()
            .filter(|&&id| {
                self.doc
                    .find_node(id)
                    .map(|n| n.common().visible)
                    .unwrap_or(false)
            })
            .copied()
            .collect();

        // Allocate the result at root, canvas-sized, Normal/100%, named
        // "Background" (Photoshop convention).
        let canvas_bounds = self.doc.canvas_rect();
        let result_id = self.doc.add_raster_layer(None);
        if let Some(LayerNode::Layer(Layer::Raster(r))) = self.doc.find_node_mut(result_id) {
            r.pixels.bounds = canvas_bounds;
            r.common.name = "Background".to_string();
        }
        self.compositor.ensure_raster_layer(
            &self.gpu.device,
            &self.gpu.queue,
            result_id,
            canvas_bounds,
        );

        // Bake the composite of every visible top-level node into the
        // result. If `visible_ids` is empty (everything hidden), the bake
        // produces a transparent result — that's the right semantic.
        self.compositor.bake_subtree_to_layer(
            &self.gpu.device,
            &self.gpu.queue,
            &mut self.doc,
            &visible_ids,
            result_id,
        );

        // Snapshot tombstones for the detached sources BEFORE detaching.
        let mut source_tombstones: Vec<LayerId> = Vec::new();
        let mut sources: Vec<BakeSourceSlot> = Vec::new();
        for (idx, &id) in top_level.iter().enumerate() {
            if id == result_id {
                continue;
            }
            source_tombstones.extend(self.collect_pixel_node_ids(id));
            sources.push(BakeSourceSlot {
                id,
                parent: Some(root_id),
                position: idx,
            });
        }

        // Detach every top-level non-result node. Textures stay alive as
        // tombstones owned by the BakeLayersAction.
        for slot in &sources {
            self.doc.detach_for_undo(slot.id);
        }

        // Reposition result to root position 0 (bottom of stack — flatten
        // makes the result the new "Background").
        self.doc.detach_for_undo(result_id);
        self.doc.reinsert_entity(result_id, Some(root_id), 0);

        let result_parent = self.doc.parent_of(result_id);
        let result_position = self.doc.position_in_parent(result_id).unwrap_or(0);

        self.compositor.mark_dirty();
        self.push_undo(Box::new(BakeLayersAction::new(
            sources,
            source_tombstones,
            result_id,
            result_parent,
            result_position,
            vec![result_id],
        )));

        Ok(result_id)
    }

    /// True when the document has at least one layer at root. Used by
    /// frontend predicates to enable/disable Flatten Image.
    #[handler]
    pub fn can_flatten(&self) -> bool {
        let root_id = self.doc.root_id();
        !self.doc.children_of(root_id).is_empty()
    }

    /// Flatten a single node into plain pixels it owns:
    ///
    /// - For a **paintable layer with filters** (a raster carrying a mask):
    ///   bakes the filters into the layer's RGBA and removes them. The layer
    ///   keeps its id, blend props, and tree position. Implemented as a re-use
    ///   of [`Self::apply_mask`] — same semantics, just a different entry name
    ///   so the UI can call "Flatten" uniformly across node kinds.
    /// - For a **layer whose pixels are generated** (a void — smart object,
    ///   camera; a filter layer; a vector layer): rasterizes it, replacing the
    ///   node with a raster holding what it currently renders. This is the
    ///   "make it paintable" path: the source layer keeps existing only inside
    ///   the undo entry.
    /// - For a **group**: bakes the group's children plus its mask into a
    ///   single raster. The result inherits the group's name, blend mode,
    ///   opacity, visible, and locked, and takes the group's tree slot.
    ///   The group's children and the group itself are tombstoned for undo.
    ///
    /// Errors when flattening would be a no-op — a raster with no filters
    /// already *is* plain pixels.
    #[handler]
    pub fn flatten_node(&mut self, node_id: LayerId) -> Result<LayerId, String> {
        if !self.doc.is_node_editable(node_id) {
            return Err("Layer is locked".into());
        }
        match self.doc.find_node(node_id) {
            Some(LayerNode::Layer(_)) => {
                if !self.is_node_paintable(node_id) {
                    self.bake_node_to_raster(node_id)
                } else if self.doc.mask_filter_id(node_id).is_some() {
                    self.apply_mask(node_id);
                    Ok(node_id)
                } else {
                    Err("Layer has no filters to flatten".into())
                }
            }
            Some(LayerNode::Group(_)) => self.bake_node_to_raster(node_id),
            None => Err("Unknown node".into()),
        }
    }

    /// Per-node flatten predicate used by the frontend right-click menu.
    /// A layer is flattenable when it owns filters to bake, or when its pixels
    /// are generated and flattening would hand it its own; groups always.
    #[handler]
    pub fn can_flatten_node(&self, node_id: LayerId) -> bool {
        match self.doc.find_node(node_id) {
            Some(LayerNode::Layer(_)) => {
                !self.is_node_paintable(node_id) || self.doc.mask_filter_id(node_id).is_some()
            }
            Some(LayerNode::Group(_)) => true,
            None => false,
        }
    }

    /// Replace `node_id` with a raster holding what that node currently
    /// renders, inheriting its identity and blend props and taking its tree
    /// slot. Kind-agnostic: a group bakes its children, a void bakes its
    /// procedural output, a vector layer bakes its rasterization.
    fn bake_node_to_raster(&mut self, node_id: LayerId) -> Result<LayerId, String> {
        // Snapshot every property we need before mutation; once we start
        // adding/detaching, the borrows churn.
        let (name, visible, locked, opacity, blend_mode, parent, position) =
            match self.doc.find_node(node_id) {
                Some(node) => (
                    node.common().name.clone(),
                    node.common().visible,
                    node.common().locked,
                    node.blend().opacity,
                    node.blend().blend_mode,
                    self.doc.parent_of(node_id),
                    self.doc.position_in_parent(node_id).unwrap_or(0),
                ),
                None => return Err("Unknown node".into()),
            };

        // Allocate the result raster, canvas-sized, inheriting the source's
        // identity props so it composites into the parent the same way the
        // source did (modulo whatever internal structure is now baked).
        let canvas_bounds = self.doc.canvas_rect();
        let result_id = self.doc.add_raster_layer(Some(node_id));
        if let Some(LayerNode::Layer(Layer::Raster(r))) = self.doc.find_node_mut(result_id) {
            r.pixels.bounds = canvas_bounds;
            r.common.name = name;
            r.common.visible = visible;
            r.common.locked = locked;
            r.blend.opacity = opacity;
            r.blend.blend_mode = blend_mode;
        }
        self.compositor.ensure_raster_layer(
            &self.gpu.device,
            &self.gpu.queue,
            result_id,
            canvas_bounds,
        );
        self.refresh_blend_uniforms(result_id);

        // Override the source's compositor uniforms to Normal/1.0 so the bake
        // captures its composite without doubling its own blend/opacity (those
        // live on the result raster instead). Only the GPU uniforms move; the
        // doc-side blend props are untouched, so undo doesn't need to revert
        // anything.
        let normal_gpu = crate::gpu::blend_mode::registry().default().gpu_value;
        self.write_blend_uniforms(node_id, 1.0, normal_gpu, false);

        // Bake the source as the single child of the transient bake accum. For
        // a group, `compose_children` recursively composes its children into
        // its composite_cache first; either way the node's own texture is
        // blended into the accum with our Normal/1 uniforms, and its mask, if
        // any, is applied as part of that blend.
        self.compositor.bake_subtree_to_layer(
            &self.gpu.device,
            &self.gpu.queue,
            &mut self.doc,
            &[node_id],
            result_id,
        );

        // Restore real uniforms so undo brings the source back in a sane state.
        self.refresh_blend_uniforms(node_id);

        // Collect tombstones before detaching — `detach_for_undo` removes
        // the parent link find_node walks rely on for `Group::children`.
        let source_tombstones = self.collect_pixel_node_ids(node_id);

        self.doc.detach_for_undo(node_id);

        // Reposition the result to take the source's slot.
        self.doc.detach_for_undo(result_id);
        self.doc.reinsert_entity(result_id, parent, position);

        let result_parent = self.doc.parent_of(result_id);
        let result_position = self.doc.position_in_parent(result_id).unwrap_or(0);

        self.compositor.mark_dirty();
        self.push_undo(Box::new(BakeLayersAction::new(
            vec![BakeSourceSlot {
                id: node_id,
                parent,
                position,
            }],
            source_tombstones,
            result_id,
            result_parent,
            result_position,
            vec![result_id],
        )));

        Ok(result_id)
    }
}
