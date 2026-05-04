//! Layer CRUD and property operations.

use super::DarklyEngine;
use crate::document::MoveTarget;
use crate::layer::{BlendMode, Layer, LayerId, LayerNode};
use crate::undo::property::Property;
use crate::undo::{LayerAddAction, LayerMoveAction, LayerRemoveAction, PropertyAction};

impl DarklyEngine {
    // --- Layer CRUD ---

    pub fn add_raster_layer(&mut self) -> LayerId {
        let id = self.doc.add_raster_layer();
        let bounds = match self.doc.layer(id) {
            Some(Layer::Raster(r)) => r.pixels.bounds,
            _ => crate::coord::CanvasRect::from_xywh(0, 0, self.doc.width, self.doc.height),
        };
        self.compositor
            .ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id, bounds);
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack
            .push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn add_raster_layer_in(&mut self, group_id: LayerId) -> LayerId {
        let id = self.doc.add_raster_layer_in(Some(group_id));
        let bounds = match self.doc.layer(id) {
            Some(Layer::Raster(r)) => r.pixels.bounds,
            _ => crate::coord::CanvasRect::from_xywh(0, 0, self.doc.width, self.doc.height),
        };
        self.compositor
            .ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id, bounds);
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack
            .push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn add_group(&mut self) -> LayerId {
        let id = self.doc.add_group();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack
            .push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn has_layer(&self, layer_id: LayerId) -> bool {
        // "Has" means linked into the tree — not just sitting orphaned in the
        // document's slotmap waiting on an undo reattach. Detached-for-undo
        // layers must report `false` so callers (and the layer panel) treat
        // them as gone until reattach.
        self.doc.layer(layer_id).is_some() && self.doc.parent_of(layer_id).is_some()
    }

    /// Returns the layer's pixel-space bounds in canvas coordinates.
    /// Used by tests and the WASM bridge to report storage extent.
    pub fn layer_bounds(&self, layer_id: LayerId) -> Option<crate::coord::CanvasRect> {
        match self.doc.layer(layer_id)? {
            Layer::Raster(r) => Some(r.pixels.bounds),
        }
    }

    /// Returns the pixel-space bounds of any pixel-bearing node id (raster
    /// layer or mask modifier). Generalization of [`Self::layer_bounds`] —
    /// when callers hold a node id without knowing its kind, this resolves
    /// against the document's unified `pixels()` accessor. Returns `None`
    /// for groups (no pixel buffer) or unknown ids.
    pub fn node_pixel_bounds(&self, node_id: LayerId) -> Option<crate::coord::CanvasRect> {
        if let Some(rect) = self.layer_bounds(node_id) {
            return Some(rect);
        }
        self.doc
            .find_modifier(node_id)
            .and_then(|m| m.pixels())
            .map(|p| p.bounds)
    }

    pub fn remove_layer(&mut self, layer_id: LayerId) -> Result<(), String> {
        if self.doc.node_count() <= 1 {
            return Err("Cannot delete the last layer".into());
        }

        let parent = self.doc.parent_of(layer_id);
        let pos = self.doc.position_in_parent(layer_id).unwrap_or(0);

        if self.doc.detach_for_undo(layer_id).is_some() {
            // Drop per-layer GPU state to avoid leaking textures across
            // delete-then-add cycles. The orphaned node in the slotmap
            // keeps the layer's metadata for undo; pixel data does not
            // survive (see Compositor::dispose_layer).
            self.compositor.dispose_layer(layer_id);
            self.undo_stack
                .push(Box::new(LayerRemoveAction::new(layer_id, parent, pos)));
        }

        self.compositor.mark_dirty();
        Ok(())
    }

    pub fn move_layer(&mut self, layer_id: LayerId, target: MoveTarget) {
        let old_parent = self.doc.parent_of(layer_id);
        let old_pos = match self.doc.position_in_parent(layer_id) {
            Some(p) => p,
            None => return,
        };

        self.doc.move_layer(layer_id, target);

        let new_parent = self.doc.parent_of(layer_id);
        let new_pos = self.doc.position_in_parent(layer_id).unwrap_or(0);

        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(LayerMoveAction::new(
            layer_id, old_parent, old_pos, new_parent, new_pos,
        )));
    }

    // --- Layer properties ---

