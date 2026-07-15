use super::UndoAction;
use crate::document::Document;
use crate::gpu::blend_mode::BlendModeRegistration;
use crate::gpu::params::ParamValue;
use crate::layer::{Layer, LayerId, LayerNode};
use std::collections::{HashMap, HashSet};

/// A layer property value that can be saved and restored.
#[derive(Clone)]
pub enum Property {
    Opacity(f32),
    /// Held as a registry reference so undo records the *same* identity the
    /// document carries — there is no enum copy that could drift.
    BlendMode(&'static BlendModeRegistration),
    Visible(bool),
    Name(String),
    Passthrough(bool),
    Collapsed(bool),
    /// Full parameter vector of a void layer. Same shape the wire format
    /// uses; replaces in bulk because a void's params are dependent on its
    /// `void_type` and only meaningful as a complete schema-aligned vector.
    /// Coalescing on the `same_kind` discriminant collapses a slider drag
    /// (multiple `VoidParams` edits in a row) into one undo step, matching
    /// how opacity behaves.
    VoidParams(Vec<ParamValue>),
    /// Full parameter vector of a filter layer (the five curves of a `curves`
    /// layer, say). Direct analog of `VoidParams`: replaces in bulk, coalesces a
    /// curve drag into one undo step. The compositor rebuilds the derived LUT
    /// from the restored params on the next `sync_projection_states`.
    FilterParams(Vec<ParamValue>),
    /// A void layer's user transform (gizmo-edited pan / scale / rotate).
    /// Coalesces on `same_kind` so a whole gizmo drag is one undo step,
    /// exactly like `VoidParams`. The GPU re-sync happens in
    /// `sync_compositor_layers` after the doc is restored.
    Transform(crate::transform::Transform),
    /// Full object list of a vector layer. Replaced in bulk (like `VoidParams`)
    /// because a text edit / style change rewrites the bespoke object that owns
    /// the change, and the realization rebuilds from the whole list anyway.
    /// Coalesces so a run of edits (e.g. typing) collapses into one undo step;
    /// the compositor re-realizes via `sync_vector_layer` after the doc is
    /// restored.
    VectorObjects(Vec<crate::layer::VectorObject>),
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
            Property::Opacity(v) => node.blend_mut().opacity = *v,
            Property::BlendMode(v) => node.blend_mut().blend_mode = *v,
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
            Property::VoidParams(values) => {
                if let LayerNode::Layer(Layer::Void(v)) = node {
                    v.params = values.clone();
                }
            }
            Property::FilterParams(values) => {
                if let LayerNode::Layer(Layer::Filter(f)) = node {
                    f.params = values.clone();
                }
            }
            Property::Transform(t) => {
                if let LayerNode::Layer(Layer::Void(v)) = node {
                    v.transform = *t;
                }
            }
            Property::VectorObjects(objects) => {
                if let LayerNode::Layer(Layer::Vector(v)) = node {
                    v.objects = objects.clone();
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
    /// Finer-grained coalesce key, layered on top of `(layer_id, Property
    /// kind)`. Default `0` for callers separated by `Property` kind alone
    /// (opacity, blend, void params…). A single `Property` kind that carries
    /// several distinct logical ops — `VectorObjects` is content / style /
    /// transform of one object — sets a non-zero tag so a typing run and a
    /// later gizmo drag don't collapse into one undo step.
    coalesce_tag: u64,
}

impl PropertyAction {
    pub fn new(layer_id: LayerId, old_value: Property, new_value: Property) -> Self {
        Self::new_coalescing(layer_id, old_value, new_value, 0)
    }

    /// Like [`Self::new`] but with an explicit coalesce discriminator. Two
    /// actions only merge when their tags also match, so callers that route
    /// multiple logical ops through one `Property` kind can keep them in
    /// separate undo steps.
    pub fn new_coalescing(
        layer_id: LayerId,
        old_value: Property,
        new_value: Property,
        coalesce_tag: u64,
    ) -> Self {
        PropertyAction {
            layer_id,
            old_value,
            new_value,
            coalesce_tag,
        }
    }

    /// Try to coalesce another PropertyAction into this one.
    /// Succeeds if both target the same layer, the same property kind, and the
    /// same coalesce tag — in which case we keep our `old_value` and take their
    /// `new_value`.
    pub fn try_coalesce(&mut self, other: &PropertyAction) -> bool {
        if self.layer_id == other.layer_id
            && self.coalesce_tag == other.coalesce_tag
            && self.new_value.same_kind(&other.new_value)
        {
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
