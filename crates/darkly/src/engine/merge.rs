//! Merge Down: bake the active node + the sibling immediately below it
//! into a single raster layer at the lower sibling's tree slot. Photoshop-
//! style — if either side is a group, it gets flattened during the bake.
//!
//! The undo system tombstones the consumed sources so undo restores
//! everything intact; on redo, the engine recomposes the result from the
//! restored sources via [`crate::gpu::compositor::Compositor::bake_subtree_to_layer`].

use super::DarklyEngine;
use crate::coord::CanvasRect;
use crate::layer::{Layer, LayerId, LayerNode};
use crate::undo::{BakeLayersAction, BakeSourceSlot};

impl DarklyEngine {
    /// Merge the layer at `source_id` into the sibling directly below it in
    /// the same parent. Returns the id of the resulting raster layer.
    ///
    /// Errors when `source_id` has no sibling below it, or when the target
    /// (the layer below) is locked. Either side may be a group; both are
    /// flattened into the result.
    pub fn merge_down(&mut self, source_id: LayerId) -> Result<LayerId, String> {
        // Resolve target: the sibling at (source_position - 1) in the same
        // parent. If source is at position 0 (or has no parent), fail.
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

        // Locked target → refuse (Photoshop convention — can't modify a
        // locked layer's pixels).
        if let Some(t) = self.doc.find_node(target_id) {
            if t.common().locked {
                return Err("Target layer is locked".into());
            }
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
        let canvas_bounds = CanvasRect::from_xywh(0, 0, self.doc.width, self.doc.height);
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

        // Collect tombstone ids BEFORE detaching — once detached, find_node
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
        self.doc.reinsert_node(result_id, parent, target_pos_before);

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
    pub fn can_merge_down(&self, source_id: LayerId) -> bool {
        let Some(pos) = self.doc.position_in_parent(source_id) else {
            return false;
        };
        pos > 0
    }
}
