//! Group layer kind — a tree container for nested layers / groups.
//!
//! Per the Modularity Principle in [AGENTS.md], the entire group kind
//! lives in this file: data lives on [`crate::layer::LayerGroup`], wire
//! format (`GroupBody`) and serializer / deserializer / id-remap
//! functions live here.

use serde::{Deserialize, Serialize};

use crate::document::layer_kind::{IdMap, LayerKindRegistration, SerializedEntity};
use crate::format::error::LoadError;
use crate::gpu::blend_mode;
use crate::layer::{BlendProps, LayerGroup, LayerId, LayerNode, NodeCommon};

pub const TYPE_ID: &str = "group";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct GroupBody {
    name: String,
    visible: bool,
    locked: bool,
    opacity: f32,
    blend_mode: String,
    passthrough: bool,
    collapsed: bool,
    /// Child node ids in display order (bottom-to-top).
    children: Vec<u64>,
    #[serde(default)]
    modifiers: Vec<u64>,
}

pub fn register() -> LayerKindRegistration {
    LayerKindRegistration {
        type_id: TYPE_ID,
        display_name: "Group",
        serialize,
        deserialize,
        remap_ids,
    }
}

fn serialize(node: &LayerNode) -> SerializedEntity {
    let g = match node {
        LayerNode::Group(g) => g,
        _ => panic!("group::serialize received non-group LayerNode"),
    };
    let body = GroupBody {
        name: g.common.name.clone(),
        visible: g.common.visible,
        locked: g.common.locked,
        opacity: g.blend.opacity,
        blend_mode: g.blend.blend_mode.type_id.to_string(),
        passthrough: g.passthrough,
        collapsed: g.collapsed,
        children: g.children.iter().map(|c| c.to_ffi()).collect(),
        modifiers: g.modifiers.iter().map(|m| m.to_ffi()).collect(),
    };
    SerializedEntity {
        body: serde_json::to_value(&body).expect("derived serde for GroupBody is infallible"),
        pixel_blobs: Vec::new(),
    }
}

fn deserialize(body: &serde_json::Value, id: LayerId) -> Result<LayerNode, LoadError> {
    let body: GroupBody =
        serde_json::from_value(body.clone()).map_err(|e| LoadError::CorruptManifest {
            reason: format!("group body: {e}"),
        })?;
    let blend_reg = blend_mode::registry()
        .get(&body.blend_mode)
        .ok_or_else(|| LoadError::CorruptManifest {
            reason: format!(
                "group {} references undeclared blend_mode/{} \
                 — `requires` block lies",
                id.to_ffi(),
                body.blend_mode
            ),
        })?;
    Ok(LayerNode::Group(LayerGroup {
        id,
        common: NodeCommon {
            name: body.name,
            visible: body.visible,
            locked: body.locked,
        },
        blend: BlendProps {
            opacity: body.opacity,
            blend_mode: blend_reg,
        },
        children: body.children.into_iter().map(LayerId::from_ffi).collect(),
        modifiers: body.modifiers.into_iter().map(LayerId::from_ffi).collect(),
        passthrough: body.passthrough,
        collapsed: body.collapsed,
    }))
}

fn remap_ids(node: &mut LayerNode, id_map: &IdMap) {
    let LayerNode::Group(g) = node else {
        panic!("group::remap_ids received non-group LayerNode");
    };
    for c in g.children.iter_mut() {
        if let Some(new_id) = id_map.get(&c.to_ffi()) {
            *c = *new_id;
        }
    }
    for m in g.modifiers.iter_mut() {
        if let Some(new_id) = id_map.get(&m.to_ffi()) {
            *m = *new_id;
        }
    }
}
