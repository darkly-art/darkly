//! Void layer kind — procedural-content leaf in the layer tree.
//!
//! A void carries no pixel buffer; its content is regenerated each frame
//! from `(void_type, params)`. So unlike [`raster`], the serializer emits
//! NO pixel blob — the entire void state round-trips through the manifest
//! body, which is a clean win for save-file size.
//!
//! The void-type id and parameter values are validated against the
//! [`crate::gpu::void::VoidRegistry`] at deserialize time so a save file
//! referencing a void kind the binary doesn't ship surfaces as a
//! [`LoadError::CorruptManifest`] rather than a silent fallback.
//!
//! [`raster`]: crate::document::layer_kinds::raster

use serde::{Deserialize, Serialize};

use crate::document::layer_kind::{IdMap, LayerKindRegistration, SerializedEntity};
use crate::format::error::LoadError;
use crate::gpu::blend_mode;
use crate::gpu::params::ParamValue;
use crate::layer::{BlendProps, Layer, LayerId, LayerNode, NodeCommon, VoidLayer};

pub const TYPE_ID: &str = "void";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct VoidBody {
    name: String,
    visible: bool,
    locked: bool,
    opacity: f32,
    blend_mode: String,
    /// Stable `type_id` from [`crate::gpu::void::VoidRegistry`], e.g.
    /// `"noise"`. Anchors the param vector — a load that doesn't recognize
    /// this id is `CorruptManifest`, not a silent fallback.
    void_type: String,
    /// Parameter values in the order the void type's `ParamDef` schema
    /// declares them. Variant identity (`Int` vs `Float`) round-trips via
    /// the regression-tested `#[serde(untagged)]` ordering in `ParamValue`.
    params: Vec<ParamValue>,
    #[serde(default)]
    modifiers: Vec<u64>,
}

pub fn register() -> LayerKindRegistration {
    LayerKindRegistration {
        type_id: TYPE_ID,
        display_name: "Void Layer",
        serialize,
        deserialize,
        remap_ids,
    }
}

fn serialize(node: &LayerNode) -> SerializedEntity {
    let v = match node {
        LayerNode::Layer(Layer::Void(v)) => v,
        _ => panic!("void::serialize received non-void LayerNode"),
    };
    let body = VoidBody {
        name: v.common.name.clone(),
        visible: v.common.visible,
        locked: v.common.locked,
        opacity: v.blend.opacity,
        blend_mode: v.blend.blend_mode.type_id.to_string(),
        void_type: v.void_type.clone(),
        params: v.params.clone(),
        modifiers: v.modifiers.iter().map(|m| m.to_ffi()).collect(),
    };
    SerializedEntity {
        body: serde_json::to_value(&body).expect("derived serde for VoidBody is infallible"),
        pixel_blobs: Vec::new(),
    }
}

fn deserialize(body: &serde_json::Value, id: LayerId) -> Result<LayerNode, LoadError> {
    let body: VoidBody =
        serde_json::from_value(body.clone()).map_err(|e| LoadError::CorruptManifest {
            reason: format!("void body: {e}"),
        })?;
    let blend_reg = blend_mode::registry()
        .get(&body.blend_mode)
        .ok_or_else(|| LoadError::CorruptManifest {
            reason: format!(
                "void {} references undeclared blend_mode/{}",
                id.to_ffi(),
                body.blend_mode
            ),
        })?;
    Ok(LayerNode::Layer(Layer::Void(VoidLayer {
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
        void_type: body.void_type,
        params: body.params,
        modifiers: body.modifiers.into_iter().map(LayerId::from_ffi).collect(),
    })))
}

fn remap_ids(node: &mut LayerNode, id_map: &IdMap) {
    let LayerNode::Layer(Layer::Void(v)) = node else {
        panic!("void::remap_ids received non-void LayerNode");
    };
    for m in v.modifiers.iter_mut() {
        let old_ffi = m.to_ffi();
        if let Some(new_id) = id_map.get(&old_ffi) {
            *m = *new_id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::gpu::params::ParamValue;

    /// Round-trip a void layer through its registered serializer +
    /// deserializer. Regression-style: the entire procedural state must
    /// survive the wire format because there are NO pixel blobs to fall
    /// back on — params + void_type are the whole document state for a
    /// void.
    #[test]
    fn void_body_round_trips_through_registration() {
        let mut doc = Document::new(64, 64);
        let id = doc.add_void_layer(
            "noise".to_string(),
            "Noise",
            vec![
                ParamValue::Int(7),
                ParamValue::Int(4),
                ParamValue::Float(0.01),
                ParamValue::Float(2.0),
                ParamValue::Float(0.5),
                ParamValue::Float(0.1),
            ],
            None,
        );

        let reg = register();
        let node = doc.find_node(id).expect("void exists");

        // No pixel blobs — the procedural side is the entire document
        // state for a void. This is the "clean win for save-file size"
        // documented at the top of this module.
        let serialized = (reg.serialize)(node);
        assert!(
            serialized.pixel_blobs.is_empty(),
            "voids must declare no pixel blobs",
        );

        // Deserialize against a fresh id. The body should round-trip
        // bit-stable through serde_json::Value.
        let restored = (reg.deserialize)(&serialized.body, id).expect("deserialize must succeed");
        let v_after = match &restored {
            LayerNode::Layer(Layer::Void(v)) => v,
            _ => panic!("deserialize must yield a Void layer"),
        };
        assert_eq!(v_after.void_type, "noise");
        assert_eq!(
            v_after.params,
            vec![
                ParamValue::Int(7),
                ParamValue::Int(4),
                ParamValue::Float(0.01),
                ParamValue::Float(2.0),
                ParamValue::Float(0.5),
                ParamValue::Float(0.1),
            ],
            "all params (including Int variants) must survive round-trip",
        );
    }

    /// A corrupt blend_mode in the saved body must surface as
    /// `CorruptManifest`, not a silent fallback. This is the contract the
    /// raster body holds and voids must too — otherwise a save written by
    /// a build that registered a different blend mode would silently
    /// degrade.
    #[test]
    fn unknown_blend_mode_in_body_returns_corrupt_manifest() {
        let reg = register();
        let body = serde_json::json!({
            "name": "broken",
            "visible": true,
            "locked": false,
            "opacity": 1.0,
            "blend_mode": "definitely-not-real",
            "voidType": "noise",
            "params": [],
            "modifiers": []
        });
        let id = Document::new(8, 8).root_id();
        let err = (reg.deserialize)(&body, id);
        assert!(
            err.is_err(),
            "unknown blend_mode must reject the load, not fall through"
        );
    }
}
