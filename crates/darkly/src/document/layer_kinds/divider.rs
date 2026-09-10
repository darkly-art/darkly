//! Divider layer kind: the screen-space boundary as a node in the tree.
//!
//! The divider is the single node among the root's children that splits canvas
//! space (children below it) from screen space (children above it). Making the
//! boundary a node gives it an index: crossing it is an ordinary reorder,
//! moving it is an ordinary layer move, and undo of either is the ordinary
//! `LayerMoveAction`.
//!
//! The divider carries no user-editable state: no pixels, no blend that is
//! ever read, no name the panel shows editable. Its entire document meaning is
//! its position, which lives in the root's children list like any node's. The
//! structural invariant (exactly one, always a direct child of the root) is
//! established by `Document::new`, defended by the capability flags below plus
//! the move validation, and normalized on load.

use crate::document::layer_kind::{IdMap, LayerKindRegistration, SerializedEntity};
use crate::format::error::LoadError;
use crate::layer::{DividerLayer, Layer, LayerId, LayerNode};

pub const TYPE_ID: &str = "divider";

pub fn register() -> LayerKindRegistration {
    LayerKindRegistration {
        type_id: TYPE_ID,
        display_name: "Viewport Divider",
        description: "The boundary between canvas-space layers and viewport-only layers.",
        can_have_mask: false,
        leaf_renders_after_view_transform: false,
        can_rename: false,
        can_delete: false,
        can_duplicate: false,
        screen_space_boundary: true,
        has_thumbnail: false,
        icon: "fa6-solid:water",
        serialize,
        deserialize,
        remap_ids,
    }
}

fn serialize(node: &LayerNode) -> SerializedEntity {
    match node {
        LayerNode::Layer(Layer::Divider(_)) => {}
        _ => panic!("divider::serialize received non-divider LayerNode"),
    }
    // The divider's document meaning is its position in the root's children
    // list, which the root's own body records. Nothing else survives on it.
    SerializedEntity {
        body: serde_json::Value::Object(serde_json::Map::new()),
        pixel_blobs: Vec::new(),
    }
}

fn deserialize(_body: &serde_json::Value, id: LayerId) -> Result<LayerNode, LoadError> {
    Ok(LayerNode::Layer(Layer::Divider(DividerLayer::new(id))))
}

fn remap_ids(_node: &mut LayerNode, _id_map: &IdMap) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The divider round-trips through its registered serializer with an empty
    /// body: position is the root's business, and nothing else survives.
    #[test]
    fn divider_body_round_trips_empty() {
        let doc = crate::document::Document::new(8, 8);
        let id = doc.divider_id();
        let reg = register();
        let node = doc.find_node(id).expect("divider exists");
        let serialized = (reg.serialize)(node);
        assert_eq!(serialized.body, serde_json::json!({}));
        assert!(serialized.pixel_blobs.is_empty());
        let restored = (reg.deserialize)(&serialized.body, id).expect("deserialize succeeds");
        assert!(restored.is_screen_space_boundary());
    }
}
