//! Flatten Image: composite every visible top-level layer into one raster,
//! discarding the rest. Photoshop-style — hidden layers are lost; visible
//! ones are baked into a single "Background" layer at the root.

use super::DarklyEngine;
use crate::coord::CanvasRect;
use crate::layer::{Layer, LayerId, LayerNode};
use crate::undo::{BakeLayersAction, BakeSourceSlot};

impl DarklyEngine {
    /// Flatten the entire document into a single raster layer at root.
    /// Returns the id of the resulting raster.
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
        let canvas_bounds = CanvasRect::from_xywh(0, 0, self.doc.width, self.doc.height);
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
        self.doc.reinsert_node(result_id, Some(root_id), 0);

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
    pub fn can_flatten(&self) -> bool {
        let root_id = self.doc.root_id();
        !self.doc.children_of(root_id).is_empty()
    }

    /// Flatten a single node into a raster:
    ///
    /// - For a **raster layer**: bakes its modifiers (the mask) into the
    ///   layer's RGBA and removes them. The layer keeps its id, blend
    ///   props, and tree position. Implemented as a re-use of
    ///   [`Self::apply_mask`] — same semantics, just a different entry name
    ///   so the UI can call "Flatten" uniformly across node kinds.
    /// - For a **group**: bakes the group's children plus its mask into a
    ///   single raster. The result inherits the group's name, blend mode,
    ///   opacity, visible, and locked, and takes the group's tree slot.
    ///   The group's children and the group itself are tombstoned for undo.
    ///
    /// Errors when the source isn't a flattenable shape (e.g. a raster with
    /// no modifiers — flattening would be a no-op).
    pub fn flatten_node(&mut self, node_id: LayerId) -> Result<LayerId, String> {
        match self.doc.find_node(node_id) {
            Some(LayerNode::Layer(_)) => {
                if self.doc.mask_modifier_id(node_id).is_some() {
                    self.apply_mask(node_id);
                    Ok(node_id)
                } else {
                    Err("Layer has no modifiers to flatten".into())
                }
            }
            Some(LayerNode::Group(_)) => self.flatten_group(node_id),
            None => Err("Unknown node".into()),
        }
    }

    /// Per-node flatten predicate used by the frontend right-click menu.
    /// Layers are flattenable iff they own modifiers; groups always.
    pub fn can_flatten_node(&self, node_id: LayerId) -> bool {
        match self.doc.find_node(node_id) {
            Some(LayerNode::Layer(_)) => self.doc.mask_modifier_id(node_id).is_some(),
            Some(LayerNode::Group(_)) => true,
            None => false,
        }
    }

    fn flatten_group(&mut self, group_id: LayerId) -> Result<LayerId, String> {
        // Snapshot every group property we need before mutation; once we
        // start adding/detaching, the borrows churn.
        let (name, visible, locked, opacity, blend_mode, parent, position) =
            match self.doc.find_node(group_id) {
                Some(LayerNode::Group(g)) => (
                    g.common.name.clone(),
                    g.common.visible,
                    g.common.locked,
                    g.blend.opacity,
                    g.blend.blend_mode,
                    self.doc.parent_of(group_id),
                    self.doc.position_in_parent(group_id).unwrap_or(0),
                ),
                _ => return Err("Not a group".into()),
            };

        // Allocate the result raster, canvas-sized, inheriting the group's
        // identity props so it composites into the parent the same way the
        // group did (modulo the internal child structure now being baked).
        let canvas_bounds = CanvasRect::from_xywh(0, 0, self.doc.width, self.doc.height);
        let result_id = self.doc.add_raster_layer(Some(group_id));
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

        // Override the group's compositor uniforms to Normal/1.0 so the
        // bake captures the children+mask composite without doubling the
        // group's own blend/opacity (those live on the result raster
        // instead). Only the GPU uniforms move; the doc-side blend props
        // are untouched, so undo doesn't need to revert anything.
        let normal_gpu = crate::gpu::blend_mode::registry().default().gpu_value;
        self.compositor
            .update_group_uniforms(&self.gpu.queue, group_id, 1.0, normal_gpu, false);

        // Bake the group as a single child of the transient bake accum.
        // `compose_children` recursively composes the group's children into
        // its composite_cache, then blends the cache (now with our Normal/1
        // uniforms) into the bake accum. The group's mask, if any, is
        // applied as part of that blend.
        self.compositor.bake_subtree_to_layer(
            &self.gpu.device,
            &self.gpu.queue,
            &mut self.doc,
            &[group_id],
            result_id,
        );

        // Restore real uniforms so undo brings the group back in a sane
        // state.
        let isolated = self.host_renders_isolated(group_id);
        self.compositor.update_group_uniforms(
            &self.gpu.queue,
            group_id,
            opacity,
            blend_mode.gpu_value,
            isolated,
        );

        // Collect tombstones before detaching — `detach_for_undo` removes
        // the parent link find_node walks rely on for `Group::children`.
        let source_tombstones = self.collect_pixel_node_ids(group_id);

        self.doc.detach_for_undo(group_id);

        // Reposition the result to take the group's slot.
        self.doc.detach_for_undo(result_id);
        self.doc.reinsert_node(result_id, parent, position);

        let result_parent = self.doc.parent_of(result_id);
        let result_position = self.doc.position_in_parent(result_id).unwrap_or(0);

        self.compositor.mark_dirty();
        self.push_undo(Box::new(BakeLayersAction::new(
            vec![BakeSourceSlot {
                id: group_id,
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
