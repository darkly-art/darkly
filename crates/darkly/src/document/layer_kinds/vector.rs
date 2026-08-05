//! Vector layer kind — a leaf holding an ordered list of vector objects
//! (text today; paths/shapes later, reusing the same layer and renderer).
//!
//! A vector layer carries no pixel buffer. Its entire authoritative state — the
//! object list, the layer transform, blend — round-trips through the manifest
//! body, exactly like a procedural void with no persistent frame. The GPU
//! texture is a realization the compositor rebuilds from the objects when they
//! change (raster-first; never on view zoom). Geometry and style serialize via
//! kurbo/peniko's own `serde` impls, so there are no pixel blobs to emit.

use serde::{Deserialize, Serialize};

use crate::document::layer_kind::{IdMap, LayerKindRegistration, SerializedEntity};
use crate::format::error::LoadError;
use crate::gpu::blend_mode;
use crate::layer::{BlendProps, Layer, LayerId, LayerNode, NodeCommon, VectorLayer, VectorObject};

pub const TYPE_ID: &str = "vector";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VectorBody {
    name: String,
    visible: bool,
    locked: bool,
    opacity: f32,
    blend_mode: String,
    /// The authoritative object list. Each object's geometry (`kurbo::BezPath`
    /// / `Affine`), its stable [`crate::layer::ObjectId`], style (`peniko`
    /// brushes/stroke), and the bespoke `TextProps` round-trip through their
    /// derived `serde` impls.
    objects: Vec<VectorObject>,
    /// Next monotonic object id. Persisted so a reload never re-issues a live
    /// id (object identity is per-layer, never reused).
    next_object_id: u64,
    /// Layer-level gizmo transform. `#[serde(default)]` so a pre-transform save
    /// loads as identity.
    #[serde(default)]
    transform: crate::transform::Transform,
    #[serde(default)]
    modifiers: Vec<u64>,
}

pub fn register() -> LayerKindRegistration {
    LayerKindRegistration {
        type_id: TYPE_ID,
        display_name: "Vector Layer",
        description: "Resolution-independent shapes and text, rasterized at draw time.",
        can_have_mask: true,
        can_rename: true,
        has_thumbnail: false,
        icon: "tabler:vector",
        serialize,
        deserialize,
        remap_ids,
    }
}

fn serialize(node: &LayerNode) -> SerializedEntity {
    let v = match node {
        LayerNode::Layer(Layer::Vector(v)) => v,
        _ => panic!("vector::serialize received non-vector LayerNode"),
    };
    let body = VectorBody {
        name: v.common.name.clone(),
        visible: v.common.visible,
        locked: v.common.locked,
        opacity: v.blend.opacity,
        blend_mode: v.blend.blend_mode.type_id.to_string(),
        objects: v.objects.clone(),
        next_object_id: v.next_object_id,
        transform: v.transform,
        modifiers: v.filters.iter().map(|m| m.to_ffi()).collect(),
    };
    SerializedEntity {
        body: serde_json::to_value(&body).expect("derived serde for VectorBody is infallible"),
        pixel_blobs: Vec::new(),
    }
}

fn deserialize(body: &serde_json::Value, id: LayerId) -> Result<LayerNode, LoadError> {
    let body: VectorBody =
        serde_json::from_value(body.clone()).map_err(|e| LoadError::CorruptManifest {
            reason: format!("vector body: {e}"),
        })?;
    let blend_reg = blend_mode::registry()
        .get(&body.blend_mode)
        .ok_or_else(|| LoadError::CorruptManifest {
            reason: format!(
                "vector {} references undeclared blend_mode/{}",
                id.to_ffi(),
                body.blend_mode
            ),
        })?;
    Ok(LayerNode::Layer(Layer::Vector(VectorLayer {
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
        objects: body.objects,
        next_object_id: body.next_object_id,
        transform: body.transform,
        filters: body.modifiers.into_iter().map(LayerId::from_ffi).collect(),
    })))
}

fn remap_ids(node: &mut LayerNode, id_map: &IdMap) {
    let LayerNode::Layer(Layer::Vector(v)) = node else {
        panic!("vector::remap_ids received non-vector LayerNode");
    };
    for m in v.filters.iter_mut() {
        let old_ffi = m.to_ffi();
        if let Some(new_id) = id_map.get(&old_ffi) {
            *m = *new_id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{TextAlign, TextLayout, TextProps, TextStyle, VectorObject};
    use kurbo::Affine;
    use peniko::{Brush, Color};
    use slotmap::SlotMap;

    fn text_layer() -> (LayerNode, LayerId) {
        let mut sm: SlotMap<LayerId, ()> = SlotMap::with_key();
        let id = sm.insert(());
        let text = TextProps {
            content: "Hello\nDarkly".to_string(),
            font_family: "sans-serif".to_string(),
            style: TextStyle::Italic,
            variations: [("wght".to_string(), 600.0)].into_iter().collect(),
            features: [("liga".to_string(), 1)].into_iter().collect(),
            size: 42.0,
            line_height: 1.3,
            letter_spacing: 0.5,
            word_spacing: 1.0,
            align: TextAlign::Center,
            layout: TextLayout::Area {
                width: 300.0,
                height: 160.0,
            },
        };
        let obj = VectorObject::text(
            text,
            Affine::translate((120.0, 80.0)),
            Brush::Solid(Color::from_rgba8(10, 20, 30, 255)),
        );
        let mut layer = VectorLayer::new(id, "Text 1".to_string());
        layer.push_object(obj);
        (LayerNode::Layer(Layer::Vector(layer)), id)
    }

    #[test]
    fn round_trips_text_object_and_declares_no_pixels() {
        let (node, id) = text_layer();
        let entity = serialize(&node);
        // Fully procedural — the entire layer lives in the manifest body.
        assert!(
            entity.pixel_blobs.is_empty(),
            "vector layers declare no pixel blobs"
        );

        let restored = deserialize(&entity.body, id).expect("deserialize");
        // Bit-stable: re-serializing the restored layer reproduces the body.
        let reserialized = serialize(&restored);
        assert_eq!(
            entity.body, reserialized.body,
            "vector layer body must round-trip bit-stable"
        );

        // Spot-check the bespoke text fields survived.
        let LayerNode::Layer(Layer::Vector(v)) = &restored else {
            panic!("restored node is not a vector layer");
        };
        assert_eq!(v.objects.len(), 1);
        // Object identity + the monotonic counter survive the round-trip.
        assert_eq!(v.objects[0].id, crate::layer::ObjectId(0));
        assert_eq!(v.next_object_id, 1);
        match &v.objects[0].source {
            crate::layer::ObjectSource::Text(t) => {
                assert_eq!(t.content, "Hello\nDarkly");
                assert_eq!(t.align, TextAlign::Center);
                assert_eq!(t.style, TextStyle::Italic);
                // Font-driven style survives the round-trip: variations,
                // features, spacing, and the area layout.
                assert_eq!(t.variations.get("wght"), Some(&600.0));
                assert_eq!(t.features.get("liga"), Some(&1));
                assert_eq!(t.letter_spacing, 0.5);
                assert_eq!(t.word_spacing, 1.0);
                assert_eq!(
                    t.layout,
                    TextLayout::Area {
                        width: 300.0,
                        height: 160.0
                    }
                );
            }
            _ => panic!("expected a text object"),
        }
    }
}
