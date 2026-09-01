//! Merge Down: bake the active node + the sibling immediately below it
//! into a single raster layer at the lower sibling's tree slot. Photoshop-
//! style: if either side is a group, it gets flattened during the bake.
//!
//! The undo system tombstones the consumed sources so undo restores
//! everything intact; on redo, the engine recomposes the result from the
//! restored sources via [`crate::gpu::compositor::Compositor::bake_subtree_to_layer`].

use darkly_macros::handlers;

use super::DarklyEngine;
use crate::layer::{Layer, LayerId, LayerNode};
use crate::undo::{BakeLayersAction, BakeSourceSlot};

#[handlers]
impl DarklyEngine {
    /// Merge the layer at `source_id` into the sibling directly below it in
    /// the same parent. Returns the id of the resulting raster layer.
    ///
    /// Errors when `source_id` has no sibling below it, or when the target
    /// (the layer below) is locked. Either side may be a group; both are
    /// flattened into the result.
    #[handler]
    pub fn merge_down(&mut self, source_id: LayerId) -> Result<LayerId, String> {
        // Source itself is consumed (tombstoned) by the merge: locking it
        // protects it from being destroyed. Target is overwritten with the
        // merged pixels; locking it protects its content. Both ends checked
        // before any tree resolution so the error message is precise.
        if !self.doc.is_node_editable(source_id) {
            return Err("Source layer is locked".into());
        }
        // Resolve target: the sibling at (source_position - 1) in the same
        // parent. If source is at position 0 (or has no parent), fail.
        //
        // `position_in_parent` indexes whichever list holds the entity, so a
        // modifier id would yield a *filter* index and then read the host's
        // `children` with it. A modifier has no sibling below to merge into, so
        // reject it before the arithmetic rather than indexing the wrong list.
        if self.doc.is_filter(source_id) {
            return Err("Layer not in tree".into());
        }
        let parent = self.doc.parent_of(source_id);
        let pos = self
            .doc
            .position_in_parent(source_id)
            .ok_or("Layer not in tree")?;
        if pos == 0 {
            return Err("Nothing below to merge into".into());
        }
        let parent_id = parent.ok_or("Layer has no parent")?;
        let target_id = self.doc.children_of(parent_id)[pos - 1];

        if !self.doc.is_node_editable(target_id) {
            return Err("Target layer is locked".into());
        }

        // Snapshot target's properties to inherit on the result (Photoshop
        // convention: the lower layer keeps its identity; the upper bakes
        // its blend into the pixels).
        let (target_name, target_visible, target_locked, target_opacity, target_blend_mode) = {
            let t = self.doc.find_node(target_id).ok_or("Target node missing")?;
            (
                t.common().name.clone(),
                t.common().visible,
                t.common().locked,
                t.blend().opacity,
                t.blend().blend_mode,
            )
        };
        let target_pos_before = pos - 1;

        // Allocate the result raster, canvas-sized.
        let canvas_bounds = self.doc.canvas_rect();
        let result_id = self.doc.add_raster_layer(Some(target_id));
        if let Some(LayerNode::Layer(Layer::Raster(r))) = self.doc.find_node_mut(result_id) {
            r.pixels.bounds = canvas_bounds;
            r.common.name = target_name;
            r.common.visible = target_visible;
            r.common.locked = target_locked;
            r.blend.opacity = target_opacity;
            r.blend.blend_mode = target_blend_mode;
        }
        self.compositor.ensure_raster_layer(
            &self.gpu.device,
            &self.gpu.queue,
            result_id,
            canvas_bounds,
        );
        self.refresh_blend_uniforms(result_id);

        // Bake target (lower) + source (upper) into result. Order matters:
        // target is composited first so source's blend mode applies on top.
        self.compositor.bake_subtree_to_layer(
            &self.gpu.device,
            &self.gpu.queue,
            &mut self.doc,
            &[target_id, source_id],
            result_id,
        );

        // Collect tombstone ids BEFORE detaching: once detached, find_node
        // returns None and we can't walk the subtree.
        let mut source_tombstones = self.collect_pixel_node_ids(target_id);
        source_tombstones.extend(self.collect_pixel_node_ids(source_id));

        // Record the source/target slots so undo can put them back where
        // they were. Positions are captured BEFORE detach.
        let source_pos = self.doc.position_in_parent(source_id).unwrap_or(0);
        let target_pos = self.doc.position_in_parent(target_id).unwrap_or(0);
        let sources = vec![
            BakeSourceSlot {
                id: target_id,
                parent,
                position: target_pos,
            },
            BakeSourceSlot {
                id: source_id,
                parent,
                position: source_pos,
            },
        ];

        // Detach sources. Their textures stay alive in node_textures as
        // tombstones owned by the BakeLayersAction.
        self.doc.detach_for_undo(target_id);
        self.doc.detach_for_undo(source_id);

        // Move the result into target's original slot. `add_raster_layer`
        // with anchor=target_id placed it above; now that target is detached
        // we need to land at `target_pos_before` exactly.
        let _ = target_pos_before;
        // Reposition the result to the target's old position. Simplest:
        // detach then re-insert.
        self.doc.detach_for_undo(result_id);
        self.doc
            .reinsert_entity(result_id, parent, target_pos_before);

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

    /// True when `source_id` has a same-parent sibling immediately below
    /// it. Used by frontend predicates to enable/disable Merge Down.
    #[handler]
    pub fn can_merge_down(&self, source_id: LayerId) -> bool {
        let Some(pos) = self.doc.position_in_parent(source_id) else {
            return false;
        };
        pos > 0
    }

    /// Bake every layer in `ids` (≥2, any parent) into one raster placed at
    /// the panel-topmost selected layer's slot. The result inherits the
    /// topmost's name / blend mode / opacity / visibility; sources from
    /// other parents are detached and tombstoned. Groups in the selection
    /// are flattened during the bake. Returns the id of the result.
    ///
    /// Errors when fewer than two distinct sources are provided, when any
    /// source is locked (a partial bake is destructive), or when a source
    /// id isn't in the tree.
    #[handler]
    pub fn merge_layers(&mut self, ids: Vec<LayerId>) -> Result<LayerId, String> {
        // De-dup while preserving caller order, important for the error
        // path that names the input set in messages.
        let mut unique: Vec<LayerId> = Vec::with_capacity(ids.len());
        for id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        if unique.len() < 2 {
            return Err("Merge needs at least two layers".into());
        }
        for &id in &unique {
            if self.doc.find_node(id).is_none() {
                return Err("Layer not in tree".into());
            }
            if !self.doc.is_node_editable(id) {
                return Err("A selected layer is locked".into());
            }
        }

        // Sort by panel display order so the bake walks bottom-to-top and
        // the LAST entry is the topmost selected layer in the panel: that
        // one anchors the result's slot and provides the inherited props.
        let order = self.doc.all_node_ids_in_order();
        let order_idx = |id: LayerId| order.iter().position(|&x| x == id).unwrap_or(usize::MAX);
        unique.sort_by_key(|&id| order_idx(id));
        let topmost_id = *unique.last().expect("len >= 2");

        // Snapshot the topmost's properties + slot now, before any detach.
        let (top_name, top_visible, top_locked, top_opacity, top_blend_mode) = {
            let t = self
                .doc
                .find_node(topmost_id)
                .ok_or("Topmost source missing")?;
            (
                t.common().name.clone(),
                t.common().visible,
                t.common().locked,
                t.blend().opacity,
                t.blend().blend_mode,
            )
        };
        let topmost_parent = self.doc.parent_of(topmost_id);
        let topmost_pos = self
            .doc
            .position_in_parent(topmost_id)
            .ok_or("Topmost source not in tree")?;

        // Record each source's prior (parent, position) for undo reinsert.
        // Captured BEFORE any detach so positions reflect the live tree.
        let sources: Vec<BakeSourceSlot> = unique
            .iter()
            .map(|&id| BakeSourceSlot {
                id,
                parent: self.doc.parent_of(id),
                position: self.doc.position_in_parent(id).unwrap_or(0),
            })
            .collect();

        // Allocate the result raster, anchored at the topmost so it sits
        // in the right parent group; we'll reposition exactly after the
        // sources detach.
        let canvas_bounds = self.doc.canvas_rect();
        let result_id = self.doc.add_raster_layer(Some(topmost_id));
        if let Some(LayerNode::Layer(Layer::Raster(r))) = self.doc.find_node_mut(result_id) {
            r.pixels.bounds = canvas_bounds;
            r.common.name = top_name;
            r.common.visible = top_visible;
            r.common.locked = top_locked;
            r.blend.opacity = top_opacity;
            r.blend.blend_mode = top_blend_mode;
        }
        self.compositor.ensure_raster_layer(
            &self.gpu.device,
            &self.gpu.queue,
            result_id,
            canvas_bounds,
        );
        self.refresh_blend_uniforms(result_id);

        // Bake sources bottom-to-top (the `unique` order after sorting)
        // so blend modes stack correctly when composited into the result.
        self.compositor.bake_subtree_to_layer(
            &self.gpu.device,
            &self.gpu.queue,
            &mut self.doc,
            &unique,
            result_id,
        );

        // Collect tombstones from every source BEFORE detach so the walk
        // can reach the subtrees.
        let mut source_tombstones: Vec<LayerId> = Vec::new();
        for &id in &unique {
            source_tombstones.extend(self.collect_pixel_node_ids(id));
        }

        // Detach every source. Their textures stay alive in node_textures
        // as tombstones owned by the BakeLayersAction.
        for &id in &unique {
            self.doc.detach_for_undo(id);
        }

        // Reposition the result at the topmost's original slot. Detach +
        // reinsert is the simplest exact-slot landing.
        self.doc.detach_for_undo(result_id);
        self.doc
            .reinsert_entity(result_id, topmost_parent, topmost_pos);

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
}