    pub fn set_opacity(&mut self, layer_id: LayerId, opacity: f32) {
        let old_opacity = match self.doc.find_node(layer_id) {
            Some(n) => n.blend().opacity,
            None => return,
        };
        if let Some(node) = self.doc.find_node_mut(layer_id) {
            node.blend_mut().opacity = opacity;
        } else {
            return;
        }

        self.refresh_blend_uniforms(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.coalesce_property(PropertyAction::new(
            layer_id,
            Property::Opacity(old_opacity),
            Property::Opacity(opacity),
        ));
    }

    pub fn set_blend_mode(&mut self, layer_id: LayerId, mode: u32) {
        let blend_mode = BlendMode::from_u32(mode);
        let old_mode = match self.doc.find_node(layer_id) {
            Some(n) => n.blend().blend_mode,
            None => return,
        };
        if let Some(node) = self.doc.find_node_mut(layer_id) {
            node.blend_mut().blend_mode = blend_mode;
        } else {
            return;
        }

        self.refresh_blend_uniforms(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(PropertyAction::new(
            layer_id,
            Property::BlendMode(old_mode),
            Property::BlendMode(blend_mode),
        )));
    }

    /// Set the `visible` flag on any node — layer, group, or modifier.
    /// Works uniformly across kinds because they all carry [`NodeCommon`].
    pub fn set_layer_visible(&mut self, node_id: LayerId, visible: bool) {
        // Try layers/groups first; fall through to modifiers.
        if let Some(node) = self.doc.find_node_mut(node_id) {
            let old = node.common().visible;
            node.common_mut().visible = visible;
            self.compositor.mark_dirty();
            self.undo_stack
                .push(Box::new(crate::undo::NodeVisibleAction::new(node_id, old)));
        } else if let Some(modifier) = self.doc.find_modifier_mut(node_id) {
            let old = modifier.common.visible;
            modifier.common.visible = visible;
            self.compositor.mark_dirty();
            self.undo_stack
                .push(Box::new(crate::undo::NodeVisibleAction::new(node_id, old)));
        }
    }

    /// Set the `locked` flag on any node — layer, group, or modifier.
    pub fn set_node_locked(&mut self, node_id: LayerId, locked: bool) {
        if let Some(node) = self.doc.find_node_mut(node_id) {
            let old = node.common().locked;
            node.common_mut().locked = locked;
            self.undo_stack
                .push(Box::new(crate::undo::NodeLockedAction::new(node_id, old)));
        } else if let Some(modifier) = self.doc.find_modifier_mut(node_id) {
            let old = modifier.common.locked;
            modifier.common.locked = locked;
            self.undo_stack
                .push(Box::new(crate::undo::NodeLockedAction::new(node_id, old)));
        }
    }

    /// Set the session-level "isolate this node" flag.
    ///
    /// When `Some(id)`, the renderer treats `id`'s subtree as the only
    /// thing on the canvas: the compose walk skips off-path siblings and,
    /// when `id` is a mask modifier, the host's blend pass renders the
    /// mask channel as grayscale.
    ///
    /// Pure session state — no document mutation. The eye-icon column on
    /// every layer is independent: toggling visibility while isolated
    /// modifies that layer's `visible` field, and clearing isolation
    /// preserves whatever the user set.
    pub fn set_isolated_node(&mut self, id: Option<LayerId>) {
        if self.isolated_node == id {
            return;
        }
        self.isolated_node = id;
        // Mirror to the compositor so the render walk can filter off-path
        // subtrees, then resync host uniforms — the `isolated` flag on a
        // host flips depending on whether one of its modifiers is the new
        // target.
        self.compositor.set_isolated_node(id);
        self.sync_compositor_layers();
        self.compositor.mark_dirty();
    }

    /// Read the current isolated-node id, if any.
    pub fn isolated_node(&self) -> Option<LayerId> {
        self.isolated_node
    }

    /// True when the host's `isolated` blend uniform should fire — i.e. the
    /// current isolation target is one of `host_id`'s modifiers (the user
    /// asked to see the mask channel as grayscale on canvas). Isolating the
    /// host itself doesn't trigger this; the host renders normally and the
    /// compose walk hides its siblings instead.
    pub(crate) fn host_renders_isolated(&self, host_id: LayerId) -> bool {
        match self.isolated_node {
            Some(t) => self.doc.modifiers_of(host_id).contains(&t),
            None => false,
        }
    }

    pub fn set_layer_name(&mut self, layer_id: LayerId, name: &str) {
        let old_name = match self.doc.find_node(layer_id) {
            Some(n) => n.common().name.clone(),
            None => return,
        };
        if let Some(node) = self.doc.find_node_mut(layer_id) {
            node.common_mut().name = name.to_string();
        } else {
            return;
        }

        self.undo_stack.push(Box::new(PropertyAction::new(
            layer_id,
            Property::Name(old_name),
            Property::Name(name.to_string()),
        )));
    }

    /// Push the current opacity/blend_mode of a layer or group into the
    /// compositor's uniform buffer for that node. Group isolation is driven
    /// by `engine.isolated_node` and reflected uniformly across node kinds.
    fn refresh_blend_uniforms(&mut self, layer_id: LayerId) {
        match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => {
                self.compositor.update_raster_uniforms(
                    &self.gpu.queue,
                    layer_id,
                    r.blend.opacity,
                    r.blend.blend_mode,
                );
            }
            Some(LayerNode::Group(g)) => {
                let opacity = g.blend.opacity;
                let blend_mode = g.blend.blend_mode;
                let isolated = self.host_renders_isolated(layer_id);
                self.compositor.update_group_uniforms(
                    &self.gpu.queue,
                    layer_id,
                    opacity,
                    blend_mode,
                    isolated,
                );
            }
            None => {}
        }
    }

    pub fn set_group_collapsed(&mut self, group_id: LayerId, collapsed: bool) {
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(group_id) {
            g.collapsed = collapsed;
        }
    }

    pub fn set_group_passthrough(&mut self, group_id: LayerId, passthrough: bool) {
        let old = match self.doc.find_node(group_id) {
            Some(LayerNode::Group(g)) => g.passthrough,
            _ => return,
        };
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(group_id) {
            g.passthrough = passthrough;
        }
        if !passthrough {
            self.compositor
                .ensure_group_state(&self.gpu.device, &self.gpu.queue, group_id);
            let isolated = self.host_renders_isolated(group_id);
            if let Some(LayerNode::Group(g)) = self.doc.find_node(group_id) {
                self.compositor.update_group_uniforms(
                    &self.gpu.queue,
                    group_id,
                    g.blend.opacity,
                    g.blend.blend_mode,
                    isolated,
                );
            }
        }
        self.compositor.mark_dirty();
        self.undo_stack.push(Box::new(PropertyAction::new(
            group_id,
            Property::Passthrough(old),
            Property::Passthrough(passthrough),
        )));
    }
}
