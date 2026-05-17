//! Duplicate a layer or group, including all descendants and mask modifiers,
//! with a fresh subtree of [`LayerId`]s. The duplicate is placed directly
//! above its source, and the act is recorded as a single [`DuplicateAction`]
//! that tombstones every duplicated texture so its GPU resources are released
//! when the action eventually leaves the undo stack.

use super::DarklyEngine;
use crate::document::MoveTarget;
use crate::layer::{Layer, LayerId, LayerNode};
use crate::undo::DuplicateAction;

impl DarklyEngine {
    /// Duplicate `source_id` and place the copy directly above it in the
    /// same parent. Returns the id of the new top-level node (the duplicated
    /// raster or group). Returns `None` if the source isn't a layer / group.
    pub fn duplicate_node(&mut self, source_id: LayerId) -> Option<LayerId> {
        self.doc.find_node(source_id)?;

        let root_new_id = self.clone_subtree(source_id, None, /* is_root: */ true)?;

        // Anchor the duplicate directly above the source. `add_raster_layer`
        // / `add_group` insert with `anchor=source_id` at the time of
        // creation (see clone_subtree); a second move is unnecessary except
        // when we created via `add_group` on root (which lands at the top).
        // The "anchor above source" placement is built into the document's
        // add methods.

        let parent = self.doc.parent_of(root_new_id);
        let position = self.doc.position_in_parent(root_new_id).unwrap_or(0);
        let tombstones = self.collect_pixel_node_ids(root_new_id);

        self.compositor.mark_dirty();
        self.push_undo(Box::new(DuplicateAction::new(
            root_new_id,
            parent,
            position,
            tombstones,
        )));

        Some(root_new_id)
    }

    /// Recursive subtree clone. `anchor` is the position the new node should
    /// land at (None → use the doc's default insertion). `is_root=true` adds
    /// the " copy" suffix to the top-level name; deeper clones keep their
    /// original names.
    fn clone_subtree(
        &mut self,
        source_id: LayerId,
        anchor: Option<LayerId>,
        is_root: bool,
    ) -> Option<LayerId> {
        // Snapshot the source node so we can read its properties without
        // holding a borrow across the mutating `add_*` calls below.
        let node = self.doc.find_node(source_id)?;
        let common_name = node.common().name.clone();
        let common_visible = node.common().visible;
        let common_locked = node.common().locked;
        let blend_opacity = node.blend().opacity;
        let blend_mode_reg = node.blend().blend_mode;

        match node {
            LayerNode::Layer(Layer::Raster(r)) => {
                let bounds = r.pixels.bounds;
                let new_id = self.doc.add_raster_layer(anchor);
                // Mirror the source's exact bounds so a same-extent
                // `copy_texture_to_texture` keeps every pixel at the right
                // canvas position.
                if let Some(LayerNode::Layer(Layer::Raster(nr))) = self.doc.find_node_mut(new_id) {
                    nr.pixels.bounds = bounds;
                }

                self.compositor.ensure_raster_layer(
                    &self.gpu.device,
                    &self.gpu.queue,
                    new_id,
                    bounds,
                );

                // Apply blend / visibility / lock / name to the new node.
                let new_name = if is_root {
                    format!("{common_name} copy")
                } else {
                    common_name
                };
                if let Some(LayerNode::Layer(Layer::Raster(nr))) = self.doc.find_node_mut(new_id) {
                    nr.common.name = new_name;
                    nr.common.visible = common_visible;
                    nr.common.locked = common_locked;
                    nr.blend.opacity = blend_opacity;
                    nr.blend.blend_mode = blend_mode_reg;
                }
                self.refresh_blend_uniforms(new_id);

                // Copy pixels from source → new.
                self.clone_node_pixels(source_id, new_id);

                // Duplicate every modifier on the source (currently masks).
                self.clone_modifiers(source_id, new_id);

                Some(new_id)
            }
            LayerNode::Group(g) => {
                let children = g.children.clone();
                let passthrough = g.passthrough;
                let collapsed = g.collapsed;
                let new_id = self.doc.add_group(anchor);

                // Apply group properties (name + " copy" suffix on root,
                // blend, visible/locked, passthrough/collapsed).
                let new_name = if is_root {
                    format!("{common_name} copy")
                } else {
                    common_name
                };
                if let Some(LayerNode::Group(ng)) = self.doc.find_node_mut(new_id) {
                    ng.common.name = new_name;
                    ng.common.visible = common_visible;
                    ng.common.locked = common_locked;
                    ng.blend.opacity = blend_opacity;
                    ng.blend.blend_mode = blend_mode_reg;
                    ng.passthrough = passthrough;
                    ng.collapsed = collapsed;
                }
                if !passthrough {
                    self.compositor
                        .ensure_group_state(&self.gpu.device, &self.gpu.queue, new_id);
                    self.refresh_blend_uniforms(new_id);
                }

                // Recursively duplicate every child in source order. Each
                // child is added at the bottom of the new group; subsequent
                // children stack above using the previous as anchor inside
                // the new group.
                let mut last_inside: Option<LayerId> = None;
                for child_id in children {
                    // Add a placeholder child at the right slot, then we
                    // populate by calling clone_subtree which inserts into
                    // the right place.
                    let new_child = self.clone_subtree_into_group(child_id, new_id, last_inside)?;
                    last_inside = Some(new_child);
                }

                // Clone modifiers on the group itself (e.g. group mask).
                self.clone_modifiers(source_id, new_id);

                Some(new_id)
            }
        }
    }

