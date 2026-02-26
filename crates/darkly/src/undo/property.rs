use super::UndoAction;
use crate::document::Document;
use crate::layer::{BlendMode, Layer, LayerId, LayerNode};
use std::collections::{HashMap, HashSet};

/// A layer property value that can be saved and restored.
pub enum Property {
    Opacity(f32),
    BlendMode(BlendMode),
    Visible(bool),
    Name(String),
    Passthrough(bool),
    Collapsed(bool),
}

impl Property {
    /// Apply this property value to the layer/group in the document.
    fn apply(&self, doc: &mut Document, layer_id: LayerId) {
        match self {
            Property::Opacity(v) => {
                match doc.find_node_mut(layer_id) {
                    Some(LayerNode::Layer(Layer::Raster(r))) => r.opacity = *v,
                    Some(LayerNode::Group(g)) => g.opacity = *v,
                    _ => {}
                }
            }
            Property::BlendMode(v) => {
                match doc.find_node_mut(layer_id) {
                    Some(LayerNode::Layer(Layer::Raster(r))) => r.blend_mode = *v,
                    Some(LayerNode::Group(g)) => g.blend_mode = *v,
                    _ => {}
                }
            }
            Property::Visible(v) => {
                match doc.find_node_mut(layer_id) {
                    Some(LayerNode::Layer(Layer::Raster(r))) => r.visible = *v,
                    Some(LayerNode::Layer(Layer::Filter(f))) => f.visible = *v,
                    Some(LayerNode::Group(g)) => g.visible = *v,
                    _ => {}
                }
            }
            Property::Name(v) => {
                match doc.find_node_mut(layer_id) {
                    Some(LayerNode::Layer(Layer::Raster(r))) => r.name = v.clone(),
                    Some(LayerNode::Group(g)) => g.name = v.clone(),
                    _ => {}
                }
            }
            Property::Passthrough(v) => {
                if let Some(LayerNode::Group(g)) = doc.find_node_mut(layer_id) {
                    g.passthrough = *v;
                }
            }
            Property::Collapsed(v) => {
                if let Some(LayerNode::Group(g)) = doc.find_node_mut(layer_id) {
                    g.collapsed = *v;
                }
            }
        }
    }
}

/// Undo action for a property change on a layer or group.
pub struct PropertyAction {
    layer_id: LayerId,
    old_value: Property,
    new_value: Property,
}

impl PropertyAction {
    pub fn new(layer_id: LayerId, old_value: Property, new_value: Property) -> Self {
        PropertyAction {
            layer_id,
            old_value,
            new_value,
        }
    }
}

impl UndoAction for PropertyAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.old_value.apply(doc, self.layer_id);
        HashMap::new()
    }

    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        self.new_value.apply(doc, self.layer_id);
        HashMap::new()
    }
}
