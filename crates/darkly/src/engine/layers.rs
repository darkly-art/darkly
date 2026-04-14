//! Layer CRUD and property operations.

use super::DarklyEngine;
use crate::document::MoveTarget;
use crate::layer::{BlendMode, Layer, LayerNode};
use crate::undo::{LayerAddAction, LayerRemoveAction, LayerMoveAction, PropertyAction};
use crate::undo::property::Property;

impl DarklyEngine {
    // --- Layer CRUD ---

    pub fn add_raster_layer(&mut self) -> u64 {
        let id = self.doc.add_raster_layer();
        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn add_raster_layer_in(&mut self, group_id: u64) -> u64 {
        let id = self.doc.add_raster_layer_in(Some(group_id));
        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn add_group(&mut self) -> u64 {
        let id = self.doc.add_group();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn remove_layer(&mut self, layer_id: u64) -> Result<(), String> {
        if self.doc.node_count() <= 1 {
            return Err("Cannot delete the last layer".into());
        }

        let parent = self.doc.parent_of(layer_id);
        let pos = self.doc.position_in_parent(layer_id).unwrap_or(0);

        if let Some(node) = self.doc.detach_for_undo(layer_id) {
            self.undo_stack.push(Box::new(LayerRemoveAction::new(node, parent, pos)));
        }

        self.compositor.mark_dirty();
        Ok(())
    }

    pub fn move_layer(&mut self, layer_id: u64, target: MoveTarget) {
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

    pub fn set_opacity(&mut self, layer_id: u64, opacity: f32) {
        let old_opacity = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.opacity,
            Some(LayerNode::Group(g)) => g.opacity,
            _ => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.opacity = opacity,
            Some(LayerNode::Group(g)) => g.opacity = opacity,
            _ => return,
        }

        if let Some(Layer::Raster(r)) = self.doc.layer(layer_id) {
            self.compositor.update_raster_uniforms(
                &self.gpu.queue, layer_id, r.opacity, r.blend_mode,
            );
        } else if let Some(LayerNode::Group(g)) = self.doc.find_node(layer_id) {
            self.compositor.update_group_uniforms(
                &self.gpu.queue, layer_id, g.opacity, g.blend_mode, g.show_mask,
            );
        }
        self.compositor.mark_dirty();

        self.undo_stack.coalesce_property(PropertyAction::new(
            layer_id,
            Property::Opacity(old_opacity),
            Property::Opacity(opacity),
        ));
    }

    pub fn set_blend_mode(&mut self, layer_id: u64, mode: u32) {
        let blend_mode = BlendMode::from_u32(mode);

        let old_mode = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.blend_mode,
            Some(LayerNode::Group(g)) => g.blend_mode,
            _ => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.blend_mode = blend_mode,
            Some(LayerNode::Group(g)) => g.blend_mode = blend_mode,
            _ => return,
        }

        if let Some(Layer::Raster(r)) = self.doc.layer(layer_id) {
            self.compositor.update_raster_uniforms(
                &self.gpu.queue, layer_id, r.opacity, r.blend_mode,
            );
        } else if let Some(LayerNode::Group(g)) = self.doc.find_node(layer_id) {
            self.compositor.update_group_uniforms(
                &self.gpu.queue, layer_id, g.opacity, g.blend_mode, g.show_mask,
            );
        }
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(PropertyAction::new(
            layer_id,
            Property::BlendMode(old_mode),
            Property::BlendMode(blend_mode),
        )));
    }

    pub fn set_layer_visible(&mut self, layer_id: u64, visible: bool) {
        let old_visible = match self.doc.find_node(layer_id) {
            Some(n) => n.visible(),
            None => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.visible = visible,
            Some(LayerNode::Group(g)) => g.visible = visible,
            _ => return,
        }
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(PropertyAction::new(
            layer_id,
            Property::Visible(old_visible),
            Property::Visible(visible),
        )));
    }

    pub fn set_layer_name(&mut self, layer_id: u64, name: &str) {
        let old_name = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.name.clone(),
            Some(LayerNode::Group(g)) => g.name.clone(),
            _ => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.name = name.to_string(),
            Some(LayerNode::Group(g)) => g.name = name.to_string(),
            _ => return,
        }

        self.undo_stack.push(Box::new(PropertyAction::new(
            layer_id,
            Property::Name(old_name),
            Property::Name(name.to_string()),
        )));
    }

    pub fn set_group_collapsed(&mut self, group_id: u64, collapsed: bool) {
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(group_id) {
            g.collapsed = collapsed;
        }
    }

    pub fn set_group_passthrough(&mut self, group_id: u64, passthrough: bool) {
        let old = match self.doc.find_node(group_id) {
            Some(LayerNode::Group(g)) => g.passthrough,
            _ => return,
        };
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(group_id) {
            g.passthrough = passthrough;
        }
        if !passthrough {
            self.compositor.ensure_group_state(&self.gpu.device, &self.gpu.queue, group_id);
            if let Some(LayerNode::Group(g)) = self.doc.find_node(group_id) {
                self.compositor.update_group_uniforms(
                    &self.gpu.queue, group_id, g.opacity, g.blend_mode, g.show_mask,
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
