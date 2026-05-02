use super::UndoAction;
use crate::document::Document;
use crate::layer::{BlendMode, LayerId, LayerNode};
use std::collections::{HashMap, HashSet};

/// A layer property value that can be saved and restored.
#[derive(Clone)]
pub enum Property {
    Opacity(f32),
    BlendMode(BlendMode),
    Visible(bool),
    Name(String),
    Passthrough(bool),
    Collapsed(bool),
}

impl Property {
    /// Returns true if both values are the same property kind (e.g. both Opacity).
    pub fn same_kind(&self, other: &Property) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    /// Apply this property value to the layer/group in the document.
    fn apply(&self, doc: &mut Document, layer_id: LayerId) {
        let node = match doc.find_node_mut(layer_id) {
            Some(n) => n,
            None => return,
        };
        match self {
            Property::Opacity(v) => node.common_mut().opacity = *v,
            Property::BlendMode(v) => node.common_mut().blend_mode = *v,
            Property::Visible(v) => node.common_mut().visible = *v,
            Property::Name(v) => node.common_mut().name = v.clone(),
            Property::Passthrough(v) => {
                if let LayerNode::Group(g) = node {
                    g.passthrough = *v;
                }
            }
            Property::Collapsed(v) => {
                if let LayerNode::Group(g) = node {
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

    /// Try to coalesce another PropertyAction into this one.
    /// Succeeds if both target the same layer and same property kind,
    /// in which case we keep our `old_value` and take their `new_value`.
    pub fn try_coalesce(&mut self, other: &PropertyAction) -> bool {
        if self.layer_id == other.layer_id && self.new_value.same_kind(&other.new_value) {
            self.new_value = other.new_value.clone();
            true
        } else {
            false
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

    fn try_coalesce_property(&mut self, other: &PropertyAction) -> bool {
        self.try_coalesce(other)
    }
}