    /// Clone `source_id` and insert the result inside `dest_parent`, after
    /// `anchor_inside` (None → at the bottom of the group). Wrapper around
    /// `clone_subtree` that handles the "insert inside a specific group"
    /// case the bare anchor argument doesn't model.
    fn clone_subtree_into_group(
        &mut self,
        source_id: LayerId,
        dest_parent: LayerId,
        anchor_inside: Option<LayerId>,
    ) -> Option<LayerId> {
        // Reuse the recursive clone but force placement inside `dest_parent`.
        let new_id = self.clone_subtree(source_id, anchor_inside, false)?;

        // If anchor_inside is None the node landed wherever `add_*` placed
        // it; move it into the destination group at the bottom.
        match anchor_inside {
            None => {
                self.doc
                    .move_layer(new_id, MoveTarget::IntoGroupBottom(dest_parent));
            }
            Some(anchor) if self.doc.parent_of(new_id) != Some(dest_parent) => {
                // anchor_inside lives in `dest_parent`; clone_subtree should
                // already have landed `new_id` next to it. Safety net only.
                self.doc.move_layer(new_id, MoveTarget::After(anchor));
            }
            _ => {}
        }

        Some(new_id)
    }

    /// Duplicate every modifier hanging off `src_host` onto `dst_host`. v1
    /// supports mask modifiers; the loop is generic so other future pixel-
    /// bearing modifiers fall in by the same path.
    ///
    /// Goes through `Document::add_mask_modifier` + compositor allocation
    /// directly instead of [`Self::add_mask`] so we don't push a spurious
    /// `ModifierAddAction` that the parent [`DuplicateAction`] already
    /// covers (a single undo step should reverse the whole duplicate).
    fn clone_modifiers(&mut self, src_host: LayerId, dst_host: LayerId) {
        let src_mod_ids = self.doc.modifiers_of(src_host).to_vec();
        for src_mod_id in src_mod_ids {
            if self.doc.mask_modifier_id(src_host) != Some(src_mod_id) {
                continue; // Non-mask modifiers don't ship in v1.
            }
            let Some(new_mod_id) = self.doc.add_mask_modifier(dst_host) else {
                continue;
            };
            let bounds = match self.doc.find_modifier(new_mod_id).and_then(|m| m.pixels()) {
                Some(p) => p.bounds,
                None => continue,
            };
            self.compositor.ensure_node_texture(
                &self.gpu.device,
                &self.gpu.queue,
                new_mod_id,
                wgpu::TextureFormat::R8Unorm,
                bounds,
            );
            self.compositor
                .ensure_passthrough_mask_state(&self.gpu.device, dst_host);
            self.clone_modifier_pixels(src_mod_id, new_mod_id);
            self.compositor.mark_node_pixels_dirty(new_mod_id);
        }
    }
}
